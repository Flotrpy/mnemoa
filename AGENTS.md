# AGENTS.md

mnemoa is an offline Rust CLI and API that keeps agent repo memory small, true and enforced.

## Commands

- `make check` runs fmt, clippy and tests. It must pass before every commit.
- `make scan` runs mnemoa on this repo. It must report zero findings.
- `make sdk-test` runs the Python SDK tests; build the binaries first.

## Boundaries

- `mnemoa-core` never makes network calls. Network code lives behind the `llm` feature.
- Never publish crates, PyPI packages or images.
- Every scan rule needs one true-positive fixture and two near-miss negatives.
- Files under `fixtures/` contain deliberate mistakes that tests depend on; never fix them.

## Read when needed

- Read [docs/plan.md](docs/plan.md) before starting work, to see which commit is next.
- Read [CONTEXT.md](CONTEXT.md) when you need the layout of crates and modules.
