# Architecture

Four Rust crates and a Python SDK.

| Crate | Kind | Role |
|---|---|---|
| `mnemoa-core` | library | Discovery, the document model, rules, drift, promotion, retirement, forget, reports. Never touches the network unless built with the `llm` feature. |
| `mnemoa-cli` | binary `mnemoa` | Thin command layer over core; with `--server` it calls `mnemoa-api` instead. |
| `mnemoa-api` | binary `mnemoa-api` | axum server exposing core over HTTP with path confinement and a write switch. |
| `mnemoa-bench` | binary `mnemoa-bench` | 12 benchmark suites over core. |
| `sdk/python` | package `mnemoa-sdk` | httpx clients for the API and a wrapper for the CLI. |

## Pipeline

```
        repo on disk
             │
   discover::discover ──► Discovery { files, dirs, surfaces[ kind, loading per target ] }
             │
   Workspace::load ─────► parsed Doc per surface + every @import (≤ 4 hops for Claude Code)
             │
   ┌─────────┼────────────┬──────────────┬───────────────┐
   │         │            │              │               │
 rules    budget        drift         promote          retire
 MNA001–  per target    refs, symbols, catalog +       retire-when
 MNA013   tokens        commands,      structural      conditions
   │                    plans          patterns,       (safety guard)
   │                    (+ --base)     ledger, drafter
   └─────────┴────────────┴──────┬───────┴───────────────┘
                                 │
                          forget::plan  ──► non-overlapping edits, new files,
                                 │          before/after tokens per target
                                 ▼
                 report (pretty · json · sarif · github) / apply
```

## Modules in `mnemoa-core`

| Module | What it does |
|---|---|
| `discover` | Walks the repo with `ignore` (respects `.gitignore`), classifies agent files into `SurfaceKind`s and records, per target, whether each is loaded always, on demand (with the trigger) or not at all. The loading model follows each agent's documentation; see `docs/research.md`. |
| `frontmatter` | YAML front matter (`paths`, `globs`, `alwaysApply`, `inclusion`, `applyTo`). |
| `doc` | `pulldown-cmark` offset iterator → headings, rules (list items and imperative sentences, with polarity), code spans and blocks, links, `@imports`, HTML comments, lists, paragraphs, all with byte spans. |
| `workspace` | Parses surfaces and imported files; computes each target's always-loaded set, including Claude Code imports and the ones it drops. |
| `tokens`, `budget` | o200k_base token estimates (labeled as estimates) and per-target cost, including the Codex 32 KiB cap. |
| `diag`, `rules` | `Finding` with stable fingerprints and byte spans; the `Check` trait, rule metadata and the runner. One file per rule family under `rules/`. |
| `repo`, `enforcers`, `catalog` | Task-runner commands, dependencies and lockfiles; configured formatters and linters; the topic catalog in `rules/promotions.toml`. |
| `history` | `History` trait (commits newest first with changed paths, tree diffs, file contents at a revision). `GixHistory` uses `gix`; `MemoryHistory` backs tests. |
| `symbols`, `drift`, `plans` | Regex declaration index plus every identifier mentioned in source (a seam for tree-sitter); reference drift and diff-aware drift; Spec Kit, Kiro and Cline plan drift. |
| `rationale`, `ledger`, `promote`, `drafter`, `llm` | Rationale comments and the sidecar; the promotion ledger; artifact emitters; LLM draft prompting, parsing and ast-grep validation; the HTTP client (feature `llm`). |
| `retire`, `forget`, `init` | Retire-when evaluation with a safety guard; the combined prune plan; scaffolding. |
| `scan`, `report`, `config` | The scan entry point and baseline; renderers; `.mnemoa.toml` with `MNEMOA_*` overrides. |

## Design rules

- **Under-match.** A detector that is unsure stays silent or reports `info`. Examples: lint leakage needs a configured enforcer (and for opt-in lints, the lint itself); stale commands are skipped when a Makefile includes other files; inline-code paths are only checked when their top-level directory exists. This is the goal, not yet the result: on 77 public repositories most rules fell well short of it (see [Measured on real repositories](../README.md#measured-on-real-repositories)).
- **Offline by default.** Only `llm.rs` (feature `llm`) and the CLI's `--server` client make HTTP calls. A drafted rule is used only after ast-grep validates it against the model's own positive and negative samples.
- **Dry run first.** `promote`, `retire` and `forget` compute full new file contents and a unified diff; nothing is written until `--apply` (and `--yes` for forget). The API refuses writes unless `MNEMOA_API_ALLOW_WRITE=true`.
- **Never overwrite blindly.** Merges into `.editorconfig`, TOML, Prettier JSON and pre-commit YAML fall back to a manual snippet on conflict; `init` creates files with `create_new`.

## Extension points

- **A new rule:** add a `Check` in `crates/mnemoa-core/src/rules/`, its `RuleInfo` in `RULES`, register it, add tests (one true positive, two near misses) and a fixture line. See CONTRIBUTING.md.
- **A new promotion:** add a `[[topic]]` to `rules/promotions.toml`; if mnemoa should emit a check for it, add a branch to `Planner::emit_topic` in `promote.rs`.
- **A new agent:** add a `Target`, classify its files in `discover::classify`, and cite the loading behavior in `docs/research.md`.
- **Better symbols:** replace `SymbolIndex::build` with a tree-sitter backend; the rest of drift only uses `declared` and `knows`.
