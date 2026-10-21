//! Retire rules whose `retire-when` condition is met.
//!
//! Safety-relevant rules (secrets, credentials, production, deletion, migrations) are never
//! retired automatically, whatever their condition says.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::config::Config;
use crate::ledger::{LEDGER_FILE, Ledger, State};
use crate::promote::removal_range;
use crate::rationale::{self, RetireWhen, Sidecar};
use crate::rules::Context;
use crate::workspace::Workspace;

static SAFETY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(secrets?|credentials?|passwords?|tokens?|api[ _-]?keys?|private[ _-]?keys?|prod|production|delete|deletes|deleting|drop|truncate|migrations?|migrate|force[ -]?push|rm\s+-rf|billing|payments?|pii|gdpr)\b").unwrap()
});

/// Whether a rule is too risky to retire without a person.
pub fn is_safety_relevant(rule: &str) -> bool {
    SAFETY_RE.is_match(rule)
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub source: String,
    pub line: usize,
    pub rule: String,
    pub condition: RetireWhen,
    /// The condition holds.
    pub met: bool,
    pub reason: String,
    /// Safety-relevant: reported, never removed automatically.
    pub protected: bool,
    /// `apply` can delete it cleanly (a list item without nested lists).
    pub removable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetirePlan {
    pub candidates: Vec<Candidate>,
    pub diff: String,
    #[serde(skip)]
    pub files: BTreeMap<String, String>,
    pub files_changed: Vec<String>,
}

impl RetirePlan {
    /// Candidates `apply` will delete.
    pub fn retiring(&self) -> impl Iterator<Item = &Candidate> {
        self.candidates
            .iter()
            .filter(|c| c.met && !c.protected && c.removable)
    }
}

/// Check a `check:<id>` condition: the ledger entry exists and is applied; for ast-grep checks,
/// the rule must also report no violations when `ast_grep` is available.
fn check_condition(
    ws: &Workspace,
    ledger: &Ledger,
    id: &str,
    ast_grep: Option<&Path>,
) -> (bool, String) {
    let Some(e) = ledger.by_id(id) else {
        return (false, format!("no check `{id}` in {LEDGER_FILE}"));
    };
    if e.state != State::Applied {
        return (false, format!("check `{id}` is only proposed"));
    }
    if let Some(rule_file) = e
        .artifacts
        .iter()
        .find(|a| a.starts_with(crate::promote::CHECKS_DIR))
    {
        let Some(ag) = ast_grep else {
            return (
                true,
                format!("check `{id}` is applied (ast-grep not installed, so it was not run)"),
            );
        };
        let out = std::process::Command::new(ag)
            .arg("scan")
            .arg("--rule")
            .arg(ws.root.join(rule_file))
            .arg("--json=compact")
            .arg(&ws.root)
            .output();
        let n = out
            .ok()
            .and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .unwrap_or("[]")
                    .to_string();
                serde_json::from_str::<serde_json::Value>(&s).ok()
            })
            .and_then(|v| v.as_array().map(Vec::len));
        return match n {
            Some(0) => (true, format!("check `{id}` passes")),
            Some(n) => (false, format!("check `{id}` reports {n} violation(s)")),
            None => (false, format!("could not run check `{id}`")),
        };
    }
    (true, format!("check `{id}` is applied ({})", e.enforcer))
}

/// Find rules with a retire condition and decide whether each is met.
pub fn plan(ws: &Workspace, config: &Config, today: &str, ast_grep: Option<&Path>) -> RetirePlan {
    let ledger = ws
        .read_file(LEDGER_FILE)
        .and_then(|t| Ledger::parse(&t).ok())
        .unwrap_or_default();
    let mut sidecar = ws
        .read_file(rationale::SIDECAR)
        .and_then(|t| Sidecar::parse(&t).ok());
    let targets = config.targets.clone();
    let cx = Context::new(ws, config, &targets);
    let mut candidates = Vec::new();
    let mut removals: BTreeMap<String, Vec<std::ops::Range<usize>>> = BTreeMap::new();
    let mut sidecar_changed = false;

    for doc in cx.instruction_docs() {
        for rule in doc
            .rules
            .iter()
            .filter(|r| r.is_list_item() || r.imperative)
        {
            let Some(r) = rationale::rationale_for(doc, rule, sidecar.as_ref()) else {
                continue;
            };
            let Some(cond) = r.retire_when.clone() else {
                continue;
            };
            let (met, reason) = match &cond {
                RetireWhen::Manual => (false, "retire-when is manual".to_string()),
                RetireWhen::Date(d) => {
                    if today >= d.as_str() {
                        (true, format!("date {d} has passed"))
                    } else {
                        (false, format!("waits until {d}"))
                    }
                }
                RetireWhen::PathGone(p) => {
                    if ws.discovery.exists(p) {
                        (false, format!("`{p}` still exists"))
                    } else {
                        (true, format!("`{p}` is gone"))
                    }
                }
                RetireWhen::Check(id) => check_condition(ws, &ledger, id, ast_grep),
            };
            let protected = is_safety_relevant(&rule.text);
            let range = removal_range(doc, rule);
            if met
                && !protected
                && let Some(rg) = range.clone()
            {
                removals.entry(doc.path.clone()).or_default().push(rg);
                if let Some(s) = sidecar.as_mut()
                    && s.entries
                        .remove(&rationale::rule_hash(&rule.text))
                        .is_some()
                {
                    sidecar_changed = true;
                }
            }
            candidates.push(Candidate {
                source: doc.path.clone(),
                line: doc.line_of(rule.span.start),
                rule: rule.text.clone(),
                condition: cond,
                met,
                reason: if protected && met {
                    format!("{reason}, but the rule is safety-relevant; remove it by hand if you are sure")
                } else {
                    reason
                },
                protected,
                removable: range.is_some(),
            });
        }
    }

    let mut files = BTreeMap::new();
    for (path, mut ranges) in removals {
        let Some(doc) = ws.doc(&path) else { continue };
        ranges.sort_by_key(|r| r.start);
        let mut out = String::new();
        let mut pos = 0;
        for r in ranges {
            if r.start >= pos {
                out.push_str(&doc.text[pos..r.start]);
                pos = r.end;
            }
        }
        out.push_str(&doc.text[pos..]);
        files.insert(path, out);
    }
    if sidecar_changed && let Some(s) = sidecar {
        files.insert(rationale::SIDECAR.to_string(), s.to_toml());
    }
    let mut diff = String::new();
    for (path, after) in &files {
        let before = ws.read_file(path).unwrap_or_default();
        diff.push_str(&similar::udiff::unified_diff(
            similar::Algorithm::Myers,
            &before,
            after,
            3,
            Some((&format!("a/{path}"), &format!("b/{path}"))),
        ));
    }
    RetirePlan {
        candidates,
        diff,
        files_changed: files.keys().cloned().collect(),
        files,
    }
}

