# Contributing

Thanks for helping. Open an issue before large changes so we can agree on the approach.

## Setup

```bash
make fmt && make lint     # rustfmt, then fmt --check and clippy -D warnings (all features)
make test                 # cargo test, plus the llm feature
make scan                 # mnemoa on this repo; must report zero findings
cargo build -p mnemoa-cli -p mnemoa-api && make sdk-test   # Python SDK
make coverage-summary     # needs cargo-llvm-cov
```

Installing `ast-grep` (`pip install ast-grep-cli`) lets the draft-validation tests run instead of skipping.

## Conventions

- Conventional Commits (`feat(core): …`, `fix(cli): …`, `docs: …`, `test: …`, `ci: …`), one change per commit, each commit green.
- `mnemoa-core` makes no network calls outside the `llm` feature.
- Detectors under-match: when unsure, stay silent or report `info`.
- Snapshot tests live in `crates/mnemoa-core/tests/fixtures.rs`. After an intended output change, run `INSTA_UPDATE=always cargo test -p mnemoa-core --test fixtures` and review every changed `.snap` file before committing.
- `fixtures/` contains deliberate mistakes. Do not fix them; add new cases instead.

## How to add a rule

1. Pick the next ID and add its `RuleInfo` to `RULES` in `crates/mnemoa-core/src/rules/mod.rs`: summary, rationale, a research link, default severity, `fixable`, `opt_in` and the `explain` text.
2. Implement `Check` in a file under `crates/mnemoa-core/src/rules/` and add it to `registry()`. Use `cx.always_docs()` for rules about always-loaded context and `cx.instruction_docs()` for everything else. Build findings with `finding_at` so they carry a byte span.
3. Tests next to the rule: at least one true positive and two near-miss negatives (`rules::testutil::check`).
4. Add a triggering line to `fixtures/smelly` and update the snapshots.
5. Regenerate `docs/rules.md` and add an example; a test checks that every rule is documented.
6. If `forget` can fix it, add a branch in `crates/mnemoa-core/src/forget.rs`.

## How to add a promotion

1. Add a `[[topic]]` to `rules/promotions.toml`:
   - `id`, `summary`, case-insensitive `patterns`, and the `enforcers` that cover it.
   - Write `"tool:regex"` when the enforcer only covers the topic with an opt-in setting. The regex must match its config. For ESLint and Biome, a lint set to `warn` or `off` there does not count.
   - Set `named = true` when the rule must name the tool.
   - Add `exclude` patterns for rules that match but are about something else (a commit message's length rather than code's). Excludes also see the rule's enclosing headings.
   - Set `restates = false` when matching rules say which tool to run rather than restate what it checks.
2. This alone makes MNA002 flag the prose when an enforcer is configured, and `promote` remove the prose in that case.
3. To emit a check when nothing enforces it yet, add a branch to `Planner::emit_topic` in `crates/mnemoa-core/src/promote.rs` that returns artifacts (EditorConfig keys, a TOML key, Prettier JSON, an ast-grep rule, or a pre-commit hook). Merge into existing files and fall back to `Action::Manual` on conflict.
4. Test with `plan_for(...)` in `promote.rs`: the artifact content, the prose removal and a near miss. If the artifact is an ast-grep rule, check it with real ast-grep.

## How to release

1. On a branch, bump `version` under `[workspace.package]` in `Cargo.toml` and add a `## vX.Y.Z — YYYY-MM-DD` section at the top of `CHANGELOG.md`. That section becomes the release notes. Merge to `main`.
2. Publish in one of two ways:
   - **From GitHub:** go to Actions › Release › Run workflow, choose `main`, tick **Publish**, and run. The workflow tags the commit `vX.Y.Z` itself.
   - **From a clone:** `git tag -a vX.Y.Z -m "mnemoa vX.Y.Z" && git push origin vX.Y.Z`.
3. Either way, the workflow builds Linux (x86_64, arm64) and macOS (Apple silicon) binaries, smoke-tests them, and publishes a GitHub release with archives, checksums and the changelog section.

What the workflow checks:
- A run without **Publish** is a dry run: it builds and packages but doesn't publish.
- It refuses to publish if the version's tag or release already exists, if `CHANGELOG.md` has no section for the version, or if a pushed tag doesn't match `Cargo.toml`.

Releases go to GitHub only; nothing is published to crates.io, PyPI or a container registry.

## Research claims

`docs/research.md` labels every claim primary, search or unverified. Cite sources, paraphrase, and keep quotes to short phrases.
