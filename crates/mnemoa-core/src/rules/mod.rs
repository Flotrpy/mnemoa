//! Scan rules MNA001–MNA013 and the registry that runs them.

use serde::Serialize;

pub mod commands;
pub mod conflicts;
pub mod history_rules;
pub mod lint;
pub mod loading;
pub mod promoted;
pub mod references;
pub mod size;
pub mod skills;

use crate::config::Config;
use crate::diag::{self, Finding, Severity};
use crate::discover::Target;
use crate::doc::Doc;
use crate::history::History;
use crate::repo::RepoFacts;
use crate::workspace::Workspace;

/// Static description of a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RuleInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// One line: what the rule detects.
    pub summary: &'static str,
    /// Why it matters, in one or two sentences.
    pub rationale: &'static str,
    /// Research backing the rule.
    pub link: &'static str,
    pub default_severity: Severity,
    /// `mnemoa forget` can fix findings of this rule.
    pub fixable: bool,
    /// Off unless listed in `rules.enable`.
    pub opt_in: bool,
    /// Longer explanation for `mnemoa explain`.
    pub explain: &'static str,
}

const SMELLS: &str = "https://arxiv.org/abs/2606.15828";
const CLAUDE_MEMORY: &str = "https://code.claude.com/docs/en/memory";

