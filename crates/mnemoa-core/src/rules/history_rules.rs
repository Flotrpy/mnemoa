//! MNA005 init-fossilization.

use super::{Check, Context, RULES, RuleInfo, finding_file};
use crate::diag::Finding;

/// MNA005.
pub struct InitFossilization;

impl Check for InitFossilization {
    fn info(&self) -> &'static RuleInfo {
        &RULES[4]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        let Some(h) = cx.history else { return };
        if h.is_shallow() {
            return;
        }
        let threshold = cx.config.rules.fossil_commits;
        for doc in cx.always_docs() {
            // A file that only imports others (`@AGENTS.md`) has nothing of its own to go stale.
            let import_only = doc.text.lines().all(|l| {
                let t = l.trim();
                t.is_empty() || (t.starts_with('@') && !t.contains(' '))
            });
            if import_only {
                continue;
            }
            let touching = h.touching(&doc.path);
            let [only] = touching.as_slice() else {
                continue;
            };
            let later = h.code_commits_since(*only);
            if later >= threshold {
                let commit = &h.commits()[*only];
                out.push(
                    finding_file(
                        self.info(),
                        &doc.path,
                        format!(
                            "{} was committed once ({}) and never updated, while {later} later commits changed code.",
                            doc.path,
                            &commit.id[..commit.id.len().min(8)]
                        ),
                    )
                    .with_snippet("fossil")
                    .with_help("Review it against the current code: delete what no longer holds, and keep only what the agent cannot discover."),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::history::{History, MemoryHistory};
    use crate::rules::Context;
    use crate::workspace::tests::ws;

    fn run(h: &dyn History, threshold: usize) -> Vec<Finding> {
        let w = ws(&[("AGENTS.md", "- x\n"), ("src/a.rs", "")]);
        let mut c = Config::default();
        c.rules.fossil_commits = threshold;
        let cx = Context::new(&w, &c, &c.targets).with_history(Some(h));
        let mut out = Vec::new();
        InitFossilization.check(&cx, &mut out);
        out
    }

    fn history(code_commits: usize, agents_updates: usize) -> MemoryHistory {
        let mut commits: Vec<(String, Vec<&str>)> =
            vec![("init".into(), vec!["AGENTS.md", "src/a.rs"])];
        for i in 0..code_commits {
            commits.push((format!("c{i}"), vec!["src/a.rs"]));
        }
        for i in 0..agents_updates {
            commits.push((format!("d{i}"), vec!["AGENTS.md"]));
        }
        let refs: Vec<(&str, &[&str])> = commits
            .iter()
            .map(|(id, p)| (id.as_str(), p.as_slice()))
            .collect();
        MemoryHistory::linear(&refs)
    }

    #[test]
    fn fossil_is_flagged() {
        let f = run(&history(30, 0), 25);
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("30 later commits"));
    }

    #[test]
    fn import_only_file_is_not_fossil() {
        let w = ws(&[
            ("CLAUDE.md", "@AGENTS.md\n"),
            ("AGENTS.md", "- x\n"),
            ("src/a.rs", ""),
        ]);
        let mut commits: Vec<(String, Vec<&str>)> =
            vec![("init".into(), vec!["CLAUDE.md", "AGENTS.md", "src/a.rs"])];
        for i in 0..30 {
            commits.push((format!("c{i}"), vec!["src/a.rs"]));
        }
        commits.push(("d".into(), vec!["AGENTS.md"]));
        let refs: Vec<(&str, &[&str])> = commits
            .iter()
            .map(|(id, p)| (id.as_str(), p.as_slice()))
            .collect();
        let h = MemoryHistory::linear(&refs);
        let c = Config::default();
        let cx = Context::new(&w, &c, &c.targets).with_history(Some(&h));
        let mut out = Vec::new();
        InitFossilization.check(&cx, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn updated_file_is_not_fossil() {
        assert!(run(&history(30, 1), 25).is_empty());
    }

    #[test]
    fn below_threshold_and_doc_only_commits_pass() {
        assert!(run(&history(10, 0), 25).is_empty());
        let docs_only = MemoryHistory::linear(&[
            ("init", &["AGENTS.md"]),
            ("a", &["README.md"]),
            ("b", &["docs/x.md"]),
        ]);
        assert!(run(&docs_only, 1).is_empty());
    }

    #[test]
    fn shallow_or_missing_history_is_silent() {
        let mut h = history(30, 0);
        h.shallow = true;
        assert!(run(&h, 25).is_empty());
        let w = ws(&[("AGENTS.md", "x")]);
        let c = Config::default();
        let cx = Context::new(&w, &c, &c.targets);
        let mut out = Vec::new();
        InitFossilization.check(&cx, &mut out);
        assert!(out.is_empty());
    }
}
