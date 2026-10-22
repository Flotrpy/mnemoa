//! MNA004 conflicting-instructions (deterministic subset only).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use super::{Check, Context, RULES, RuleInfo, finding_at};
use crate::diag::Finding;
use crate::doc::{Doc, Polarity, Rule};
use crate::rules::commands::commands;

/// Tool families where using two members is contradictory.
const FAMILIES: &[(&str, &[&str])] = &[
    (
        "JavaScript package manager",
        &["npm", "pnpm", "yarn", "bun"],
    ),
    ("Python package manager", &["uv", "poetry", "pdm", "pipenv"]),
    ("JavaScript test runner", &["jest", "vitest", "mocha"]),
];

/// Lockfile → the package manager it implies.
const LOCKFILES: &[(&str, &str)] = &[
    ("package-lock.json", "npm"),
    ("pnpm-lock.yaml", "pnpm"),
    ("yarn.lock", "yarn"),
    ("bun.lockb", "bun"),
    ("bun.lock", "bun"),
    ("uv.lock", "uv"),
    ("poetry.lock", "poetry"),
    ("pdm.lock", "pdm"),
];

/// Subcommands that change the project's dependencies, so they show which package manager the
/// project uses. Running scripts (`npm run lint` in a yarn repo) works with any lockfile and
/// says nothing about that choice.
const MANAGER_VERBS: &[&str] = &[
    "install",
    "i",
    "add",
    "ci",
    "remove",
    "rm",
    "uninstall",
    "sync",
    "lock",
    "update",
    "up",
    "upgrade",
];

/// Words that turn a mention into a "don't use this" (`bun install` (never `npm install`)).
static NEGATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(never|not|don't|dont|avoid|instead\s+of|rather\s+than|no\s+longer|deprecated)\b",
    )
    .unwrap()
});

/// Whether a command installs a tool globally (`npm i -g pnpm`), which says nothing about the
/// project's package manager.
fn is_global_install(toks: &[&str]) -> bool {
    toks.iter()
        .any(|t| matches!(*t, "-g" | "--global" | "--location=global"))
        || (toks.first() == Some(&"yarn") && toks.get(1) == Some(&"global"))
}

/// Whether the text before `span` on the same line negates it.
fn negated_on_line(doc: &Doc, span: &std::ops::Range<usize>) -> bool {
    let line_start = doc.text[..span.start].rfind('\n').map_or(0, |i| i + 1);
    NEGATION_RE.is_match(&doc.text[line_start..span.start])
}