/// Metadata for every rule, in ID order.
pub static RULES: [RuleInfo; 13] = [
    RuleInfo {
        id: "MNA001",
        name: "context-bloat",
        summary: "An always-loaded file or a target's always-loaded set is over budget.",
        rationale: "Longer context lowers accuracy even on easy tasks, and every always-loaded line is paid on every turn.",
        link: CLAUDE_MEMORY,
        default_severity: Severity::Warn,
        fixable: true,
        opt_in: false,
        explain: "Fires when one always-loaded file has at least `rules.max_lines` lines (default 200, the size Claude Code's docs recommend staying under), when a target's always-loaded set is over `rules.token_budget` estimated tokens (default 4000), or when Codex would drop guidance past its 32 KiB `project_doc_max_bytes` cap. Fix by deleting what the agent can discover, promoting enforceable rules to checks, and moving task-specific sections to on-demand files (`mnemoa forget`).",
    },
    RuleInfo {
        id: "MNA002",
        name: "lint-leakage",
        summary: "A prose rule restates what a configured formatter or linter already enforces.",
        rationale: "The tool already enforces it, so the prose only spends tokens. Lint leakage was the most common smell (62%) in the SCAM 2026 study.",
        link: SMELLS,
        default_severity: Severity::Warn,
        fixable: true,
        opt_in: false,
        explain: "mnemoa first detects which enforcers the repo configures (rustfmt, clippy, EditorConfig, Prettier, ESLint, Biome, Ruff, Black, gofmt, golangci-lint and others), then matches rule text against a catalog of topics each enforcer covers. A rule only fires when the matching enforcer is configured; for opt-in lints such as `clippy::unwrap_used` the lint itself must be enabled. Delete the line, or run `mnemoa promote` to record the check in the ledger.",
    },
    RuleInfo {
        id: "MNA003",
        name: "skill-leakage",
        summary: "A task-specific procedure sits in an always-loaded file.",
        rationale: "Procedures for rare tasks cost context on every task. Move them to a file the agent loads on demand.",
        link: SMELLS,
        default_severity: Severity::Info,
        fixable: true,
        opt_in: false,
        explain: "Fires on a section or paragraph that introduces a task (\"When adding a new ...\", \"To release ...\", \"How to ...\") followed by an ordered list of three or more steps, or on any ordered list of eight or more steps. Move the section to `.claude/rules/` with `paths`, a Kiro steering file with `inclusion: manual`, a Cursor rule with `globs`, or `docs/agents/*.md` with a one-line pointer.",
    },
    RuleInfo {
        id: "MNA004",
        name: "conflicting-instructions",
        summary: "Two instructions contradict each other (deterministic subset).",
        rationale: "When instructions conflict the agent may follow either. Detection of conflicts is hard, so this rule only reports patterns it can check and never above info.",
        link: SMELLS,
        default_severity: Severity::Info,
        fixable: false,
        opt_in: false,
        explain: "Checks three patterns across the always-loaded files of each target: contradictory tool choices (npm and pnpm, pip and uv, jest and vitest, or a tool that disagrees with the lockfile in the repo), always/never pairs on the same subject, and one concept placed in two different paths. Needs review: LLM detectors reached only 57% precision on this smell in the SCAM 2026 study.",
    },
    RuleInfo {
        id: "MNA005",
        name: "init-fossilization",
        summary: "An instruction file was committed once and never updated while the code moved on.",
        rationale: "Files generated once (for example by /init) and never revisited drift from the code they describe.",
        link: SMELLS,
        default_severity: Severity::Warn,
        fixable: false,
        opt_in: false,
        explain: "Fires when an always-loaded instruction file has exactly one commit in history and at least `rules.fossil_commits` (default 25) later commits touched non-Markdown files. Silent in shallow clones and repos without git history.",
    },
    RuleInfo {
        id: "MNA006",
        name: "blind-reference",
        summary: "A link or path with no word on when or why to read it.",
        rationale: "The agent cannot tell whether the target matters for its task, so it either ignores it or reads it every time.",
        link: SMELLS,
        default_severity: Severity::Info,
        fixable: true,
        opt_in: false,
        explain: "Fires on a list item or paragraph that consists of a link or path and nothing else beyond filler words (\"see\", \"docs\", \"more info\"). Add when or why to read it: \"Read docs/api.md before changing handlers.\"",
    },
    RuleInfo {
        id: "MNA007",
        name: "broken-reference",
        summary: "A link, path or @import that does not resolve.",
        rationale: "A broken pointer wastes a tool call at best and misleads the agent at worst.",
        link: SMELLS,
        default_severity: Severity::Error,
        fixable: true,
        opt_in: false,
        explain: "Checks relative Markdown links, Claude Code `@path` imports, and inline-code paths whose top-level directory exists in the repo. URLs, anchors, globs and placeholders are skipped. Imports to `~/` or absolute paths are reported at info level because they depend on the user's machine.",
    },
    RuleInfo {
        id: "MNA008",
        name: "stale-command",
        summary: "A documented command does not exist in the repo's task runners.",
        rationale: "Agents run documented commands verbatim; a renamed script fails and the agent improvises.",
        link: "https://github.com/toshi0607/agentslint",
        default_severity: Severity::Warn,
        fixable: false,
        opt_in: false,
        explain: "Checks commands in code spans and shell code blocks: `make` targets against Makefiles, `just` recipes against justfiles, `npm|pnpm|yarn|bun run` against package.json scripts, `cargo <alias>` against `.cargo/config.toml` aliases, `pdm run`/`hatch run` against pyproject scripts, and `./path` executables against the tree. Silent when a runner file includes others it cannot see.",
    },
    RuleInfo {
        id: "MNA009",
        name: "discoverable-content",
        summary: "A directory tree or stack list that repeats what the tree and manifests already show.",
        rationale: "Overview sections did not help agents find files faster, and they go stale as the tree changes.",
        link: "https://arxiv.org/abs/2602.11988",
        default_severity: Severity::Info,
        fixable: true,
        opt_in: false,
        explain: "Fires on a directory tree whose entries mostly exist in the repo, and on a list of three or more short items that mostly name dependencies from Cargo.toml, package.json or pyproject.toml. Kept at info: GitHub's agents.md guidance still recommends describing structure.",
    },
    RuleInfo {
        id: "MNA010",
        name: "cross-file-divergence",
        summary: "CLAUDE.md and AGENTS.md both exist, neither imports the other, and they differ.",
        rationale: "Claude Code reads CLAUDE.md and skips AGENTS.md when both exist, so rules in only one file reach only some agents.",
        link: CLAUDE_MEMORY,
        default_severity: Severity::Warn,
        fixable: true,
        opt_in: false,
        explain: "Claude Code reads AGENTS.md only when no CLAUDE.md, .claude/CLAUDE.md or CLAUDE.local.md exists at or above the working directory. With both files present and no `@AGENTS.md` import or symlink, Claude Code never sees AGENTS.md. The finding lists how many rules each file has that the other lacks. Fix: make CLAUDE.md `@AGENTS.md` plus Claude-only additions.",
    },
    RuleInfo {
        id: "MNA011",
        name: "missing-rationale",
        summary: "A rule has no rationale comment saying why it exists and when it can go (opt-in).",
        rationale: "Rules without a recorded reason become impossible to delete safely; recording the reason removed most excess instructions in a controlled study.",
        link: "https://arxiv.org/abs/2608.11095",
        default_severity: Severity::Info,
        fixable: false,
        opt_in: true,
        explain: "Expects `<!-- why: ... | seen: ... | outcome: ... | retire-when: ... -->` on the line above each list-item rule in always-loaded files, or an entry in `.mnemoa/rationale.toml`. Claude Code strips block-level HTML comments, so they cost no context there. Enable with `rules.enable = [\"MNA011\"]`.",
    },
    RuleInfo {
        id: "MNA012",
        name: "promoted-but-present",
        summary: "A rule promoted to an executable check is still in an always-loaded file.",
        rationale: "Once a check enforces the rule, the prose only costs context.",
        link: SMELLS,
        default_severity: Severity::Warn,
        fixable: true,
        opt_in: false,
        explain: "Compares rule text hashes against `.mnemoa/ledger.toml`. An entry with state `applied` means the check exists; the prose can go. Run `mnemoa forget --apply` to remove it.",
    },
    RuleInfo {
        id: "MNA013",
        name: "unloaded-file",
        summary: "An instruction file that a target would normally read is never loaded.",
        rationale: "Rules nobody loads give a false sense of control.",
        link: CLAUDE_MEMORY,
        default_severity: Severity::Warn,
        fixable: true,
        opt_in: false,
        explain: "Covers AGENTS.md hidden from Claude Code by .claude/CLAUDE.md or CLAUDE.local.md, imports deeper than four hops, plain `.md` files in `.cursor/rules/` (info, since that behavior is only search-verified), Copilot instruction files without `applyTo`, Kiro `fileMatch` steering without a pattern, and AGENTS.md shadowed by AGENTS.override.md for Codex (info).",
    },
];

