//! MNA012 promoted-but-present.

use super::{Check, Context, RULES, RuleInfo, finding_at};
use crate::diag::Finding;
use crate::ledger::{LEDGER_FILE, Ledger, State};
use crate::rationale::rule_hash;

/// MNA012.
pub struct PromotedButPresent;

impl Check for PromotedButPresent {
    fn info(&self) -> &'static RuleInfo {
        &RULES[11]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        let Some(ledger) = cx
            .ws
            .read_file(LEDGER_FILE)
            .and_then(|t| Ledger::parse(&t).ok())
        else {
            return;
        };
        for doc in cx.always_docs() {
            for rule in &doc.rules {
                let Some(e) = ledger.by_hash(&rule_hash(&rule.text)) else {
                    continue;
                };
                if e.state != State::Applied {
                    continue;
                }
                out.push(
                    finding_at(
                        self.info(),
                        doc,
                        rule.span.clone(),
                        format!("Promoted to check `{}` ({}) but the prose is still loaded.", e.id, e.enforcer),
                    )
                    .with_help("Delete the line; the check enforces it. `mnemoa forget --apply` does this."),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::testutil::check;

    fn ledger(state: &str) -> String {
        format!(
            "[[check]]\nid = \"indentation\"\nhash = \"{}\"\ntext = \"Use 2 spaces.\"\nsource = \"AGENTS.md\"\nenforcer = \"editorconfig\"\ndate = \"2026-01-01\"\nstate = \"{state}\"\n",
            rule_hash("Use 2 spaces.")
        )
    }

    #[test]
    fn applied_promotion_still_present_is_flagged() {
        let l = ledger("applied");
        let f = check(
            &PromotedButPresent,
            &[
                (LEDGER_FILE, l.as_str()),
                ("AGENTS.md", "- Use 2 spaces.\n"),
            ],
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("indentation"));
    }

    #[test]
    fn proposed_or_removed_or_on_demand_pass() {
        let p = ledger("proposed");
        assert!(
            check(
                &PromotedButPresent,
                &[
                    (LEDGER_FILE, p.as_str()),
                    ("AGENTS.md", "- Use 2 spaces.\n")
                ]
            )
            .is_empty()
        );
        let a = ledger("applied");
        assert!(
            check(
                &PromotedButPresent,
                &[(LEDGER_FILE, a.as_str()), ("AGENTS.md", "- Use tabs.\n")]
            )
            .is_empty()
        );
        assert!(
            check(
                &PromotedButPresent,
                &[
                    (LEDGER_FILE, a.as_str()),
                    ("AGENTS.md", "x\n"),
                    (
                        ".claude/rules/s.md",
                        "---\npaths:\n  - a/**\n---\n- Use 2 spaces.\n"
                    )
                ]
            )
            .is_empty()
        );
    }
}
