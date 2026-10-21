# mnemoa

> Stateless agents. Durable repos.

[![Build](https://github.com/Flotrpy/mnemoa/actions/workflows/ci.yml/badge.svg)](https://github.com/Flotrpy/mnemoa/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Crates.io](https://img.shields.io/badge/crates.io-not%20published-lightgrey.svg)](#path-b--binary)
[![PyPI](https://img.shields.io/badge/PyPI-not%20published-lightgrey.svg)](#path-c--python-sdk)
[![Docker](https://img.shields.io/badge/docker-build%20locally-lightgrey.svg)](#path-a--docker--optional-ollama-for-the-llm-drafter)
[![Open in Colab](https://colab.research.google.com/assets/colab-badge.svg)](https://colab.research.google.com/github/Flotrpy/mnemoa/blob/main/notebooks/mnemoa_colab.ipynb)

---

## What is mnemoa?

Coding agents start every session with an empty head. What survives between sessions is what lives in the repo: `AGENTS.md`, `CLAUDE.md`, rules files, plans, specs and the code itself. Memory layers such as mnemo give the agent more to remember; mnemoa does the opposite job and keeps the repo's own memory honest. That memory rots the way chat memory does: instruction files grow, restate what linters already enforce, point at files that moved, and keep rules nobody can justify deleting.

mnemoa keeps repo memory **small, true and enforced**. It finds context no agent needs, turns enforceable prose rules into real checks (EditorConfig keys, Cargo lints, Ruff settings, ast-grep rules, pre-commit hooks) so the prose can leave the context window, tracks why each rule exists and when it can go, and fails CI when docs and plans drift from the code. It is offline and deterministic by default, written in Rust, and models how seven agents (Claude Code, Codex, Cursor, Copilot, Kiro, Cline, Gemini) actually load their files.

---

## How it works

```
 repo ─► discover ─► parse ─┬─► rules MNA001–MNA013 ──────────┐
         (per-agent  (Markdown├─► budget (tokens per agent)   │
          loading     with    ├─► drift (paths, symbols,      ├─► report: pretty · json · sarif · github
          model)      offsets)│    commands, plans, --base)   │
                              ├─► promote (prose → checks)    │
                              └─► retire (retire-when)        ┘
                                          │
                                          └─► forget: one prune plan, before/after tokens, --apply
```

1. **Discover.** Walk the repo (respecting `.gitignore`) and classify every agent file. For each agent, record whether it loads the file at the start, on demand (and on what trigger), or never. For example, Claude Code skips `AGENTS.md` when a `CLAUDE.md` exists, and Codex stops reading after 32 KiB.
2. **Parse.** Build a Markdown model with exact byte offsets: rules (list items and imperative sentences), code spans and blocks, links, `@imports` (followed up to four hops), HTML comments and front matter.
3. **Check.** Run 13 deterministic rules, compute always-loaded tokens per agent, and resolve every path, symbol and command against the tree and, with `--base`, against the diff.
4. **Promote and retire.** Match rules against a catalog of what formatters, linters and ast-grep can enforce; emit the check, remove the prose and record both in `.mnemoa/ledger.toml`. Rules with a `retire-when` condition that is now met get removed, except safety-relevant ones.
5. **Forget.** Combine everything into one dry-run plan with a diff and the before/after token cost per agent. Apply it with `--apply --yes`.

---

## Why mnemoa

| | mnemoa | Most alternatives |
|---|---|---|
| Runtime | One offline Rust binary; optional HTTP API; LLM only if you turn it on | Node or Python CLIs, or model-driven audits that need an API call |
| Detection | 13 deterministic rules and a per-agent loading model; precision on real repos is uneven ([measured](#measured-on-real-repositories)) | Per-tool schema and smell checks, or an LLM's judgment |
| Fixes | Promotes enforceable rules into real checks, then deletes the prose; moves procedures on demand | Report, or propose text edits (agnix and Claude Code's `/doctor prompt-audit` do propose fixes) |
| Drift | Paths, symbols, commands, Spec Kit / Kiro tasks, Cline memory bank, diff-aware `--base` | Mostly broken links and stale commands |
| Scope | Instructions **and** plans/specs across seven agents | Usually instruction files for one or a few tools |
| Rule lifecycle | Rationale comments with `retire-when`, ledger of promotions, `mnemoa retire` | None: rules only accumulate |
| Output | pretty, JSON, SARIF 2.1.0 (schema-validated), GitHub annotations, baselines | Similar formats |

**mnemoa is not for everyone.** If you use one agent on one small repo with a 30-line `AGENTS.md`, you don't need it; read your file once a month instead.

The difference is **promotion plus forget**: mnemoa doesn't just tell you a rule is redundant, it moves the rule into the tool that enforces it, records that in a ledger so the rule never comes back, and shows how many always-loaded tokens each agent saves. For exhaustive per-tool config validation (700+ rules, LSP, MCP), run [agnix](https://github.com/agent-sh/agnix) alongside mnemoa; they don't overlap much.

---

## Quickstart

### Path A — Docker (+ optional Ollama for the LLM drafter)

```bash
git clone https://github.com/Flotrpy/mnemoa && cd mnemoa
cp .env.example .env                       # set MNEMOA_WORKSPACE to the repo you want to check
docker compose up -d                       # builds mnemoa:local; mounts the repo at /workspace/repo
docker exec mnemoa-ollama ollama pull qwen2.5-coder:7b   # only if you set MNEMOA_LLM_ENABLED=true
curl http://localhost:8080/health
curl -X POST http://localhost:8080/scan -H 'content-type: application/json' -d '{"path": "repo"}'
```

The image has not been published; `docker compose up` builds it locally. Writes over HTTP are off unless `MNEMOA_API_ALLOW_WRITE=true` (and the volume is mounted read-write).

### Path B — Binary

```bash
cargo install --path crates/mnemoa-cli                  # add --features llm for the drafter
mnemoa scan .
mnemoa forget                                          # dry run: actions, token table, diff
mnemoa forget --apply --yes
```

LLM drafter (optional, used by `mnemoa promote --llm`):

```bash
# Ollama (default)
export MNEMOA_LLM_ENABLED=true
export MNEMOA_LLM_PROVIDER=ollama
export MNEMOA_LLM_BASE_URL=http://localhost:11434/v1
export MNEMOA_LLM_MODEL=qwen2.5-coder:7b

# OpenAI
export MNEMOA_LLM_PROVIDER=openai
export MNEMOA_LLM_BASE_URL=https://api.openai.com/v1
export MNEMOA_LLM_MODEL=<model>
export MNEMOA_LLM_API_KEY=sk-...

# Anthropic (base URL defaults to https://api.anthropic.com, model to claude-opus-5-5)
export MNEMOA_LLM_PROVIDER=anthropic
export MNEMOA_LLM_API_KEY=sk-ant-...
```

Drafted ast-grep rules are applied only after `ast-grep` (`pip install ast-grep-cli`) confirms them against the model's own positive and negative samples.

### Path C — Python SDK

```bash
pip install -e sdk/python        # `pip install mnemoa-sdk` once published
```

```python
from mnemoa import Client

with Client("http://localhost:8080") as client:
    report = client.scan(".")
    print(report.summary)
    plan = client.forget(".")
    for t in plan["tokens"]:
        print(t["target"], t["before"], "->", t["after"])
```

---

## API Reference

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/health` | none | `HealthResponse` (version, config source, root, write switch, LLM status) |
| POST | `/scan` | `{path, targets?, rules?, strict?}` | `ScanReport` |
| POST | `/budget` | `{path, targets?}` | `BudgetReport` |
| POST | `/drift` | `{path, base?}` | `DriftReport` |
| POST | `/promote` | `{path, llm?, apply?}` | `PromotionPlan` + `written` |
| POST | `/forget` | `{path, apply?}` | `ForgetPlan` + `written` |
| GET | `/rules` | none | `RuleInfo[]` |
| GET | `/rules/{id}` | none | `RuleInfo` |
| GET | `/stats` | none | requests, scans, findings by rule, uptime |

```jsonc
// POST /scan  {"path": "repo"}
{
  "targets": ["claude-code", "codex", "cursor"],
  "findings": [{
    "rule": "MNA007", "name": "broken-reference", "severity": "error",
    "path": "AGENTS.md", "line": 6, "column": 8,
    "message": "Link target `docs/setup.md` does not exist.",
    "fingerprint": "5f0c2d6b1e9a4c37"          // stable across line moves; used by baselines
  }],
  "summary": { "errors": 1, "warnings": 0, "infos": 0 }
}

// POST /forget  {"path": "repo"}
{
  "actions": [{ "kind": "move", "path": "AGENTS.md", "line": 25, "summary": "move \"Adding a new endpoint\" to docs/agents/adding-a-new-endpoint.md" }],
  "tokens":  [{ "target": "codex", "before": 153, "after": 79 }],   // always-loaded tokens, o200k_base estimate
  "diff": "--- a/AGENTS.md\n+++ b/AGENTS.md\n…",
  "written": []                                                      // filled when apply: true is allowed
}
```

Paths must stay inside `MNEMOA_ROOT` (canonicalized; `..` and symlink escapes get `403`). Full reference: [docs/api.md](docs/api.md).

---

## Configuration

### Environment variables

| Variable | Default | Meaning |
|---|---|---|
| `MNEMOA_ROOT` | `.` | Root the API may read |
| `MNEMOA_PORT` | `8080` | API port |
| `MNEMOA_TARGETS` | `claude-code,codex,cursor` | Agents to model |
| `MNEMOA_MAX_LINES` | `200` | MNA001 line threshold |
| `MNEMOA_TOKEN_BUDGET` | `4000` | MNA001 always-loaded token budget per agent |
| `MNEMOA_FOSSIL_COMMITS` | `25` | MNA005 threshold |
| `MNEMOA_LLM_ENABLED` | `false` | Turn on the drafter |
| `MNEMOA_LLM_PROVIDER` | `ollama` | `ollama`, `openai`, `anthropic`, `custom` |
| `MNEMOA_LLM_BASE_URL` | `http://localhost:11434/v1` | OpenAI-compatible base URL (Anthropic: Messages API base) |
| `MNEMOA_LLM_MODEL` | `qwen2.5-coder:7b` | Model name (the tag was not verifiable from the build environment; change it if Ollama lacks it) |
| `MNEMOA_LLM_API_KEY` | `ollama` | API key |
| `MNEMOA_API_ALLOW_WRITE` | `false` | Allow `apply` over HTTP |
| `MNEMOA_SERVER` | unset | Make the CLI call this API instead of running locally |

### TOML config file

`.mnemoa.toml` at the repository root (or `--config`); every key is documented in [`mnemoa.example.toml`](mnemoa.example.toml):

```toml
targets = ["claude-code", "codex", "cursor"]

[scan]
exclude = ["fixtures/**"]
include = ["CONTEXT.md"]
strict = false

[rules]
max_lines = 200
token_budget = 4000
fossil_commits = 25
disable = []
enable = ["MNA011"]          # opt-in rules

[rules.severity]
MNA006 = "warn"

[llm]
enabled = false
```

Precedence, highest first: `MNEMOA_*` environment variables, the `--config` file, `.mnemoa.toml` at the scanned root, built-in defaults.

---

## CLI

```bash
cargo install --path crates/mnemoa-cli
```

```bash
mnemoa scan [PATH] [--format pretty|json|sarif|github] [--strict] [--rules MNA006,MNA007]
                   [--baseline FILE] [--write-baseline] [--no-history]   # lint agent context; exit 1 on errors
mnemoa budget [PATH] [--check]                    # always-loaded tokens per agent, on-demand files, Codex cap
mnemoa drift [PATH] [--base origin/main] [--strict]   # paths, symbols, commands, Spec Kit/Kiro tasks, memory bank
mnemoa promote [PATH] [--apply] [--llm]           # prose rules -> checks + .mnemoa/ledger.toml (dry run shows a diff)
mnemoa retire [PATH] [--apply]                    # remove rules whose retire-when condition is met
mnemoa forget [PATH] [--apply --yes]              # one prune plan with before/after tokens
mnemoa init [PATH] [--ci]                         # minimal AGENTS.md, CLAUDE.md, .mnemoa.toml; never overwrites
mnemoa rules | mnemoa explain MNA006              # rule list and explanations
mnemoa --server http://localhost:8080 scan repo   # run against mnemoa-api (paths relative to MNEMOA_ROOT)

# global: --targets claude-code,codex,cursor,copilot,kiro,cline,gemini  --config FILE  --color auto|always|never
```

In GitHub Actions: `uses: Flotrpy/mnemoa@main` (see [`action.yml`](action.yml)) runs scan and diff-aware drift with annotations.

---

## Python SDK

```bash
pip install -e sdk/python
```

Details in [sdk/python/README.md](sdk/python/README.md).

**Async example**

```python
import asyncio
from mnemoa import AsyncClient

async def main():
    async with AsyncClient("http://localhost:8080") as client:
        budget, drift = await asyncio.gather(client.budget("repo"), client.drift("repo", base="origin/main"))
        for t in budget["targets"]:
            print(t["target"], t["total_tokens"])
        print(len(drift["items"]), "drift items")

asyncio.run(main())
```

More in [examples/basic_usage.py](examples/basic_usage.py). `mnemoa.CLI` wraps the local binary when you don't run a server.

---

## Architecture

Four Rust crates wired together:

| Crate | Role |
|---|---|
| `mnemoa-core` | Discovery and loading model, Markdown model, rules, budget, drift, promotion and ledger, retirement, forget, reports. Offline unless built with `llm`. |
| `mnemoa-cli` | The `mnemoa` binary; runs core locally or calls the API with `--server`. |
| `mnemoa-api` | The `mnemoa-api` axum server with path confinement and a write switch. |
| `mnemoa-bench` | 12 benchmark suites with `--filter` and `--json`. |

More in [docs/architecture.md](docs/architecture.md); rules in [docs/rules.md](docs/rules.md).

---

## Performance

Hardware: Intel Xeon @ 2.10 GHz, 4 cores, Linux x86_64 (cloud container). Numbers are from `mnemoa-bench` on that machine; they will differ on yours.

| # | Suite | Release avg | Release throughput | Debug avg |
|---|---|---:|---:|---:|
| 1 | discovery walk (10k files) | 25.15 ms | 397.8k files/s | 100.11 ms |
| 2 | markdown parse (1k lines) | 973.78 µs | 1.03M lines/s | 9.52 ms |
| 3 | token estimate (1k lines, o200k_base) | 1.61 ms | 621.4k lines/s | 24.14 ms |
| 4 | full rule pass (200-line file) | 2.28 ms | 439.1 passes/s | 26.40 ms |
| 5 | reference resolution (500 refs) | 819.04 µs | 610.5k refs/s | 6.83 ms |
| 6 | stale-command lookup (400 cmds) | 576.45 µs | 693.9k cmds/s | 3.51 ms |
| 7 | lint-leakage classify (200 rules) | 709.21 µs | 282.0k rules/s | 21.15 ms |
| 8 | git history scan (2,000 commits) | 237.34 ms | 8.4k commits/s | 2.83 s |
| 9 | plan drift (300 Spec Kit tasks) | 594.87 µs | 504.3k tasks/s | 12.23 ms |
| 10 | promotion plan (40 rules) | 637.72 µs | 62.7k rules/s | 7.20 ms |
| 11 | SARIF render (1,000 findings) | 9.59 ms | 104.2k findings/s | 53.02 ms |
| 12 | forget pipeline (generated monorepo) | 1.70 ms | 586.8 plans/s | 9.04 ms |

```bash
cargo run -p mnemoa-bench --release              # markdown table
cargo run -p mnemoa-bench --release -- --json    # machine-readable
cargo run -p mnemoa-bench --release -- --filter drift
```

---

## Testing

### Rust

```bash
make check                                   # fmt --check, clippy -D warnings (all features), tests
cargo test -p mnemoa-core --features llm     # adds the HTTP drafter tests
INSTA_UPDATE=always cargo test -p mnemoa-core --test fixtures   # refresh snapshots, then review
make coverage-summary                        # cargo-llvm-cov
```

Tests include per-rule true positives and near misses, snapshot tests over `fixtures/`, property tests for the parsers, SARIF validation against the official 2.1.0 schema, real `git` and `ast-grep` integration (skipped when ast-grep is absent), and API tests for path traversal and symlink escapes.

### Python SDK

```bash
cargo build -p mnemoa-cli -p mnemoa-api
cd sdk/python && pip install -e '.[test]' && pytest -q
```

### Benchmarks

```bash
cargo run -p mnemoa-bench --release -- --quick
```

Current test counts: **265 Rust tests** (269 with `--features llm`) · **9 Python tests** · **12 benchmarks**. Line coverage: **90.5%** (cargo-llvm-cov, same machine).

---

## Research

- Longer inputs hurt: Chroma's [context rot](https://research.trychroma.com/context-rot) report saw 18 models degrade as input grew, even on trivial tasks; [Du et al.](https://arxiv.org/abs/2510.05381) found 13.9–85% drops from length alone, even with whitespace padding; models use the [middle of long contexts](https://arxiv.org/abs/2307.03172) worst.
- Anthropic's [context engineering guide](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents) treats context as a finite attention budget and aims for the smallest set of high-signal tokens.
- [Gloaguen et al. (ICLR 2026)](https://arxiv.org/abs/2602.11988): LLM-generated context files lowered agent success by about 3% and raised cost over 20%; developer-written files helped about 4% but still cost more. Keep requirements minimal.
- [SCAM 2026](https://arxiv.org/abs/2606.15828): 91 of the 100 most-starred repos with AGENTS.md/CLAUDE.md had at least one smell; lint leakage (62%) and context bloat (42%) led.
- [Chakrabarti (2026)](https://arxiv.org/abs/2608.11095): instruction files grew 226% across 1,867 repos; rules are cheap to add and hard to delete once their reason is lost. Recording the reason removed 99.3% of excess rules in a controlled test.
- [Claude Code's memory docs](https://code.claude.com/docs/en/memory): stay under 200 lines per file; imports still cost context; block-level HTML comments are stripped (so rationale comments are free there); AGENTS.md is skipped when a CLAUDE.md exists.
- [Codex](https://developers.openai.com/codex/guides/agents-md) reads one guidance file per directory and stops at 32 KiB by default.
- How mnemoa's own rules do on real repositories: see [Measured on real repositories](#measured-on-real-repositories) below.

Every claim, its verification status and the sources that could not be reached are in [docs/research.md](docs/research.md).

### Measured on real repositories

On 2026-10-21 [notebooks/mnemoa_colab.ipynb](notebooks/mnemoa_colab.ipynb) scanned shallow clones of the 99 public repositories it lists, using mnemoa 0.1.0 with all seven targets. 77 had agent instruction files at the root (AGENTS.md, CLAUDE.md, GEMINI.md, `.cursorrules`, Copilot instructions, or rule directories), and 77.9% of those had at least one finding. Up to 30 findings per rule were then sampled and labeled by hand.

| Rule | Repos flagged | Findings | Labeled | Precision |
|---|---|---|---|---|
| MNA001 context-bloat | 40.3% | 133 | 30 | **100.0%** |
| MNA002 lint-leakage | 13.0% | 23 | 23 | **4.3%** |
| MNA003 skill-leakage | 18.2% | 30 | 30 | **43.3%** |
| MNA004 conflicting-instructions | 6.5% | 28 | 28 | **0.0%** |
| MNA006 blind-reference | 27.3% | 115 | 30 | **16.7%** |
| MNA007 broken-reference | 32.5% | 123 | 30 | **33.3%** |
| MNA008 stale-command | 40.3% | 179 | 30 | **6.7%** |
| MNA009 discoverable-content | 16.9% | 14 | 14 | **64.3%** |
| MNA010 cross-file-divergence | 11.7% | 11 | 11 | **45.5%** |
| MNA013 unloaded-file | 16.9% | 91 | 30 | **36.7%** |

Overall, 86 of 256 labeled findings (33.6%) were correct. MNA005 needs git history, so it was not measured on shallow clones. MNA011 is opt-in and MNA012 needs a promotion ledger; neither fired.

**Read this before quoting the numbers:**
- Precision is low for most rules, so "repos flagged" overstates how common each smell is. It is not comparable to SCAM's prevalence figures, which used different detectors on a different sample.
- The labels come from Claude, the same model that wrote mnemoa. There was one labeler per finding, and ties went against the tool. Every label has a one-line reason in [notebooks/results/labels.csv](notebooks/results/labels.csv), so you can check or relabel them.
- The repos were read a few minutes after the scan, at their newest commit.
- Raw data is in [notebooks/results/](notebooks/results/).

**Main false-positive causes, to fix next:**
- **MNA008 stale-command:** commands in a nested AGENTS.md are checked against the root `package.json`, not the package the file lives in. Paths and `<placeholder>` arguments are also treated as commands.
- **MNA004 conflicting-instructions:** `npm install -g pnpm` and `npm run <script>` are read as choosing npm. A "never X" inside a command line is ignored.
- **MNA002 lint-leakage:** keywords are matched out of context, e.g. a commit-message length limit is taken as code line length. A rule also counts as enforced when the tool is configured but doesn't enforce that rule, or only warns.
- **MNA006 blind-reference:** it flags table-of-contents anchors and inline code that isn't a pointer, and ignores triggers given in a lead-in sentence or heading.
- **MNA007 broken-reference:** paths written relative to a sub-project root, paths in the user's own project, and other repos are reported as missing.
- **MNA013 unloaded-file:** Copilot `.instructions.md` files with only a `description` are reported as never loaded, but VS Code loads them on demand. Nested AGENTS.md files are flagged even when an always-loaded file tells the agent to read them.
- **MNA010 cross-file-divergence:** a CLAUDE.md that tells the agent in prose to read AGENTS.md (rather than importing it) counts as divergence.
- **MNA003 skill-leakage:** any numbered list of 8+ items is flagged, as are generic "new ..." headings.

---

## Contributing

```bash
make fmt && make lint
```

Open an issue before large changes. [CONTRIBUTING.md](CONTRIBUTING.md) covers setup, snapshots, how to add a rule and how to add a promotion.

---

## License

MIT. See [LICENSE](LICENSE).