static USE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(use|using|prefer|run\s+with|with)\s+`?([a-z][a-z0-9-]*)`?").unwrap()
});
static PLACE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:always\s+)?(?P<verb>put|place|keep|store|add|write|create)?\s*(?:all\s+|new\s+|the\s+)?(?P<concept>[a-z][a-z \-]{2,40}?)\s+(?:(?P<be>go|goes|live|lives|belong|belongs|are|is|should\s+be|must\s+be|are\s+kept|are\s+stored)\s+)?(?:in|under|into|inside)\s+`(?P<path>[^`]+)`").unwrap()
});

const STOP: &[&str] = &[
    "always", "never", "must", "not", "do", "don't", "dont", "should", "the", "a", "an", "to",
    "be", "you", "please", "avoid", "use", "any", "all", "is", "are", "of", "in", "on", "for",
    "and", "or", "it", "this", "that", "with", "no", "only", "ever",
];

/// MNA004.
pub struct ConflictingInstructions;

impl Check for ConflictingInstructions {
    fn info(&self) -> &'static RuleInfo {
        &RULES[3]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        let lockfiles = &cx.facts().lockfiles;
        let mut seen = BTreeSet::new();
        for &target in cx.targets {
            let docs: Vec<&Doc> = cx
                .ws
                .always_loaded(target)
                .iter()
                .filter_map(|f| cx.ws.doc(&f.path))
                .collect();
            if docs.is_empty() {
                continue;
            }
            let mut found = Vec::new();
            tool_conflicts(self.info(), &docs, lockfiles, &mut found);
            polarity_conflicts(self.info(), &docs, &mut found);
            placement_conflicts(self.info(), &docs, &mut found);
            for f in found {
                if seen.insert((f.fingerprint.clone(), f.line)) {
                    out.push(f);
                }
            }
        }
    }
}

struct Use<'a> {
    doc: &'a Doc,
    span: std::ops::Range<usize>,
    tool: &'static str,
}

fn tool_uses<'a>(docs: &[&'a Doc]) -> Vec<Use<'a>> {
    let mut uses = Vec::new();
    let all_tools: Vec<&'static str> = FAMILIES
        .iter()
        .flat_map(|(_, t)| t.iter().copied())
        .collect();
    for doc in docs {
        // Commands such as `pnpm install`.
        for (cmd, span) in commands(doc) {
            let toks: Vec<&str> = cmd
                .split(['&', '|', ';'])
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect();
            let (Some(&prog), second) = (toks.first(), toks.get(1)) else {
                continue;
            };
            if let Some(tool) = all_tools.iter().find(|t| **t == prog) {
                let is_runner = matches!(*tool, "jest" | "vitest" | "mocha");
                let manages = second.is_some_and(|s| MANAGER_VERBS.contains(s));
                if (is_runner || manages)
                    && !is_global_install(&toks)
                    && !negated_on_line(doc, &span)
                {
                    uses.push(Use { doc, span, tool });
                }
            }
        }
        // Prose such as "Use pnpm".
        for r in doc
            .rules
            .iter()
            .filter(|r| r.imperative && r.polarity == Polarity::Do)
        {
            for c in USE_RE.captures_iter(&r.text) {
                // "Install with `npm i -g x`" names a global tool install, not the project's
                // package manager.
                let rest = r.text[c.get(2).map_or(0, |m| m.end())..]
                    .split('`')
                    .next()
                    .unwrap_or("");
                let mut toks = vec![&c[2]];
                toks.extend(rest.split_whitespace());
                if is_global_install(&toks) {
                    continue;
                }
                if let Some(tool) = all_tools.iter().find(|t| **t == c[2].to_lowercase()) {
                    uses.push(Use {
                        doc,
                        span: r.span.clone(),
                        tool,
                    });
                }
            }
        }
    }
    uses
}

fn tool_conflicts(
    info: &RuleInfo,
    docs: &[&Doc],
    lockfiles: &BTreeSet<String>,
    out: &mut Vec<Finding>,
) {
    let uses = tool_uses(docs);
    for (family, members) in FAMILIES {
        let in_family: Vec<&Use> = uses.iter().filter(|u| members.contains(&u.tool)).collect();
        let tools: BTreeSet<&str> = in_family.iter().map(|u| u.tool).collect();
        if tools.len() >= 2 {
            let first = in_family[0];
            for u in in_family.iter().filter(|u| u.tool != first.tool) {
                let (line, _) = first.doc.line_col(first.span.start);
                out.push(
                    finding_at(
                        info,
                        u.doc,
                        u.span.clone(),
                        format!(
                            "Uses {} as the {family}, but {}:{line} uses {}.",
                            u.tool, first.doc.path, first.tool
                        ),
                    )
                    .with_help("Pick one and update the other instruction."),
                );
            }
        }
        // Disagreement with the single lockfile in the repo.
        let locked: Vec<&str> = LOCKFILES
            .iter()
            .filter(|(f, t)| lockfiles.contains(*f) && members.contains(t))
            .map(|(_, t)| *t)
            .collect();
        if let [locked] = locked.as_slice() {
            let lockfile = LOCKFILES
                .iter()
                .find(|(_, t)| t == locked)
                .map(|(f, _)| *f)
                .unwrap_or("");
            for u in in_family.iter().filter(|u| u.tool != *locked) {
                out.push(
                    finding_at(
                        info,
                        u.doc,
                        u.span.clone(),
                        format!(
                            "Uses {}, but the repo's lockfile is `{lockfile}` ({locked}).",
                            u.tool
                        ),
                    )
                    .with_help(format!(
                        "Use {locked}, or remove `{lockfile}` if the project switched."
                    )),
                );
            }
        }
    }
}

