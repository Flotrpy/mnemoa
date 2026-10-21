//! MNA001 context-bloat and MNA009 discoverable-content.

use std::collections::BTreeSet;

use super::{Check, Context, RULES, RuleInfo, finding_at, finding_file};
use crate::budget;
use crate::diag::Finding;
use crate::discover::Target;
use crate::doc::Doc;
use crate::tokens::Tokenizer;
use crate::workspace::CODEX_MAX_BYTES;

/// MNA001.
pub struct ContextBloat;

impl Check for ContextBloat {
    fn info(&self) -> &'static RuleInfo {
        &RULES[0]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        let info = self.info();
        let max_lines = cx.config.rules.max_lines;
        for doc in cx.always_docs() {
            let lines = doc.line_count();
            if max_lines > 0 && lines >= max_lines {
                let line = max_lines.min(lines).max(1);
                let start = doc.lines.line_start(line);
                out.push(
                    finding_at(
                        info,
                        doc,
                        start..start,
                        format!("{} has {lines} lines; always-loaded files should stay under {max_lines}.", doc.path),
                    )
                    .with_help("Run `mnemoa forget` to see what can be deleted, promoted or moved on demand."),
                );
            }
        }

        let tok = Tokenizer::best();
        let report = budget::budget(cx.ws, cx.targets, tok);
        let limit = cx.config.rules.token_budget;
        for b in &report.targets {
            if limit > 0 && b.total_tokens > limit {
                let Some(biggest) = b.always.iter().max_by_key(|c| c.tokens) else {
                    continue;
                };
                let parts: Vec<String> = b
                    .always
                    .iter()
                    .map(|c| format!("{} {}", c.path, c.tokens))
                    .collect();
                out.push(
                    finding_file(
                        info,
                        &biggest.path,
                        format!(
                            "{} loads about {} tokens at start ({}), over the budget of {limit}: {}.",
                            b.target,
                            b.total_tokens,
                            report.tokenizer,
                            parts.join(", ")
                        ),
                    )
                    .with_snippet(format!("budget:{}", b.target)),
                );
            }
            if b.target == Target::Codex
                && let Some(dropped) = b.dropped_bytes
            {
                // Point at the file where the running total crosses the cap.
                let mut total = 0;
                let crossing = b
                    .always
                    .iter()
                    .find(|c| {
                        total += c.bytes;
                        total > CODEX_MAX_BYTES
                    })
                    .map(|c| c.path.clone())
                    .unwrap_or_default();
                out.push(
                    finding_file(
                        info,
                        &crossing,
                        format!(
                            "Codex stops adding guidance after {CODEX_MAX_BYTES} bytes; about {dropped} bytes from `{crossing}` onward never reach it."
                        ),
                    )
                    .with_snippet("codex-byte-cap"),
                );
            }
        }
    }
}

/// MNA009.
pub struct DiscoverableContent;

impl Check for DiscoverableContent {
    fn info(&self) -> &'static RuleInfo {
        &RULES[8]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        let names = basenames(&cx.ws.discovery.files, &cx.ws.discovery.dirs);
        let facts = cx.facts();
        let mut stack: BTreeSet<String> = facts.deps.iter().map(|d| normalize(d)).collect();
        let disc = &cx.ws.discovery;
        let has_any = |names: &[&str]| {
            names.iter().any(|n| {
                disc.files
                    .iter()
                    .any(|f| f == n || f.ends_with(&format!("/{n}")))
            })
        };
        for (manifest, langs) in [
            (&["Cargo.toml"][..], &["rust", "cargo"][..]),
            (
                &["package.json"][..],
                &["node", "nodejs", "javascript", "npm"][..],
            ),
            (&["tsconfig.json"][..], &["typescript", "ts"][..]),
            (
                &["pyproject.toml", "setup.py", "requirements.txt"][..],
                &["python"][..],
            ),
            (&["go.mod"][..], &["go", "golang"][..]),
            (&["Gemfile"][..], &["ruby"][..]),
            (
                &["Dockerfile", "docker-compose.yml", "compose.yaml"][..],
                &["docker"][..],
            ),
        ] {
            if has_any(manifest) {
                stack.extend(langs.iter().map(|s| s.to_string()));
            }
        }

