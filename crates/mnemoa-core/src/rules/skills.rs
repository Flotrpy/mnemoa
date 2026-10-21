//! MNA003 skill-leakage.

use std::sync::LazyLock;

use regex::Regex;

use super::{Check, Context, RULES, RuleInfo, finding_at};
use crate::diag::Finding;
use crate::discover::Target;
use crate::doc::Doc;

static TASK_LEAD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(when|before|after|to|how\s+to|steps\s+to|if\s+you(\s+need\s+to)?|for)\s+(you\s+)?(add(ing)?|creat(e|ing)|releas(e|ing)|deploy(ing)?|publish(ing)?|migrat(e|ing)|bump(ing)?|upgrad(e|ing)|set(ting)?\s+up|onboard(ing)?|writ(e|ing)\s+a\s+new|introduc(e|ing)|regenerat(e|ing)|rotat(e|ing)|cut(ting)?\s+a)\b",
    )
    .unwrap()
});
static TASK_HEADING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(how\s+to|adding|creating|release(s|\s+process)?|releasing|deploy(ment|ing)?|migrations?|publishing|onboarding|new\s+\w+|runbook|playbook|procedure|checklist)\b",
    )
    .unwrap()
});

/// Ordered lists at least this long are flagged even without a task lead-in.
const LONG_PROCEDURE: usize = 8;

/// MNA003.
pub struct SkillLeakage;

impl Check for SkillLeakage {
    fn info(&self) -> &'static RuleInfo {
        &RULES[2]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        for doc in cx.always_docs() {
            let targets = cx.loaded_by(&doc.path);
            check_doc(self.info(), doc, &targets, out);
        }
    }
}

fn lead_in(doc: &Doc, list_start: usize, section: Option<usize>) -> String {
    let mut s = doc.section_title(section).unwrap_or("").to_string();
    if let Some(p) = doc.paragraphs.iter().rfind(|p| {
        p.span.end <= list_start && !doc.lists.iter().any(|l| l.span.contains(&p.span.start))
    }) {
        let gap = &doc.text[p.span.end..list_start];
        if gap.trim().is_empty() {
            s.push_str(" | ");
            s.push_str(&p.text);
        }
    }
    s
}

/// Where each target keeps on-demand instructions.
pub fn on_demand_home(targets: &[Target]) -> String {
    let mut homes: Vec<&str> = targets
        .iter()
        .map(|t| match t {
            Target::ClaudeCode => "`.claude/rules/<topic>.md` with `paths`",
            Target::Kiro => "`.kiro/steering/<topic>.md` with `inclusion: manual`",
            Target::Cursor => "`.cursor/rules/<topic>.mdc` with `globs`",
            Target::Copilot => "`.github/instructions/<topic>.instructions.md` with `applyTo`",
            _ => "`docs/agents/<topic>.md` with a one-line pointer",
        })
        .collect();
    homes.sort();
    homes.dedup();
    homes.join(", or ")
}

fn check_doc(info: &RuleInfo, doc: &Doc, targets: &[Target], out: &mut Vec<Finding>) {
    for list in doc.lists.iter().filter(|l| l.ordered && l.depth == 0) {
        let steps = list.items.len();
        let lead = lead_in(doc, list.span.start, list.section);
        let task = TASK_LEAD_RE.is_match(&lead)
            || TASK_HEADING_RE.is_match(doc.section_title(list.section).unwrap_or(""));
        if (task && steps >= 3) || steps >= LONG_PROCEDURE {
            let start = list
                .section
                .and_then(|i| doc.headings.get(i))
                .filter(|_| task)
                .map(|h| h.span.start)
                .unwrap_or(list.span.start);
            let what = doc
                .section_title(list.section)
                .filter(|_| task)
                .map(|t| format!("\"{t}\""))
                .unwrap_or_else(|| "this procedure".to_string());
            out.push(
                finding_at(info, doc, start..list.span.end, format!("A {steps}-step procedure ({what}) is loaded on every task, but only some tasks need it."))
                    .with_help(format!("Move it to {}.", on_demand_home(targets))),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::testutil::check;

    #[test]
    fn task_procedure_is_flagged() {
        let f = check(
            &SkillLeakage,
            &[(
                "AGENTS.md",
                "# A\n\n## Adding a new endpoint\n\n1. Add the route.\n2. Add the handler.\n3. Add a test.\n4. Update the OpenAPI file.\n",
            )],
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].line, 3);
        assert!(f[0].help.as_deref().unwrap().contains(".claude/rules/"));
    }

    #[test]
    fn lead_in_paragraph_counts() {
        let f = check(
            &SkillLeakage,
            &[(
                "AGENTS.md",
                "When releasing a version:\n\n1. Bump.\n2. Tag.\n3. Push.\n",
            )],
        );
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn short_or_unordered_lists_pass() {
        let f = check(
            &SkillLeakage,
            &[(
                "AGENTS.md",
                "## Adding a new endpoint\n\n1. Add the route.\n2. Add a test.\n\n## Rules\n\n- a\n- b\n- c\n- d\n- e\n- f\n- g\n- h\n- i\n",
            )],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn general_numbered_list_below_threshold_passes() {
        let f = check(
            &SkillLeakage,
            &[(
                "AGENTS.md",
                "## Principles\n\n1. Small PRs.\n2. Tests first.\n3. No magic.\n",
            )],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn very_long_procedure_is_flagged_without_lead_in() {
        let list: String = (1..=9).map(|i| format!("{i}. step\n")).collect();
        let f = check(&SkillLeakage, &[("AGENTS.md", list.as_str())]);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn on_demand_file_is_not_checked() {
        let f = check(
            &SkillLeakage,
            &[(
                ".claude/rules/release.md",
                "---\npaths:\n  - CHANGELOG.md\n---\n## Releasing\n\n1. a\n2. b\n3. c\n",
            )],
        );
        assert!(f.is_empty());
    }
}