fn content_words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .replace("n't", " not")
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
        .map(|w| w.trim_matches('.'))
        .filter(|w| w.len() > 1 && !STOP.contains(w))
        .map(|w| {
            w.strip_suffix('s')
                .filter(|s| s.len() > 2)
                .unwrap_or(w)
                .to_string()
        })
        .collect()
}

fn polarity_conflicts(info: &RuleInfo, docs: &[&Doc], out: &mut Vec<Finding>) {
    let rules: Vec<(&Doc, &Rule, BTreeSet<String>)> = docs
        .iter()
        .flat_map(|d| {
            d.rules
                .iter()
                .filter(|r| r.imperative)
                .map(move |r| (*d, r, content_words(&r.text)))
        })
        .filter(|(_, _, w)| w.len() >= 2)
        .collect();
    for (i, (da, ra, wa)) in rules.iter().enumerate() {
        for (db, rb, wb) in rules.iter().skip(i + 1) {
            if ra.polarity == rb.polarity {
                continue;
            }
            let inter = wa.intersection(wb).count();
            let union = wa.union(wb).count();
            if union > 0 && inter * 10 >= union * 8 {
                let (line, _) = da.line_col(ra.span.start);
                out.push(
                    finding_at(
                        info,
                        db,
                        rb.span.clone(),
                        format!("Contradicts {}:{line} (\"{}\").", da.path, ra.text),
                    )
                    .with_help("Keep one instruction, or say when each applies."),
                );
            }
        }
    }
}

