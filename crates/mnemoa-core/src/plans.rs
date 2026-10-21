//! Plan drift: Spec Kit and Kiro task lists, and Cline memory-bank freshness.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::config::Config;
use crate::discover::SurfaceKind;
use crate::doc::Doc;
use crate::drift::{DriftItem, DriftKind};
use crate::history::History;
use crate::rules::references::{has_extension_or_slash, looks_like_path};
use crate::workspace::Workspace;

static CHECKBOX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[-*]\s+\[([ xX])\]\s+(.*)$").unwrap());
/// Spec Kit: `T012 [P] [US1] Description`.
static SPECKIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(T\d{3,})(?:\s+\[P\])?(?:\s+\[US\d+\])?\s+\S").unwrap());
static SPECKIT_ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(T\d+)\b").unwrap());
/// Kiro: `1. Description` or `2.1 Description`.
static KIRO_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+(?:\.\d+)*\.?\s+\S").unwrap());
static REMOVAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(delete|deleted|remove|removed|drop|dropped|rename|renamed|move|moved|retire|retired)\b").unwrap()
});

/// A checkbox task line.
#[derive(Debug, Clone)]
pub struct Task {
    pub done: bool,
    pub body: String,
    pub offset: usize,
    pub paths: Vec<String>,
}

/// File paths mentioned in a task description (backticked or bare).
pub fn task_paths(body: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for tok in body.split_whitespace() {
        let t =
            tok.trim_matches(|c: char| matches!(c, '`' | ',' | ';' | ':' | '(' | ')' | '"' | '\''));
        let t = t.trim_end_matches(['.', ',', ';', ':', ')']);
        if looks_like_path(t) && has_extension_or_slash(t) && !t.ends_with('/') {
            out.insert(t.trim_start_matches("./").to_string());
        }
    }
    out.into_iter().collect()
}

/// Checkbox tasks outside code blocks.
pub fn tasks(doc: &Doc) -> Vec<Task> {
    let mut out = Vec::new();
    let mut offset = 0;
    for line in doc.text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if doc.in_code_block(start) {
            continue;
        }
        let Some(c) = CHECKBOX_RE.captures(line.trim_end_matches(['\n', '\r'])) else {
            continue;
        };
        let body = c[2].trim().to_string();
        out.push(Task {
            done: &c[1] != " ",
            paths: task_paths(&body),
            body,
            offset: start,
        });
    }
    out
}

fn check_tasks(
    ws: &Workspace,
    doc: &Doc,
    kiro: bool,
    history: Option<&dyn History>,
    out: &mut Vec<DriftItem>,
) {
    let disc = &ws.discovery;
    let list_idx = history.and_then(|h| h.touching(&doc.path).first().copied());
    let mut ids = BTreeSet::new();
    for t in tasks(doc) {
        // Format.
        if kiro {
            if !KIRO_RE.is_match(&t.body) {
                out.push(DriftItem::new(
                    DriftKind::MalformedTask,
                    doc,
                    t.offset,
                    &t.body,
                    "Kiro task has no number (expected `- [ ] 1. Description`).",
                ));
            }
        } else if !SPECKIT_RE.is_match(&t.body) {
            let msg = if SPECKIT_ID_RE.is_match(&t.body) {
                "Spec Kit task does not match `T### [P] [US#] Description` (ID needs three digits; only [P] and [US#] markers)."
            } else {
                "Spec Kit task has no task ID (expected `- [ ] T001 Description`)."
            };
            out.push(DriftItem::new(
                DriftKind::MalformedTask,
                doc,
                t.offset,
                &t.body,
                msg,
            ));
        } else if let Some(id) = SPECKIT_ID_RE.captures(&t.body).map(|c| c[1].to_string())
            && !ids.insert(id.clone())
        {
            out.push(DriftItem::new(
                DriftKind::MalformedTask,
                doc,
                t.offset,
                &id,
                format!("Task ID {id} is used twice."),
            ));
        }

        // Files.
        if t.done {
            if REMOVAL_RE.is_match(&t.body) {
                continue;
            }
            for p in t.paths.iter().filter(|p| !disc.exists(p)) {
                out.push(DriftItem::new(
                    DriftKind::DoneTaskMissingFile,
                    doc,
                    t.offset,
                    p,
                    format!("Task is checked off, but `{p}` does not exist."),
                ));
            }
        } else if let (Some(h), Some(idx)) = (history, list_idx) {
            for p in t.paths.iter().filter(|p| disc.exists(p)) {
                let after = h.commits_touching_since(idx, std::slice::from_ref(p));
                if after > 0 {
                    out.push(DriftItem::new(
                        DriftKind::OpenTaskFileChanged,
                        doc,
                        t.offset,
                        p,
                        format!("Task is open, but `{p}` changed in {after} commit(s) after this task list; the checkbox may be stale."),
                    ));
                }
            }
        }
    }
}

fn check_memory_bank(
    doc: &Doc,
    config: &Config,
    history: Option<&dyn History>,
    out: &mut Vec<DriftItem>,
) {
    let name = doc.path.rsplit('/').next().unwrap_or("");
    if name != "activeContext.md" && name != "progress.md" {
        return;
    }
    let Some(h) = history else { return };
    if h.is_shallow() {
        return;
    }
    let Some(&idx) = h.touching(&doc.path).first() else {
        return;
    };
    let since = h.code_commits_since(idx);
    let max = config.drift.memory_bank_max_commits;
    if since > max {
        out.push(DriftItem::new(
            DriftKind::StaleMemoryBank,
            doc,
            0,
            name,
            format!("{name} has not changed in {since} code commits (threshold {max}); Cline's memory bank expects it to track current work."),
        ));
    }
}

