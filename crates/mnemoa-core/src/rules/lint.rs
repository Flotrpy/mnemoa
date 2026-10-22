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
                // Topics that only say which tool to run ("format with Prettier") are not leaks.
                let headings = doc.section_path(rule.section);
                for topic in catalog
                    .matches_in(&rule.text, &headings)
                    .into_iter()
                    .filter(|t| t.restates)
                {
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
    fn naming_the_formatter_to_run_is_not_a_leak() {
        // cal.com "Use Biome for formatting and linting"; airflow "format ... with ruff";
        // rust-lang/rust "use ./x fmt; do not invoke rustfmt directly".
        let f = check(
            &LintLeakage,
            &[
                (".prettierrc", "{}"),
                (
                    "AGENTS.md",
                    "- Always format code with Prettier.\n- Use Biome for formatting and linting.\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    // Near misses below come from findings labeled wrong in notebooks/results/labels.csv.

    #[test]
    fn commit_message_length_is_not_code_line_length() {
        // PostHog AGENTS.md "Commits and Pull Requests > Rules".
        let f = check(
            &LintLeakage,
            &[
                (".editorconfig", "[*]\nmax_line_length = 120\n"),
                (
                    "AGENTS.md",
                    "## Commits and Pull Requests\n\n### Rules\n\n- Keep the first line under 72 characters\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn code_line_length_rule_is_still_flagged() {
        let f = check(
            &LintLeakage,
            &[
                (".editorconfig", "[*]\nmax_line_length = 120\n"),
                (
                    "AGENTS.md",
                    "## Style\n\n- Keep lines under 120 characters.\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn warn_only_lint_does_not_enforce() {
        // cal.com: biome.json sets noExplicitAny to warn.
        let f = check(
            &LintLeakage,
            &[
                (
                    "biome.json",
                    r#"{"linter":{"rules":{"suspicious":{"noExplicitAny":"warn"}}}}"#,
                ),
                ("AGENTS.md", "- Never use the any type.\n"),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn scope_rules_about_dead_code_are_not_the_unused_lint() {
        // withastro/astro AGENTS.md.
        let f = check(
            &LintLeakage,
            &[
                (
                    "eslint.config.js",
                    "export default [{ rules: { 'no-unused-vars': 'error' } }];",
                ),
                (
                    "AGENTS.md",
                    "- If you notice unrelated dead code, mention it - don't delete it.\n- Don't remove pre-existing dead code unless asked.\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }
}