/// Metadata for a rule by ID (case-insensitive).
pub fn info(id: &str) -> Option<&'static RuleInfo> {
    RULES.iter().find(|r| r.id.eq_ignore_ascii_case(id))
}

/// Everything a rule can look at.
pub struct Context<'a> {
    pub ws: &'a Workspace,
    pub config: &'a Config,
    pub targets: &'a [Target],
    /// Git history, when the root is inside a repository.
    pub history: Option<&'a dyn History>,
    facts: std::sync::OnceLock<RepoFacts>,
}

impl<'a> Context<'a> {
    pub fn new(ws: &'a Workspace, config: &'a Config, targets: &'a [Target]) -> Self {
        Context {
            ws,
            config,
            targets,
            history: None,
            facts: std::sync::OnceLock::new(),
        }
    }

    /// Attach git history.
    pub fn with_history(mut self, history: Option<&'a dyn History>) -> Self {
        self.history = history;
        self
    }

    /// Repository facts (task runners, enforcers, dependencies), computed on first use.
    pub fn facts(&self) -> &RepoFacts {
        self.facts.get_or_init(|| RepoFacts::collect(self.ws))
    }

    /// Instruction documents: every non-plan surface plus imported files.
    pub fn instruction_docs(&self) -> impl Iterator<Item = &'a Doc> + 'a {
        let ws = self.ws;
        ws.docs.values().filter(move |d| {
            ws.discovery
                .surface(&d.path)
                .is_none_or(|s| !s.kind.is_plan())
        })
    }

    /// Documents that at least one configured target loads from the first turn.
    pub fn always_docs(&self) -> Vec<&'a Doc> {
        let ws = self.ws;
        ws.always_loaded_union(self.targets)
            .into_iter()
            .filter_map(|p| ws.doc(&p))
            .collect()
    }

    /// Targets (among the configured ones) that load `path` from the first turn.
    pub fn loaded_by(&self, path: &str) -> Vec<Target> {
        self.targets
            .iter()
            .copied()
            .filter(|t| self.ws.always_loaded(*t).iter().any(|f| f.path == path))
            .collect()
    }
}

/// A scan rule.
pub trait Check: Sync {
    fn info(&self) -> &'static RuleInfo;
    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>);
}

/// Helper for building findings with a rule's ID, name and default severity.
pub fn finding_at(
    info: &RuleInfo,
    doc: &Doc,
    span: std::ops::Range<usize>,
    message: impl Into<String>,
) -> Finding {
    Finding::at(
        info.id,
        info.name,
        info.default_severity,
        doc,
        span,
        message,
    )
}

pub fn finding_file(info: &RuleInfo, path: &str, message: impl Into<String>) -> Finding {
    Finding::new(info.id, info.name, info.default_severity, path, message)
}

