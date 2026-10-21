//! MNA010 cross-file-divergence, MNA011 missing-rationale and MNA013 unloaded-file.

use std::collections::BTreeSet;

use super::{Check, Context, RULES, RuleInfo, finding_at, finding_file};
use crate::diag::{Finding, Severity};
use crate::discover::{Loading, SurfaceKind, Target};
use crate::doc::Doc;
use crate::rationale::{self, Sidecar};
use crate::workspace::resolve_relative;

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn imports_file(cx: &Context<'_>, from: &str, target: &str) -> bool {
    cx.ws.doc(from).is_some_and(|d| {
        d.imports
            .iter()
            .any(|i| resolve_relative(from, &i.path).as_deref() == Some(target))
    })
}

fn is_symlink(cx: &Context<'_>, path: &str) -> bool {
    cx.ws.mem.is_none()
        && std::fs::symlink_metadata(cx.ws.root.join(path))
            .is_ok_and(|m| m.file_type().is_symlink())
}

fn rule_set(doc: &Doc) -> BTreeSet<String> {
    doc.rules
        .iter()
        .map(|r| {
            r.text
                .to_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .trim_end_matches('.')
                .to_string()
        })
        .collect()
}

/// MNA010.
pub struct CrossFileDivergence;

impl Check for CrossFileDivergence {
    fn info(&self) -> &'static RuleInfo {
        &RULES[9]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        for agents in cx
            .ws
            .discovery
            .surfaces
            .iter()
            .filter(|s| s.kind == SurfaceKind::AgentsMd)
        {
            let dir = agents.dir();
            let candidates = if dir.is_empty() {
                vec![join(dir, "CLAUDE.md"), ".claude/CLAUDE.md".to_string()]
            } else {
                vec![join(dir, "CLAUDE.md")]
            };
            let Some(claude) = candidates.iter().find(|c| cx.ws.discovery.has_file(c)) else {
                continue;
            };
            if imports_file(cx, claude, &agents.path)
                || imports_file(cx, &agents.path, claude)
                || is_symlink(cx, claude)
                || is_symlink(cx, &agents.path)
            {
                continue;
            }
            let (Some(a), Some(c)) = (cx.ws.doc(&agents.path), cx.ws.doc(claude)) else {
                continue;
            };
            if a.text == c.text {
                continue;
            }
            let ra = rule_set(a);
            let rc = rule_set(c);
            let only_agents = ra.difference(&rc).count();
            let only_claude = rc.difference(&ra).count();
            if only_agents == 0 && only_claude == 0 {
                continue;
            }
            let mut parts = Vec::new();
            if only_agents > 0 {
                parts.push(format!(
                    "{only_agents} rule(s) only in {} never reach Claude Code",
                    agents.path
                ));
            }
            if only_claude > 0 {
                parts.push(format!(
                    "{only_claude} rule(s) only in {claude} never reach other agents"
                ));
            }
            out.push(
                finding_file(
                    self.info(),
                    claude,
                    format!(
                        "Claude Code reads {claude} and skips {} when both exist; {}.",
                        agents.path,
                        parts.join(", and ")
                    ),
                )
                .with_snippet(format!("pair:{}", agents.path))
                .with_help(format!(
                    "Make {claude} start with `@{}` and keep only Claude-specific additions below it.",
                    agents.path.rsplit('/').next().unwrap_or("AGENTS.md")
                )),
            );
        }
    }
}

/// MNA011.
pub struct MissingRationale;

impl Check for MissingRationale {
    fn info(&self) -> &'static RuleInfo {
        &RULES[10]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        let sidecar = cx
            .ws
            .read_file(rationale::SIDECAR)
            .and_then(|t| Sidecar::parse(&t).ok());
        for doc in cx.always_docs() {
            for rule in doc.rules.iter().filter(|r| r.imperative) {
                let crate::doc::RuleKind::ListItem { depth: 0, .. } = rule.kind else {
                    continue;
                };
                match rationale::rationale_for(doc, rule, sidecar.as_ref()) {
                    Some(r) if r.errors.is_empty() => {}
                    Some(r) => out.push(finding_at(
                        self.info(),
                        doc,
                        rule.span.clone(),
                        format!("Rationale comment has problems: {}.", r.errors.join("; ")),
                    )),
                    None => out.push(
                        finding_at(self.info(), doc, rule.span.clone(), "Rule has no rationale (why it exists, when it can go).")
                            .with_help("Add `<!-- why: ... | retire-when: ... -->` on the line above, or an entry in .mnemoa/rationale.toml."),
                    ),
                }
            }
        }
    }
}

