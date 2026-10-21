# CONTEXT

What a fresh session needs beyond `AGENTS.md`.

- `crates/mnemoa-core`: the library. All detection, drift, promotion and forget logic.
- `crates/mnemoa-cli`: the `mnemoa` binary. Thin wrapper over core, or over the API with `--server`.
- `crates/mnemoa-api`: the `mnemoa-api` HTTP server (axum).
- `crates/mnemoa-bench`: the `mnemoa-bench` benchmark runner.
- `sdk/python`: the `mnemoa-sdk` Python package.
- `rules/promotions.toml`: the topic catalog shared by MNA002 and `mnemoa promote`.
- `fixtures/`: small repos for snapshot tests; snapshots live in `crates/mnemoa-core/tests/snapshots`.
- `docs/plan.md`: the commit plan with progress checkboxes.

Rule IDs are `MNA001` and up. Findings are never guessed: when a detector is unsure, it stays silent or reports `info`.