fn placement_conflicts(info: &RuleInfo, docs: &[&Doc], out: &mut Vec<Finding>) {
    let mut by_concept: BTreeMap<String, Vec<(&Doc, &Rule, String)>> = BTreeMap::new();
    for d in docs {
        for r in &d.rules {
            if let Some(c) = PLACE_RE.captures(&r.text) {
                // "Refer to the guidelines in `x`" mentions a path but places nothing.
                if c.name("verb").is_none() && c.name("be").is_none() {
                    continue;
                }
                let concept: String = c["concept"]
                    .split_whitespace()
                    .filter(|w| !matches!(*w, "the" | "all" | "new" | "our" | "your"))
                    .map(|w| w.strip_suffix('s').unwrap_or(w))
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase();
                let path = c["path"]
                    .trim_end_matches('/')
                    .trim_start_matches("./")
                    .to_string();
                if !concept.is_empty() && !path.contains(' ') {
                    by_concept.entry(concept).or_default().push((*d, r, path));
                }
            }
        }
    }
    for (concept, places) in by_concept {
        let paths: BTreeSet<&str> = places.iter().map(|(_, _, p)| p.as_str()).collect();
        if paths.len() < 2 {
            continue;
        }
        let (fd, fr, fp) = &places[0];
        for (d, r, p) in places.iter().filter(|(_, _, p)| p != fp) {
            let (line, _) = fd.line_col(fr.span.start);
            out.push(
                finding_at(
                    info,
                    d,
                    r.span.clone(),
                    format!(
                        "Puts {concept} in `{p}`, but {}:{line} puts them in `{fp}`.",
                        fd.path
                    ),
                )
                .with_help("Keep one location."),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::testutil::check;

    #[test]
    fn npm_and_pnpm_conflict() {
        let f = check(
            &ConflictingInstructions,
            &[(
                "AGENTS.md",
                "- Install with `npm install`.\n- Use pnpm for everything.\n",
            )],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("pnpm") && f[0].message.contains("npm"));
    }

    #[test]
    fn negated_tool_and_single_tool_pass() {
        let f = check(
            &ConflictingInstructions,
            &[(
                "AGENTS.md",
                "- Use pnpm, never npm.\n- Run `pnpm install` then `pnpm test`.\n- `npx tsc` is fine.\n",
            )],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn lockfile_disagreement() {
        let f = check(
            &ConflictingInstructions,
            &[
                ("pnpm-lock.yaml", ""),
                ("AGENTS.md", "Run `yarn install` first.\n"),
            ],
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("pnpm-lock.yaml"));
    }

    #[test]
    fn conflicts_across_files_loaded_together() {
        let f = check(
            &ConflictingInstructions,
            &[
                ("CLAUDE.md", "@AGENTS.md\n- Always commit the lockfile.\n"),
                ("AGENTS.md", "- Never commit the lockfile.\n"),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("Contradicts"));
    }

    #[test]
    fn different_subjects_do_not_conflict() {
        let f = check(
            &ConflictingInstructions,
            &[(
                "AGENTS.md",
                "- Always commit the lockfile.\n- Never commit secrets or tokens.\n- Never edit generated code.\n",
            )],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn files_not_loaded_together_do_not_conflict() {
        let f = check(
            &ConflictingInstructions,
            &[
                ("AGENTS.md", "- Always commit the lockfile.\n"),
                (
                    ".claude/rules/x.md",
                    "---\npaths:\n  - a/**\n---\n- Never commit the lockfile.\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    // Near misses below come from findings labeled wrong in notebooks/results/labels.csv.

    #[test]
    fn global_tool_installs_are_not_a_package_manager_choice() {
        // vercel/ai AGENTS.md, electron, astro.
        let f = check(
            &ConflictingInstructions,
            &[
                ("pnpm-lock.yaml", ""),
                (
                    "AGENTS.md",
                    "- **pnpm**: v12+ (`npm install -g pnpm@12`)\n- `npm i -g @electron/build-tools`\n- `npm install -g agent-browser && agent-browser install`\n- `yarn global add x`\n\n```bash\npnpm install        # Install all dependencies\npnpm build\n```\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn prose_about_a_global_install_is_not_a_choice() {
        // electron .github/copilot-instructions.md.
        let f = check(
            &ConflictingInstructions,
            &[
                ("yarn.lock", ""),
                (
                    "AGENTS.md",
                    "Electron uses `@electron/build-tools` (`e` CLI). Install with `npm i -g @electron/build-tools`.\n- Use npm for dependencies.\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("yarn.lock"), "{f:?}");
        assert_eq!(f[0].line, 2);
    }

    #[test]
    fn running_scripts_is_not_a_package_manager_choice() {
        // electron CLAUDE.md and copilot-instructions with yarn.lock.
        let f = check(
            &ConflictingInstructions,
            &[
                ("yarn.lock", ""),
                (
                    "AGENTS.md",
                    "- Invoke via `yarn <tool>` or `yarn run <tool>`.\n\n```bash\nnpm run lint              # Run all linters\nnpm run test -- -g \"pattern\"\n```\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn never_on_the_same_line_negates_the_command() {
        // cline .clinerules/bun-and-node.md.
        let f = check(
            &ConflictingInstructions,
            &[(
                "AGENTS.md",
                "## Use bun for tooling\n\n- `bun install` (never `npm install` / `npm ci`)\n- `bun run <script>` (never `npm run <script>`)\n",
            )],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn install_commands_with_two_managers_still_conflict() {
        let f = check(
            &ConflictingInstructions,
            &[
                ("pnpm-lock.yaml", ""),
                (
                    "AGENTS.md",
                    "- Run `pnpm install`.\n- Add packages with `yarn add x`.\n",
                ),
            ],
        );
        assert!(!f.is_empty(), "{f:?}");
        assert!(f.iter().all(|x| x.message.contains("yarn")), "{f:?}");
    }

    #[test]
    fn referring_to_a_file_is_not_placing_something() {
        // vercel/next.js AGENTS.md.
        let f = check(
            &ConflictingInstructions,
            &[(
                "AGENTS.md",
                "- Refer to the guidelines in `.github/pull_request_template.md`.\n- Refer to the guidelines in `.github/ISSUE_TEMPLATE/1.bug_report.yml`.\n",
            )],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn same_concept_two_paths() {
        let f = check(
            &ConflictingInstructions,
            &[(
                "AGENTS.md",
                "- Tests live in `tests/unit`.\n- Put tests in `src/__tests__`.\n- Fixtures live in `tests/fixtures`.\n",
            )],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("test"));
    }
}
