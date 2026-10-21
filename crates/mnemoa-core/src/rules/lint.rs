//! MNA002 lint-leakage.

use super::{Check, Context, RULES, RuleInfo, finding_at};
use crate::catalog::Catalog;
use crate::diag::Finding;
use crate::enforcers;

/// MNA002.
pub struct LintLeakage;

impl Check for LintLeakage {
    fn info(&self) -> &'static RuleInfo {
        &RULES[1]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        let configured = enforcers::detect(cx.ws);
        if configured.is_empty() {
            return;
        }
        let catalog = Catalog::builtin();
        for doc in cx.always_docs() {
            for rule in doc.rules.iter().filter(|r| r.imperative) {
                for topic in catalog.matches(&rule.text) {
                    let by = topic.covered_by(&rule.text, &configured);
                    let Some(first) = by.first() else { continue };
                    let names: Vec<&str> = by.iter().map(|e| e.name.as_str()).collect();
                    out.push(
                        finding_at(
                            self.info(),
                            doc,
                            rule.span.clone(),
                            format!(
                                "Restates what {} already enforces ({}; configured in {}).",
                                names.join(" and "),
                                topic.summary,
                                first.evidence.join(", ")
                            ),
                        )
                        .with_help("Delete the line, or run `mnemoa promote` to record the check in the ledger."),
                    );
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::testutil::check;

    #[test]
    fn indentation_rule_with_editorconfig_is_flagged() {
        let f = check(
            &LintLeakage,
            &[
                (
                    ".editorconfig",
                    "[*]\nindent_style = space\nindent_size = 2\n",
                ),
                ("AGENTS.md", "- Use 2 spaces for indentation.\n"),
            ],
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("EditorConfig"));
    }

    #[test]
    fn same_rule_without_enforcer_is_silent() {
        let f = check(
            &LintLeakage,
            &[("AGENTS.md", "- Use 2 spaces for indentation.\n")],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn editorconfig_without_indent_keys_is_silent() {
        let f = check(
            &LintLeakage,
            &[
                (".editorconfig", "[*]\ncharset = utf-8\n"),
                ("AGENTS.md", "- Use 2 spaces for indentation.\n"),
            ],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn unwrap_needs_the_clippy_lint() {
        let ci = (
            ".github/workflows/ci.yml",
            "run: cargo clippy -- -D warnings\n",
        );
        let no_lint = check(
            &LintLeakage,
            &[
                ci,
                ("Cargo.toml", "[package]\n"),
                ("AGENTS.md", "- Never use `.unwrap()` in `src/`.\n"),
            ],
        );
        assert!(no_lint.is_empty());
        let with_lint = check(
            &LintLeakage,
            &[
                ci,
                ("Cargo.toml", "[lints.clippy]\nunwrap_used = \"deny\"\n"),
                ("AGENTS.md", "- Never use `.unwrap()` in `src/`.\n"),
            ],
        );
        assert_eq!(with_lint.len(), 1);
    }

    #[test]
    fn non_imperative_and_on_demand_text_is_silent() {
        let f = check(
            &LintLeakage,
            &[
                (".prettierrc", "{}"),
                ("AGENTS.md", "The old code used double quotes everywhere.\n"),
                (
                    ".claude/rules/js.md",
                    "---\npaths:\n  - \"**/*.js\"\n---\n- Use single quotes.\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn named_formatter_rule() {
        let f = check(
            &LintLeakage,
            &[
                (".prettierrc", "{}"),
                (
                    "AGENTS.md",
                    "- Always format code with Prettier.\n- Always format Python with Black.\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].snippet.contains("Prettier"));
    }
}