/// Plan drift for every plan surface in the workspace.
pub fn plan_drift(
    ws: &Workspace,
    config: &Config,
    history: Option<&dyn History>,
) -> Vec<DriftItem> {
    let mut out = Vec::new();
    for s in &ws.discovery.surfaces {
        let Some(doc) = ws.doc(&s.path) else { continue };
        match s.kind {
            SurfaceKind::SpecKitTasks => check_tasks(ws, doc, false, history, &mut out),
            SurfaceKind::KiroSpec if s.path.ends_with("/tasks.md") => {
                check_tasks(ws, doc, true, history, &mut out)
            }
            SurfaceKind::ClineMemoryBank => check_memory_bank(doc, config, history, &mut out),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::MemoryHistory;
    use crate::workspace::tests::ws;

    const SPEC: &str = "# Tasks\n\n- [x] T001 Create `src/models/user.py`\n- [x] T002 [P] [US1] Add tests in tests/test_user.py\n- [ ] T003 [US1] Implement src/services/auth.py\n- [ ] T4 bad id\n- [ ] Write docs\n- [x] T005 Delete src/legacy.py\n\n```md\n- [ ] T999 inside a code block\n```\n";

    #[test]
    fn task_parsing() {
        let d = crate::doc::Doc::parse("specs/001/tasks.md", SPEC);
        let t = tasks(&d);
        assert_eq!(t.len(), 6);
        assert!(t[0].done && !t[2].done);
        assert_eq!(t[0].paths, vec!["src/models/user.py"]);
        assert_eq!(t[1].paths, vec!["tests/test_user.py"]);
    }

    #[test]
    fn speckit_drift() {
        let w = ws(&[
            ("specs/001-auth/tasks.md", SPEC),
            ("src/models/user.py", ""),
            ("src/services/auth.py", ""),
        ]);
        let items = plan_drift(&w, &Config::default(), None);
        let kinds: Vec<(DriftKind, &str)> = items
            .iter()
            .map(|i| (i.kind, i.reference.as_str()))
            .collect();
        assert!(
            kinds.contains(&(DriftKind::DoneTaskMissingFile, "tests/test_user.py")),
            "{kinds:?}"
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|(k, _)| *k == DriftKind::MalformedTask)
                .count(),
            2,
            "{kinds:?}"
        );
        // Deleting a file is expected to leave it missing.
        assert!(!kinds.iter().any(|(_, r)| *r == "src/legacy.py"));
        // Existing file for a done task is fine.
        assert!(!kinds.iter().any(|(_, r)| *r == "src/models/user.py"));
    }

    #[test]
    fn open_task_with_later_changes_is_flagged() {
        let w = ws(&[
            ("specs/001-auth/tasks.md", SPEC),
            ("src/models/user.py", ""),
            ("src/services/auth.py", ""),
        ]);
        let h = MemoryHistory::linear(&[
            ("a", &["specs/001-auth/tasks.md", "src/services/auth.py"]),
            ("b", &["src/services/auth.py"]),
        ]);
        let items = plan_drift(&w, &Config::default(), Some(&h));
        assert!(
            items
                .iter()
                .any(|i| i.kind == DriftKind::OpenTaskFileChanged
                    && i.reference == "src/services/auth.py")
        );
    }

    #[test]
    fn kiro_tasks() {
        let w = ws(&[(
            ".kiro/specs/login/tasks.md",
            "- [x] 1. Add `src/login.ts`\n  - _Requirements: 1.1_\n- [ ] 2.1 Wire it up\n- [ ] tidy\n",
        )]);
        let items = plan_drift(&w, &Config::default(), None);
        let kinds: Vec<DriftKind> = items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            vec![DriftKind::DoneTaskMissingFile, DriftKind::MalformedTask],
            "{items:?}"
        );
        // Kiro requirements.md is not a task list.
        let w = ws(&[(".kiro/specs/login/requirements.md", "- [ ] no number\n")]);
        assert!(plan_drift(&w, &Config::default(), None).is_empty());
    }

    #[test]
    fn stale_memory_bank() {
        let w = ws(&[
            ("memory-bank/activeContext.md", "# Now\n"),
            ("memory-bank/projectbrief.md", "# Brief\n"),
            ("src/a.rs", ""),
        ]);
        let mut commits: Vec<(String, Vec<&str>)> = vec![(
            "init".into(),
            vec![
                "memory-bank/activeContext.md",
                "memory-bank/projectbrief.md",
            ],
        )];
        for i in 0..12 {
            commits.push((format!("c{i}"), vec!["src/a.rs"]));
        }
        let refs: Vec<(&str, &[&str])> = commits
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_slice()))
            .collect();
        let h = MemoryHistory::linear(&refs);
        let items = plan_drift(&w, &Config::default(), Some(&h));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, DriftKind::StaleMemoryBank);
        assert_eq!(items[0].path, "memory-bank/activeContext.md");
        // Fresh enough with a higher threshold, and silent without history.
        let mut c = Config::default();
        c.drift.memory_bank_max_commits = 20;
        assert!(plan_drift(&w, &c, Some(&h)).is_empty());
        assert!(plan_drift(&w, &Config::default(), None).is_empty());
    }
}