/// MNA013.
pub struct UnloadedFile;

impl Check for UnloadedFile {
    fn info(&self) -> &'static RuleInfo {
        &RULES[12]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        let info = self.info();
        let claude_loaded: BTreeSet<String> = if cx.targets.contains(&Target::ClaudeCode) {
            cx.ws
                .always_loaded(Target::ClaudeCode)
                .into_iter()
                .map(|f| f.path)
                .collect()
        } else {
            BTreeSet::new()
        };
        for s in &cx.ws.discovery.surfaces {
            for (target, loading) in &s.loading {
                if !cx.targets.contains(target) {
                    continue;
                }
                let Loading::NotLoaded { reason } = loading else {
                    continue;
                };
                let (severity, skip) = match (s.kind, target) {
                    (SurfaceKind::AgentsMd, Target::ClaudeCode) => {
                        let same_dir_claude = cx.ws.discovery.has_file(&join(s.dir(), "CLAUDE.md"))
                            || (s.dir().is_empty()
                                && cx.ws.discovery.has_file(".claude/CLAUDE.md"));
                        // MNA010 covers a CLAUDE.md next to AGENTS.md.
                        (
                            Severity::Warn,
                            same_dir_claude || claude_loaded.contains(&s.path),
                        )
                    }
                    (SurfaceKind::AgentsMd, Target::Codex)
                    | (SurfaceKind::CursorRuleIgnored, _) => (Severity::Info, false),
                    _ => (Severity::Warn, false),
                };
                if skip {
                    continue;
                }
                let mut f = finding_file(
                    info,
                    &s.path,
                    format!("{target} never loads {}: {reason}.", s.path),
                )
                .with_snippet(format!("unloaded:{target}"));
                f.severity = severity;
                out.push(f);
            }
        }
        if cx.targets.contains(&Target::ClaudeCode) {
            for d in cx.ws.dropped_imports() {
                if d.path.starts_with('~') || d.path.starts_with('/') {
                    continue; // MNA007 reports these.
                }
                let Some(doc) = cx.ws.doc(&d.from) else {
                    continue;
                };
                let span = doc
                    .imports
                    .iter()
                    .find(|i| {
                        resolve_relative(&d.from, &i.path).as_deref() == Some(d.path.as_str())
                    })
                    .map(|i| i.span.clone())
                    .unwrap_or(0..0);
                out.push(
                    finding_at(
                        info,
                        doc,
                        span,
                        format!("Claude Code never loads `{}`: {}.", d.path, d.reason),
                    )
                    .with_help(
                        "Import it from a file closer to CLAUDE.md, or inline what matters.",
                    ),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::rules::testutil::{check, check_with};

    // ---- MNA010 ----

    #[test]
    fn diverging_pair_is_flagged() {
        let f = check(
            &CrossFileDivergence,
            &[
                ("AGENTS.md", "- Use pnpm.\n- Run `make test`.\n"),
                ("CLAUDE.md", "- Use pnpm.\n- Prefer small PRs.\n"),
            ],
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("1 rule(s) only in AGENTS.md"));
        assert!(f[0].message.contains("1 rule(s) only in CLAUDE.md"));
    }

    #[test]
    fn importing_pair_and_identical_copy_pass() {
        let f = check(
            &CrossFileDivergence,
            &[
                ("AGENTS.md", "- Use pnpm.\n"),
                ("CLAUDE.md", "@AGENTS.md\n\n- Claude only.\n"),
            ],
        );
        assert!(f.is_empty());
        let f = check(
            &CrossFileDivergence,
            &[
                ("AGENTS.md", "- Use pnpm.\n"),
                ("CLAUDE.md", "- Use pnpm.\n"),
            ],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn single_file_passes() {
        assert!(check(&CrossFileDivergence, &[("AGENTS.md", "- a\n")]).is_empty());
    }

    #[test]
    fn symlinked_pair_on_disk_passes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "- Use pnpm.\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("AGENTS.md", dir.path().join("CLAUDE.md")).unwrap();
        #[cfg(unix)]
        {
            let w = crate::workspace::Workspace::load(dir.path(), &Default::default()).unwrap();
            let c = Config::default();
            let cx = Context::new(&w, &c, &c.targets);
            let mut out = Vec::new();
            CrossFileDivergence.check(&cx, &mut out);
            assert!(out.is_empty(), "{out:?}");
        }
    }

    // ---- MNA011 ----

    fn enabled() -> Config {
        let mut c = Config::default();
        c.rules.enable.push("MNA011".into());
        c
    }

    #[test]
    fn rule_without_rationale_is_flagged() {
        let f = check_with(
            &MissingRationale,
            &[("AGENTS.md", "- Never run migrations by hand.\n")],
            &enabled(),
        );
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn rationale_comment_or_sidecar_satisfies() {
        let f = check_with(
            &MissingRationale,
            &[(
                "AGENTS.md",
                "<!-- why: broke prod twice | retire-when: manual -->\n- Never run migrations by hand.\n\nThe project is a CLI.\n",
            )],
            &enabled(),
        );
        assert!(f.is_empty(), "{f:?}");
        let sidecar = format!(
            "[[rule]]\nhash = \"{}\"\nwhy = \"x\"\n",
            rationale::rule_hash("Never run migrations by hand.")
        );
        let f = check_with(
            &MissingRationale,
            &[
                ("AGENTS.md", "- Never run migrations by hand.\n"),
                (".mnemoa/rationale.toml", sidecar.as_str()),
            ],
            &enabled(),
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn bad_rationale_is_reported() {
        let f = check_with(
            &MissingRationale,
            &[(
                "AGENTS.md",
                "<!-- why: x | retire-when: soon -->\n- Never do X.\n",
            )],
            &enabled(),
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("problems"));
    }

    // ---- MNA013 ----

    #[test]
    fn agents_md_hidden_by_claude_local() {
        let f = check(
            &UnloadedFile,
            &[("AGENTS.md", "- a\n"), ("CLAUDE.local.md", "- mine\n")],
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Warn);
        assert!(f[0].message.contains("claude-code never loads AGENTS.md"));
    }

    #[test]
    fn same_dir_pair_and_imported_agents_are_left_to_mna010() {
        assert!(
            check(
                &UnloadedFile,
                &[("AGENTS.md", "- a\n"), ("CLAUDE.md", "- b\n")]
            )
            .is_empty()
        );
        assert!(
            check(
                &UnloadedFile,
                &[("AGENTS.md", "- a\n"), ("CLAUDE.local.md", "@AGENTS.md\n")]
            )
            .is_empty()
        );
    }

    #[test]
    fn cursor_md_and_codex_override_are_info() {
        let f = check(
            &UnloadedFile,
            &[
                (".cursor/rules/style.md", "- x\n"),
                ("AGENTS.md", "a"),
                ("AGENTS.override.md", "b"),
            ],
        );
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f.iter().all(|f| f.severity == Severity::Info));
    }

    #[test]
    fn untargeted_agents_are_ignored() {
        // Kiro is not a default target.
        assert!(
            check(
                &UnloadedFile,
                &[(".kiro/steering/a.md", "---\ninclusion: fileMatch\n---\n")]
            )
            .is_empty()
        );
        let mut c = Config::default();
        c.targets.push(Target::Kiro);
        let f = check_with(
            &UnloadedFile,
            &[(".kiro/steering/a.md", "---\ninclusion: fileMatch\n---\n")],
            &c,
        );
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn deep_imports_are_flagged() {
        let f = check(
            &UnloadedFile,
            &[
                ("CLAUDE.md", "@a.md"),
                ("a.md", "@b.md"),
                ("b.md", "@c.md"),
                ("c.md", "@d.md"),
                ("d.md", "@e.md"),
                ("e.md", "x"),
            ],
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "d.md");
    }
}
