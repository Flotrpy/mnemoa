# Rules

mnemoa ships 13 rules. Every rule under-matches on purpose: when a detector is not confident it stays silent or reports `info`. Each rule has at least one true-positive test and two near-miss negatives in `crates/mnemoa-core/src/rules/`; `fixtures/smelly` triggers every rule that does not need git history.

Configure rules in `.mnemoa.toml`: `rules.disable`, `rules.enable` (opt-in rules), `[rules.severity]` overrides, and the thresholds `max_lines`, `token_budget` and `fossil_commits`. `mnemoa explain <ID>` prints the text below.

| ID | Name | Default | Fixable by `forget` | Opt-in |
|---|---|---|---|---|
| [MNA001](#mna001-context-bloat) | context-bloat | warn | yes | no |
| [MNA002](#mna002-lint-leakage) | lint-leakage | warn | yes | no |
| [MNA003](#mna003-skill-leakage) | skill-leakage | info | yes | no |
| [MNA004](#mna004-conflicting-instructions) | conflicting-instructions | info | no | no |
| [MNA005](#mna005-init-fossilization) | init-fossilization | warn | no | no |
| [MNA006](#mna006-blind-reference) | blind-reference | info | yes | no |
| [MNA007](#mna007-broken-reference) | broken-reference | error | yes | no |
| [MNA008](#mna008-stale-command) | stale-command | warn | no | no |
| [MNA009](#mna009-discoverable-content) | discoverable-content | info | yes | no |
| [MNA010](#mna010-cross-file-divergence) | cross-file-divergence | warn | yes | no |
| [MNA011](#mna011-missing-rationale) | missing-rationale | info | no | yes |
| [MNA012](#mna012-promoted-but-present) | promoted-but-present | warn | yes | no |
| [MNA013](#mna013-unloaded-file) | unloaded-file | warn | yes | no |

## MNA001 context-bloat

An always-loaded file or a target's always-loaded set is over budget.

**Why:** Longer context lowers accuracy even on easy tasks, and every always-loaded line is paid on every turn. ([source](https://code.claude.com/docs/en/memory))

Fires when one always-loaded file has at least `rules.max_lines` lines (default 200, the size Claude Code's docs recommend staying under), when a target's always-loaded set is over `rules.token_budget` estimated tokens (default 4000), or when Codex would drop guidance past its 32 KiB `project_doc_max_bytes` cap. Fix by deleting what the agent can discover, promoting enforceable rules to checks, and moving task-specific sections to on-demand files (`mnemoa forget`).

- Flags: A 260-line CLAUDE.md, or a target whose always-loaded set is ~9,000 tokens.
- Leaves alone: A 40-line AGENTS.md; a 300-line `.claude/rules/api.md` with `paths` (loaded on demand).

## MNA002 lint-leakage

A prose rule restates what a configured formatter or linter already enforces.

**Why:** The tool already enforces it, so the prose only spends tokens. Lint leakage was the most common smell (62%) in the SCAM 2026 study. ([source](https://arxiv.org/abs/2606.15828))

mnemoa first detects which enforcers the repo configures (rustfmt, clippy, EditorConfig, Prettier, ESLint, Biome, Ruff, Black, gofmt, golangci-lint and others), then matches rule text against a catalog of topics each enforcer covers. A rule only fires when the matching enforcer is configured; for opt-in lints such as `clippy::unwrap_used` the lint itself must be enabled. Delete the line, or run `mnemoa promote` to record the check in the ledger.

- Flags: `- Use 2 spaces for indentation.` with `.editorconfig` setting `indent_size = 2`.
- Leaves alone: The same line with no EditorConfig; `- Never use .unwrap()` when `clippy::unwrap_used` is not enabled.

## MNA003 skill-leakage

A task-specific procedure sits in an always-loaded file.

**Why:** Procedures for rare tasks cost context on every task. Move them to a file the agent loads on demand. ([source](https://arxiv.org/abs/2606.15828))

Fires on a section or paragraph that introduces a task ("When adding a new ...", "To release ...", "How to ...") followed by an ordered list of three or more steps, or on any ordered list of eight or more steps. Move the section to `.claude/rules/` with `paths`, a Kiro steering file with `inclusion: manual`, a Cursor rule with `globs`, or `docs/agents/*.md` with a one-line pointer.

- Flags: `## Adding a new endpoint` followed by four numbered steps in AGENTS.md.
- Leaves alone: A two-step list; the same procedure in `.claude/rules/` with `paths`.

## MNA004 conflicting-instructions

Two instructions contradict each other (deterministic subset).

**Why:** When instructions conflict the agent may follow either. Detection of conflicts is hard, so this rule only reports patterns it can check and never above info. ([source](https://arxiv.org/abs/2606.15828))

Checks three patterns across the always-loaded files of each target: contradictory tool choices (npm and pnpm, pip and uv, jest and vitest, or a tool that disagrees with the lockfile in the repo), always/never pairs on the same subject, and one concept placed in two different paths. Needs review: LLM detectors reached only 57% precision on this smell in the SCAM 2026 study.

- Flags: `Install with npm install.` and `Use pnpm for everything.` in files loaded together.
- Leaves alone: `Use pnpm, never npm.`; the two rules in files no target loads together.

## MNA005 init-fossilization

An instruction file was committed once and never updated while the code moved on.

**Why:** Files generated once (for example by /init) and never revisited drift from the code they describe. ([source](https://arxiv.org/abs/2606.15828))

Fires when an always-loaded instruction file has exactly one commit in history and at least `rules.fossil_commits` (default 25) later commits touched non-Markdown files. Silent in shallow clones and repos without git history.

- Flags: AGENTS.md committed once, then 30 code commits without touching it.
- Leaves alone: A file updated since; a shallow clone; a CLAUDE.md that only contains `@AGENTS.md`.

## MNA006 blind-reference

A link or path with no word on when or why to read it.

**Why:** The agent cannot tell whether the target matters for its task, so it either ignores it or reads it every time. ([source](https://arxiv.org/abs/2606.15828))

Fires on a list item or paragraph that consists of a link or path and nothing else beyond filler words ("see", "docs", "more info"). Add when or why to read it: "Read docs/api.md before changing handlers."

- Flags: `- [API docs](docs/api.md)`
- Leaves alone: `- Read [API docs](docs/api.md) before changing handlers.`; a link under `## Read before releasing`.

## MNA007 broken-reference

A link, path or @import that does not resolve.

**Why:** A broken pointer wastes a tool call at best and misleads the agent at worst. ([source](https://arxiv.org/abs/2606.15828))

Checks relative Markdown links, Claude Code `@path` imports, and inline-code paths whose top-level directory exists in the repo. URLs, anchors, globs and placeholders are skipped. Imports to `~/` or absolute paths are reported at info level because they depend on the user's machine.

- Flags: `[guide](docs/missing.md)`; `@docs/gone.md`; `` `src/api/handlers.rs` `` when only `src/api/routes.rs` exists.
- Leaves alone: URLs and anchors; globs like `src/**/*.rs`; `` `target/debug/app` `` (ignored directory); "Create `src/new.rs`".

## MNA008 stale-command

A documented command does not exist in the repo's task runners.

**Why:** Agents run documented commands verbatim; a renamed script fails and the agent improvises. ([source](https://github.com/toshi0607/agentslint))

Checks commands in code spans and shell code blocks: `make` targets against Makefiles, `just` recipes against justfiles, `npm|pnpm|yarn|bun run` against package.json scripts, `cargo <alias>` against `.cargo/config.toml` aliases, `pdm run`/`hatch run` against pyproject scripts, and `./path` executables against the tree. Silent when a runner file includes others it cannot see.

- Flags: `` `make check` `` when the Makefile has no `check` target.
- Leaves alone: `` `make -j4 test V=1` `` when `test` exists; `` `pnpm --filter web run dev` ``; a Makefile with `include`.

## MNA009 discoverable-content

A directory tree or stack list that repeats what the tree and manifests already show.

**Why:** Overview sections did not help agents find files faster, and they go stale as the tree changes. ([source](https://arxiv.org/abs/2602.11988))

Fires on a directory tree whose entries mostly exist in the repo, and on a list of three or more short items that mostly name dependencies from Cargo.toml, package.json or pyproject.toml. Kept at info: GitHub's agents.md guidance still recommends describing structure.

- Flags: A `├──` tree whose entries exist in the repo; a list of TypeScript, React, Vite that matches package.json.
- Leaves alone: A tree of a planned layout; a list of rules that mention React.

## MNA010 cross-file-divergence

CLAUDE.md and AGENTS.md both exist, neither imports the other, and they differ.

**Why:** Claude Code reads CLAUDE.md and skips AGENTS.md when both exist, so rules in only one file reach only some agents. ([source](https://code.claude.com/docs/en/memory))

Claude Code reads AGENTS.md only when no CLAUDE.md, .claude/CLAUDE.md or CLAUDE.local.md exists at or above the working directory. With both files present and no `@AGENTS.md` import or symlink, Claude Code never sees AGENTS.md. The finding lists how many rules each file has that the other lacks. Fix: make CLAUDE.md `@AGENTS.md` plus Claude-only additions.

- Flags: AGENTS.md and CLAUDE.md side by side with different rules and no import.
- Leaves alone: CLAUDE.md that starts with `@AGENTS.md`; a symlink; identical copies.

## MNA011 missing-rationale

A rule has no rationale comment saying why it exists and when it can go (opt-in).

**Why:** Rules without a recorded reason become impossible to delete safely; recording the reason removed most excess instructions in a controlled study. ([source](https://arxiv.org/abs/2608.11095))

Expects `<!-- why: ... | seen: ... | outcome: ... | retire-when: ... -->` on the line above each list-item rule in always-loaded files, or an entry in `.mnemoa/rationale.toml`. Claude Code strips block-level HTML comments, so they cost no context there. Enable with `rules.enable = ["MNA011"]`.

- Flags: `- Never run migrations by hand.` with no comment above (when enabled).
- Leaves alone: The same rule with `<!-- why: ... | retire-when: manual -->` above it, or a sidecar entry; numbered procedure steps.

## MNA012 promoted-but-present

A rule promoted to an executable check is still in an always-loaded file.

**Why:** Once a check enforces the rule, the prose only costs context. ([source](https://arxiv.org/abs/2606.15828))

Compares rule text hashes against `.mnemoa/ledger.toml`. An entry with state `applied` means the check exists; the prose can go. Run `mnemoa forget --apply` to remove it.

- Flags: A rule whose hash is in the ledger with `state = "applied"`, still in AGENTS.md.
- Leaves alone: A proposed entry; a rule that only exists in an on-demand file.

## MNA013 unloaded-file

An instruction file that a target would normally read is never loaded.

**Why:** Rules nobody loads give a false sense of control. ([source](https://code.claude.com/docs/en/memory))

Covers AGENTS.md hidden from Claude Code by .claude/CLAUDE.md or CLAUDE.local.md, imports deeper than four hops, plain `.md` files in `.cursor/rules/` (info, since that behavior is only search-verified), Copilot instruction files without `applyTo`, Kiro `fileMatch` steering without a pattern, and AGENTS.md shadowed by AGENTS.override.md for Codex (info).

- Flags: AGENTS.md next to a CLAUDE.local.md and no CLAUDE.md that imports it; imports five hops deep.
- Leaves alone: A CLAUDE.md that imports AGENTS.md; a Kiro steering file when Kiro is not a configured target.