/// Every implemented rule.
pub fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(size::ContextBloat),
        Box::new(lint::LintLeakage),
        Box::new(skills::SkillLeakage),
        Box::new(conflicts::ConflictingInstructions),
        Box::new(history_rules::InitFossilization),
        Box::new(references::BlindReference),
        Box::new(references::BrokenReference),
        Box::new(commands::StaleCommand),
        Box::new(size::DiscoverableContent),
        Box::new(loading::CrossFileDivergence),
        Box::new(loading::MissingRationale),
        Box::new(promoted::PromotedButPresent),
        Box::new(loading::UnloadedFile),
    ]
}

/// Run every enabled rule and apply severity overrides. Findings are sorted.
pub fn run(cx: &Context<'_>) -> Vec<Finding> {
    run_with(cx, &registry(), None)
}

/// Run a set of rules, optionally limited to the given IDs.
pub fn run_with(
    cx: &Context<'_>,
    rules: &[Box<dyn Check>],
    only: Option<&[String]>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for rule in rules {
        let info = rule.info();
        if let Some(only) = only {
            if !only.iter().any(|o| o.eq_ignore_ascii_case(info.id)) {
                continue;
            }
        } else if !cx.config.rule_enabled(info.id, info.opt_in) {
            continue;
        }
        let mut found = Vec::new();
        rule.check(cx, &mut found);
        let overridden = cx
            .config
            .rules
            .severity
            .keys()
            .any(|k| k.eq_ignore_ascii_case(info.id));
        for mut f in found {
            if overridden {
                f.severity = cx.config.severity_for(info.id, f.severity);
            }
            out.push(f);
        }
    }
    diag::sort(&mut out);
    out.dedup_by(|a, b| a.fingerprint == b.fingerprint && a.line == b.line);
    out
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use crate::workspace::tests::ws;

    /// Run one rule over in-memory files with default config and targets.
    pub fn check(rule: &dyn Check, files: &[(&str, &str)]) -> Vec<Finding> {
        check_with(rule, files, &Config::default())
    }

    pub fn check_with(rule: &dyn Check, files: &[(&str, &str)], config: &Config) -> Vec<Finding> {
        let w = ws(files);
        let cx = Context::new(&w, config, &config.targets);
        let mut out = Vec::new();
        rule.check(&cx, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_ids_are_sequential_and_unique() {
        for (i, r) in RULES.iter().enumerate() {
            assert_eq!(r.id, format!("MNA{:03}", i + 1));
            assert!(!r.explain.is_empty() && !r.summary.is_empty());
            assert!(r.link.starts_with("https://"));
        }
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(info("mna007").unwrap().name, "broken-reference");
        assert!(info("MNA999").is_none());
    }

    #[test]
    fn only_mna011_is_opt_in() {
        let opt: Vec<&str> = RULES.iter().filter(|r| r.opt_in).map(|r| r.id).collect();
        assert_eq!(opt, vec!["MNA011"]);
    }

    struct Always;
    impl Check for Always {
        fn info(&self) -> &'static RuleInfo {
            &RULES[5]
        }
        fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
            for d in cx.instruction_docs() {
                out.push(finding_at(self.info(), d, 0..1, "x"));
            }
        }
    }

    #[test]
    fn run_applies_disable_and_severity_overrides() {
        let w = crate::workspace::tests::ws(&[("AGENTS.md", "x\n")]);
        let mut c = Config::default();
        let rules: Vec<Box<dyn Check>> = vec![Box::new(Always)];
        let cx = Context::new(&w, &c, &c.targets);
        assert_eq!(run_with(&cx, &rules, None)[0].severity, Severity::Info);

        c.rules.severity.insert("MNA006".into(), Severity::Error);
        let cx = Context::new(&w, &c, &c.targets);
        assert_eq!(run_with(&cx, &rules, None)[0].severity, Severity::Error);

        assert_eq!(testutil::check(&Always, &[("AGENTS.md", "x")]).len(), 1);
        let f = finding_file(&RULES[0], "AGENTS.md", "whole file");
        assert_eq!((f.line, f.rule.as_str()), (1, "MNA001"));

        c.rules.disable.push("MNA006".into());
        let cx = Context::new(&w, &c, &c.targets);
        assert!(run_with(&cx, &rules, None).is_empty());
        assert_eq!(
            run_with(&cx, &rules, Some(&["MNA006".to_string()])).len(),
            1
        );
    }
}
