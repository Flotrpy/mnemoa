//! Always-loaded context cost per target agent.

use serde::Serialize;

use crate::discover::{Loading, Target};
use crate::tokens::Tokenizer;
use crate::workspace::{CODEX_MAX_BYTES, Workspace};

/// Cost of one file as a target sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileCost {
    pub path: String,
    pub lines: usize,
    pub bytes: usize,
    pub tokens: usize,
    /// Set when the file arrives through an `@import`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

/// A file a target loads later, and what makes it load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OnDemandCost {
    pub path: String,
    pub trigger: String,
    pub tokens: usize,
}

/// Budget for one target.
#[derive(Debug, Clone, Serialize)]
pub struct TargetBudget {
    pub target: Target,
    pub always: Vec<FileCost>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub total_tokens: usize,
    pub on_demand: Vec<OnDemandCost>,
    /// Bytes past the target's hard cap that the target drops (Codex only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_bytes: Option<usize>,
    pub notes: Vec<String>,
}

/// Budget for several targets.
#[derive(Debug, Clone, Serialize)]
pub struct BudgetReport {
    pub tokenizer: String,
    pub targets: Vec<TargetBudget>,
}

impl BudgetReport {
    pub fn get(&self, target: Target) -> Option<&TargetBudget> {
        self.targets.iter().find(|t| t.target == target)
    }
}

/// Cost of a document as `target` sees it (front matter removed; block comments removed when
/// the target strips them).
pub fn file_cost(ws: &Workspace, path: &str, target: Target, tok: Tokenizer) -> Option<FileCost> {
    let doc = ws.doc(path)?;
    let text = doc.loaded_text(target.strips_html_comments());
    Some(FileCost {
        path: path.to_string(),
        lines: text.lines().count(),
        bytes: text.len(),
        tokens: tok.count(&text),
        via: None,
    })
}

/// Compute the budget for each target.
pub fn budget(ws: &Workspace, targets: &[Target], tok: Tokenizer) -> BudgetReport {
    let targets = targets
        .iter()
        .map(|&target| {
            let always: Vec<FileCost> = ws
                .always_loaded(target)
                .into_iter()
                .filter_map(|f| {
                    let mut c = file_cost(ws, &f.path, target, tok)?;
                    c.via = f.via;
                    Some(c)
                })
                .collect();
            let on_demand = ws
                .discovery
                .surfaces
                .iter()
                .filter_map(|s| match s.loading_for(target) {
                    Some(Loading::OnDemand { trigger }) => Some(OnDemandCost {
                        path: s.path.clone(),
                        trigger: trigger.clone(),
                        tokens: file_cost(ws, &s.path, target, tok).map_or(0, |c| c.tokens),
                    }),
                    _ => None,
                })
                .collect();
            let total_bytes = always.iter().map(|c| c.bytes).sum();
            let mut notes = Vec::new();
            let dropped_bytes = if target == Target::Codex && total_bytes > CODEX_MAX_BYTES {
                notes.push(format!(
                    "Codex stops adding guidance after {CODEX_MAX_BYTES} bytes (project_doc_max_bytes)"
                ));
                Some(total_bytes - CODEX_MAX_BYTES)
            } else {
                None
            };
            match target {
                Target::Kiro => notes.push(
                    "Kiro CLI handling of inclusion modes is unverified; on-demand steering may load at start"
                        .to_string(),
                ),
                Target::Copilot => notes.push(
                    "Copilot file layout is modeled from common usage and is unverified".to_string(),
                ),
                Target::Codex => notes.push(
                    "Codex is assumed not to strip HTML comments (unverified)".to_string(),
                ),
                _ => {}
            }
            TargetBudget {
                target,
                total_lines: always.iter().map(|c| c.lines).sum(),
                total_tokens: always.iter().map(|c| c.tokens).sum(),
                total_bytes,
                always,
                on_demand,
                dropped_bytes,
                notes,
            }
        })
        .collect();
    BudgetReport {
        tokenizer: tok.label().to_string(),
        targets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::ws;

    #[test]
    fn claude_cost_excludes_comments_but_codex_does_not() {
        let w = ws(&[(
            "AGENTS.md",
            "<!-- why: a very long rationale comment here -->\n- Use pnpm.\n",
        )]);
        let r = budget(
            &w,
            &[Target::ClaudeCode, Target::Codex],
            Tokenizer::Heuristic,
        );
        let claude = r.get(Target::ClaudeCode).unwrap();
        let codex = r.get(Target::Codex).unwrap();
        assert_eq!(claude.always.len(), 1);
        assert!(claude.total_bytes < codex.total_bytes);
        assert!(r.tokenizer.contains("estimate"));
    }

    #[test]
    fn imports_are_counted_once_with_via() {
        let w = ws(&[("CLAUDE.md", "@AGENTS.md\n"), ("AGENTS.md", "- a\n- b\n")]);
        let r = budget(&w, &[Target::ClaudeCode], Tokenizer::Heuristic);
        let b = r.get(Target::ClaudeCode).unwrap();
        assert_eq!(b.always.len(), 2);
        assert_eq!(b.always[1].via.as_deref(), Some("CLAUDE.md"));
        assert_eq!(b.total_lines, 3);
    }

    #[test]
    fn codex_byte_cap_reports_dropped_bytes() {
        let big = "- rule line that is long enough\n".repeat(1200);
        let w = ws(&[("AGENTS.md", big.as_str())]);
        let r = budget(&w, &[Target::Codex], Tokenizer::Heuristic);
        let b = r.get(Target::Codex).unwrap();
        assert_eq!(b.dropped_bytes, Some(big.len() - CODEX_MAX_BYTES));
    }

    #[test]
    fn on_demand_files_are_listed_with_trigger() {
        let w = ws(&[
            ("CLAUDE.md", "x"),
            (
                ".claude/rules/api.md",
                "---\npaths:\n  - src/api/**\n---\n- y\n",
            ),
        ]);
        let r = budget(&w, &[Target::ClaudeCode], Tokenizer::Heuristic);
        let b = r.get(Target::ClaudeCode).unwrap();
        assert_eq!(b.on_demand.len(), 1);
        assert!(b.on_demand[0].trigger.contains("src/api/**"));
    }
}