        for doc in cx.always_docs() {
            for b in &doc.code_blocks {
                if let Some((found, total)) = tree_entries(&b.code, &names)
                    && total >= 3
                    && found * 10 >= total * 6
                {
                    out.push(
                        finding_at(
                            self.info(),
                            doc,
                            b.span.clone(),
                            format!("Directory tree repeats the repo layout ({found} of {total} entries exist); the agent can list the tree itself."),
                        )
                        .with_help("Delete it, or keep one line per directory that says what is non-obvious about it."),
                    );
                }
            }
            check_stack_lists(self.info(), doc, &stack, out);
        }
    }
}

fn basenames(files: &[String], dirs: &[String]) -> BTreeSet<String> {
    files
        .iter()
        .chain(dirs.iter())
        .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
        .collect()
}

/// Count tree entries that exist. `None` when the block does not look like a tree.
fn tree_entries(code: &str, names: &BTreeSet<String>) -> Option<(usize, usize)> {
    let tree_lines = code
        .lines()
        .filter(|l| {
            l.contains("├")
                || l.contains("└")
                || l.contains("│")
                || l.contains("|--")
                || l.contains("+--")
                || l.contains("`--")
        })
        .count();
    if tree_lines < 3 {
        return None;
    }
    let mut found = 0;
    let mut total = 0;
    for line in code.lines() {
        let entry = line
            .trim_start_matches(|c: char| c.is_whitespace() || "├└│─|`+-".contains(c))
            .split(['#', '←', '<'])
            .next()
            .unwrap_or("")
            .split("--")
            .next()
            .unwrap_or("")
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        if entry.is_empty() || entry == "." || entry.contains("...") || entry.contains('*') {
            continue;
        }
        total += 1;
        if names.contains(entry) {
            found += 1;
        }
    }
    Some((found, total))
}

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn check_stack_lists(info: &RuleInfo, doc: &Doc, stack: &BTreeSet<String>, out: &mut Vec<Finding>) {
    for list in &doc.lists {
        if list.ordered || list.items.len() < 3 {
            continue;
        }
        let items: Vec<&crate::doc::Rule> = doc
            .rules
            .iter()
            .filter(|r| list.items.iter().any(|i| i.start == r.span.start))
            .collect();
        if items.len() < 3 {
            continue;
        }
        let mut matched = 0;
        for item in &items {
            let words: Vec<&str> = item.text.split_whitespace().collect();
            if words.len() > 6 || item.imperative {
                continue;
            }
            let label = item
                .text
                .split([':', '(', '—', '-', ','])
                .next()
                .unwrap_or("");
            let cleaned: Vec<String> = label
                .split_whitespace()
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '+'))
                .filter(|w| {
                    !w.is_empty()
                        && !w.chars().next().is_some_and(|c| c.is_ascii_digit())
                        && !w.starts_with('v')
                })
                .map(normalize)
                .collect();
            let joined: String = cleaned.concat();
            let hit = stack.contains(&joined)
                || cleaned.iter().any(|w| stack.contains(w))
                || stack.contains(&format!("{joined}js"));
            if hit {
                matched += 1;
            }
        }
        if matched >= 3 && matched * 4 >= items.len() * 3 {
            out.push(
                finding_at(
                    info,
                    doc,
                    list.span.clone(),
                    format!(
                        "Stack list repeats what the manifests declare ({matched} of {} items).",
                        items.len()
                    ),
                )
                .with_help("Delete it; keep only versions or choices the manifests do not show."),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::rules::testutil::{check, check_with};

    #[test]
    fn long_file_is_flagged() {
        let body = "- line\n".repeat(205);
        let f = check(&ContextBloat, &[("AGENTS.md", body.as_str())]);
        assert!(f.iter().any(|f| f.message.contains("205 lines")), "{f:?}");
    }

    #[test]
    fn short_file_and_on_demand_long_file_pass() {
        let long = "- line\n".repeat(300);
        let f = check(
            &ContextBloat,
            &[
                ("AGENTS.md", "- short\n"),
                (
                    ".claude/rules/big.md",
                    format!("---\npaths:\n  - src/**\n---\n{long}").as_str(),
                ),
                ("docs/big.md", long.as_str()),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn token_budget_per_target() {
        let mut c = Config::default();
        c.rules.token_budget = 20;
        c.rules.max_lines = 0;
        let f = check_with(
            &ContextBloat,
            &[(
                "AGENTS.md",
                "Always use the same words in every single sentence here please.\n"
                    .repeat(5)
                    .as_str(),
            )],
            &c,
        );
        assert!(f.iter().any(|f| f.message.contains("claude-code loads")));
        assert!(f.iter().any(|f| f.message.contains("codex loads")));
        c.rules.token_budget = 100_000;
        let f = check_with(&ContextBloat, &[("AGENTS.md", "short\n")], &c);
        assert!(f.is_empty());
    }

    #[test]
    fn codex_cap() {
        let mut c = Config::default();
        c.rules.max_lines = 0;
        c.rules.token_budget = 0;
        let big = "x".repeat(CODEX_MAX_BYTES + 10);
        let f = check_with(&ContextBloat, &[("AGENTS.md", big.as_str())], &c);
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("Codex stops"));
    }

    const TREE: &str =
        "```\n.\n├── src/\n│   ├── main.rs\n│   └── lib.rs\n├── tests/\n└── Cargo.toml\n```\n";

    #[test]
    fn tree_that_matches_repo_is_flagged() {
        let f = check(
            &DiscoverableContent,
            &[
                ("AGENTS.md", TREE),
                ("src/main.rs", ""),
                ("src/lib.rs", ""),
                ("tests/a.rs", ""),
                ("Cargo.toml", ""),
            ],
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "MNA009");
    }

    #[test]
    fn tree_of_planned_layout_and_code_blocks_pass() {
        let f = check(
            &DiscoverableContent,
            &[
                (
                    "AGENTS.md",
                    "```\n├── future/\n│   ├── a.rs\n└── b.rs\n```\n\n```rust\nfn main() {}\n```\n",
                ),
                ("src/main.rs", ""),
            ],
        );
        assert!(f.is_empty());
        // Only two tree lines is not a tree.
        let f = check(
            &DiscoverableContent,
            &[
                ("AGENTS.md", "```\n├── src/\n└── tests/\n```\n"),
                ("src/a", ""),
                ("tests/b", ""),
            ],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn stack_list_matching_manifests_is_flagged() {
        let f = check(
            &DiscoverableContent,
            &[
                (
                    "AGENTS.md",
                    "## Stack\n\n- TypeScript\n- React 18\n- Vite\n- Vitest\n",
                ),
                (
                    "package.json",
                    r#"{"dependencies":{"react":"18"},"devDependencies":{"vite":"5","vitest":"1","typescript":"5"}}"#,
                ),
                ("tsconfig.json", "{}"),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn stack_list_near_misses() {
        let f = check(
            &DiscoverableContent,
            &[
                (
                    "AGENTS.md",
                    "- Use React hooks, never class components.\n- Prefer Vite plugins over custom scripts.\n- Run vitest in watch mode.\n\n- Postgres\n- Redis\n- Kafka\n",
                ),
                (
                    "package.json",
                    r#"{"dependencies":{"react":"18"},"devDependencies":{"vite":"5","vitest":"1"}}"#,
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }
}