/// Write the plan's file changes.
pub fn apply(ws: &Workspace, plan: &RetirePlan) -> std::io::Result<Vec<String>> {
    for (path, content) in &plan.files {
        std::fs::write(ws.writable_path(path)?, content)?;
    }
    Ok(plan.files.keys().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::ws;

    const DOC: &str = "# Rules\n\n<!-- why: old API | retire-when: path-gone:src/legacy -->\n- Do not call the legacy client.\n<!-- why: launch freeze | retire-when: date:2026-01-01 -->\n- Keep PRs under 200 lines.\n<!-- why: outage | retire-when: date:2026-01-01 -->\n- Never run migrations against prod by hand.\n<!-- why: x | retire-when: manual -->\n- Use the new logger.\n<!-- why: y | retire-when: date:2099-01-01 -->\n- Prefer small functions.\n";

    #[test]
    fn met_conditions_are_retired_except_safety_rules() {
        let w = ws(&[("AGENTS.md", DOC), ("src/main.rs", "")]);
        let p = plan(&w, &Config::default(), "2026-10-21", None);
        assert_eq!(p.candidates.len(), 5);
        let retiring: Vec<&str> = p.retiring().map(|c| c.rule.as_str()).collect();
        assert_eq!(
            retiring,
            vec![
                "Do not call the legacy client.",
                "Keep PRs under 200 lines."
            ]
        );
        let prod = p
            .candidates
            .iter()
            .find(|c| c.rule.contains("migrations"))
            .unwrap();
        assert!(prod.met && prod.protected);
        let after = &p.files["AGENTS.md"];
        assert!(!after.contains("legacy client") && !after.contains("old API"));
        assert!(after.contains("migrations against prod") && after.contains("Use the new logger"));
        assert!(p.diff.contains("-- Keep PRs under 200 lines."));
    }

    #[test]
    fn unmet_conditions_change_nothing() {
        let w = ws(&[("AGENTS.md", DOC), ("src/legacy/x.rs", "")]);
        let p = plan(&w, &Config::default(), "2025-12-31", None);
        assert_eq!(p.retiring().count(), 0);
        assert!(p.files.is_empty());
    }

    #[test]
    fn check_condition_uses_ledger() {
        let ledger = "[[check]]\nid = \"fmt\"\nhash = \"h\"\ntext = \"t\"\nsource = \"AGENTS.md\"\nenforcer = \"editorconfig\"\ndate = \"2026-01-01\"\nstate = \"applied\"\n";
        let doc = "<!-- why: diffs | retire-when: check:fmt -->\n- Use 2 spaces.\n<!-- why: diffs | retire-when: check:missing -->\n- Use tabs in Makefiles.\n";
        let w = ws(&[("AGENTS.md", doc), (LEDGER_FILE, ledger)]);
        let p = plan(&w, &Config::default(), "2026-10-21", None);
        let met: Vec<bool> = p.candidates.iter().map(|c| c.met).collect();
        assert_eq!(met, vec![true, false]);
        assert!(p.candidates[1].reason.contains("no check `missing`"));
    }

    #[test]
    fn sidecar_rationale_is_retired_and_entry_removed() {
        let side = format!(
            "[[rule]]\nhash = \"{}\"\ntext = \"Keep PRs small.\"\nwhy = \"x\"\nretire-when = \"date:2020-01-01\"\n",
            rationale::rule_hash("Keep PRs small.")
        );
        let w = ws(&[
            ("AGENTS.md", "- Keep PRs small.\n- Other.\n"),
            (rationale::SIDECAR, side.as_str()),
        ]);
        let p = plan(&w, &Config::default(), "2026-10-21", None);
        assert_eq!(p.retiring().count(), 1);
        assert_eq!(p.files["AGENTS.md"], "- Other.\n");
        assert!(!p.files[rationale::SIDECAR].contains("Keep PRs small"));
    }

    #[test]
    fn safety_keywords() {
        assert!(is_safety_relevant("Never commit secrets"));
        assert!(is_safety_relevant("Don't force-push to main"));
        assert!(!is_safety_relevant("Keep functions short"));
    }
}
