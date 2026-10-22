# Changelog

## v0.1.0 — 2026-10-22

First release of mnemoa: an offline Rust CLI and HTTP API that keeps repo memory for coding agents small, true and enforced.

### Install

Download the archive for your platform below, then unpack it and put `mnemoa` (and `mnemoa-api`, if you want the HTTP server) on your `PATH`:

```bash
tar -xzf mnemoa-v0.1.0-<target>.tar.gz
shasum -a 256 -c mnemoa-v0.1.0-<target>.tar.gz.sha256   # run next to the .tar.gz
./mnemoa-v0.1.0-<target>/mnemoa scan .
```

Prebuilt targets:
- `x86_64-unknown-linux-gnu`: built on Ubuntu 24.04, so it needs glibc 2.39 or newer.
- `aarch64-unknown-linux-gnu`: built on Ubuntu 24.04, same glibc requirement.
- `aarch64-apple-darwin`: Apple silicon.

On other platforms, build from source with `cargo install --git https://github.com/Flotrpy/mnemoa --tag v0.1.0 mnemoa-cli`.

The prebuilt binaries include the optional LLM drafter (`mnemoa promote --llm`). It stays off and makes no network calls unless you enable it in config or with `MNEMOA_LLM_ENABLED=true`.

### What's in it

- **`mnemoa scan`**:
  - 13 deterministic rules (MNA001–MNA013) for AGENTS.md, CLAUDE.md, Cursor, Copilot, Kiro, Cline and Gemini instruction files;
  - output as pretty text, JSON, SARIF 2.1.0 or GitHub annotations, plus a baseline file.
- **Per-agent loading model:** which files each agent loads on every turn and which it loads on demand.
- **`mnemoa budget`:** always-loaded tokens per agent, with an o200k_base estimate and Codex's 32 KiB cap.
- **`mnemoa drift`:**
  - checks paths, symbols and commands in docs against the code;
  - checks Spec Kit and Kiro task lists and Cline memory-bank freshness;
  - with `--base`, catches files and symbols deleted or renamed in a branch.
- **`mnemoa promote`:** turns enforceable prose rules into real checks and records them in `.mnemoa/ledger.toml`.
- **`mnemoa retire`:** removes rules whose `retire-when` condition is met.
- **`mnemoa forget`:** a dry-run prune plan with before/after tokens.
- **`mnemoa init`:** writes a minimal scaffold and never overwrites files.
- **`mnemoa-api`:** an HTTP server with the same commands.
  - Requests are confined to `MNEMOA_ROOT`.
  - Writes are off unless `MNEMOA_API_ALLOW_WRITE=true`, and they never follow symlinks out of the repository.
  - The CLI can call it with `--server`.
- **Python SDK** in `sdk/python`, with sync and async clients. It is not on PyPI; install it from the repository.
- A GitHub Action (`uses: Flotrpy/mnemoa@v0.1.0`), `mnemoa-bench`, and a Colab notebook for the corpus study.

### Known limitations

- **Most rules report many false positives on real repositories.** Of 256 hand-labeled findings from 77 public repositories, 33.6% were correct.
  - High: MNA001 context-bloat (100%) and MNA009 discoverable-content (64%).
  - Lowest: MNA004 conflicting-instructions (0%), MNA002 lint-leakage (4%) and MNA008 stale-command (7%).
  - Treat their findings as prompts to look, not as errors to fix blindly.
  - The table and the main false-positive causes are in the README under [Measured on real repositories](https://github.com/Flotrpy/mnemoa#measured-on-real-repositories). The labels came from Claude, the model that wrote mnemoa.
- **Not published elsewhere:** there is no Docker image, crates.io crate or PyPI package. `docker compose up` builds the image locally.
- **No Windows build.** CI tests Linux and macOS only.
- **Some agent-loading details are unverified,** including Copilot's layout, Kiro's CLI behavior and Codex comment stripping. `docs/research.md` marks each one.
