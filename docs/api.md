# HTTP API

`mnemoa-api` serves core over HTTP for CI bots and editors.

```bash
MNEMOA_ROOT=/srv/repos MNEMOA_PORT=8080 mnemoa-api
```

## Security

- Every `path` is resolved against `MNEMOA_ROOT`, canonicalized (symlinks followed) and must stay inside it. Paths with `..`, absolute paths outside the root and symlinks that lead out are refused with `403`.
- `apply: true` is refused with `403` unless `MNEMOA_API_ALLOW_WRITE=true`.
- `llm: true` needs a server built with `--features llm` and `MNEMOA_LLM_ENABLED=true`; otherwise `400`.
- Each request uses the scanned repo's `.mnemoa.toml`, then the server's `MNEMOA_*` environment.

Errors are `{"error": "<message>"}` with status `400` (bad input), `403` (outside the root, or writes disabled), `404` (unknown rule) or `500`.

## Endpoints

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/health` | none | `HealthResponse` |
| POST | `/scan` | `{path, targets?, rules?, strict?, history?}` | `ScanReport` |
| POST | `/budget` | `{path, targets?}` | `BudgetReport` |
| POST | `/drift` | `{path, base?}` | `DriftReport` |
| POST | `/promote` | `{path, llm?, apply?}` | `PromotionPlan` + `written` |
| POST | `/forget` | `{path, apply?}` | `ForgetPlan` + `written` |
| GET | `/rules` | none | `RuleInfo[]` |
| GET | `/rules/{id}` | none | `RuleInfo` |
| GET | `/stats` | none | `StatsResponse` |

`path` defaults to `"."` (the root itself).

### GET /health

```json
{
  "status": "ok",
  "version": "0.1.1",
  "config_source": "defaults+env:MNEMOA_PORT,MNEMOA_ROOT",
  "root": "/srv/repos",
  "allow_write": false,
  "llm": { "enabled": false, "compiled": false, "provider": "ollama", "model": "qwen2.5-coder:7b" }
}
```

### POST /scan

```json
// request
{ "path": "my-repo", "targets": ["claude-code", "codex"], "rules": ["MNA007"], "strict": true }
// response
{
  "version": "0.1.1",
  "root": "/srv/repos/my-repo",
  "targets": ["claude-code", "codex"],
  "config_source": "file:/srv/repos/my-repo/.mnemoa.toml",
  "strict": true,
  "findings": [{
    "rule": "MNA007", "name": "broken-reference", "severity": "error",
    "message": "Link target `docs/setup.md` does not exist.",
    "path": "AGENTS.md", "line": 6, "column": 8, "end_line": 6, "end_column": 30,
    "snippet": "- Read [setup](docs/setup.md) before onboarding.",
    "help": "Fix the path or remove the link.",
    "fingerprint": "5f0c2d6b1e9a4c37"
  }],
  "summary": { "errors": 1, "warnings": 0, "infos": 0 },
  "baselined": 0,
  "files": ["AGENTS.md", "CLAUDE.md"]
}
```

Send `"history": false` to skip git history (and with it MNA005), as `scan --no-history` does locally.

### POST /budget

```json
// request
{ "path": "my-repo", "targets": ["codex"] }
// response
{
  "tokenizer": "o200k_base estimate",
  "targets": [{
    "target": "codex",
    "always": [{ "path": "AGENTS.md", "lines": 120, "bytes": 6400, "tokens": 1500 }],
    "total_lines": 120, "total_bytes": 6400, "total_tokens": 1500,
    "on_demand": [{ "path": "pkg/AGENTS.md", "trigger": "when Codex is started in `pkg/` or below", "tokens": 300 }],
    "notes": ["Codex is assumed not to strip HTML comments (unverified)"]
  }],
  "token_budget": 2000
}
```

`dropped_bytes` appears for Codex when the always-loaded set passes the 32 KiB cap. `token_budget` is the repository's `rules.token_budget`; `mnemoa --server … budget --check` fails when a target exceeds it or drops bytes.

### POST /drift

```json
// request
{ "path": "my-repo", "base": "origin/main" }
// response
{
  "version": "0.1.1", "root": "/srv/repos/my-repo", "base": "origin/main", "history": true,
  "items": [{
    "kind": "renamed-in-diff", "severity": "error", "path": "AGENTS.md", "line": 4, "column": 10,
    "reference": "src/old.rs", "message": "`src/old.rs` was renamed to `src/new.rs` since origin/main."
  }],
  "freshness": [{ "path": "AGENTS.md", "last_commit": "…", "commits_since": 3, "references": ["src/new.rs"] }],
  "summary": { "errors": 1, "warnings": 0, "infos": 0 },
  "strict": false
}
```

`strict` is the repository's `scan.strict`; when true, warnings fail `mnemoa --server … drift` too.

Drift kinds: `missing-path`, `missing-symbol`, `stale-command`, `removed-in-diff`, `renamed-in-diff`, `symbol-removed-in-diff`, `done-task-missing-file`, `open-task-file-changed`, `malformed-task`, `stale-memory-bank`.

### POST /promote

```json
// request
{ "path": "my-repo", "llm": false, "apply": false }
// response
{
  "promotions": [{
    "id": "indentation", "source": "AGENTS.md", "line": 4, "rule": "Use 2 spaces for indentation.",
    "hash": "be78bdfb73c0abe1", "topic": "indentation", "enforcer": "editorconfig", "status": "emit",
    "artifacts": [{ "path": ".editorconfig", "action": "create", "content": "root = true\n…", "note": "indent_style = space, indent_size = 2" }],
    "removes_prose": true, "retire_when": "check:indentation"
  }],
  "unmatched": [{ "source": "AGENTS.md", "line": 9, "rule": "Never edit `schema.sql` by hand." }],
  "diff": "--- /dev/null\n+++ b/.editorconfig\n…",
  "files_changed": [".editorconfig", ".mnemoa/ledger.toml", "AGENTS.md"],
  "written": []
}
```

`status` is `already-enforced`, `emit` or `drafted`. With `llm: true`, unmatched rules may carry a `draft` with its validation result.

### POST /forget

```json
// request
{ "path": "my-repo", "apply": false }
// response
{
  "actions": [{ "kind": "move", "path": "AGENTS.md", "line": 25, "summary": "move \"Adding a new endpoint\" to docs/agents/adding-a-new-endpoint.md" }],
  "tokens": [{ "target": "codex", "before": 153, "after": 79 }],
  "tokenizer": "o200k_base estimate",
  "before": { "…": "BudgetReport" },
  "after": { "…": "BudgetReport" },
  "diff": "…",
  "files_changed": ["AGENTS.md", "docs/agents/adding-a-new-endpoint.md"],
  "written": []
}
```

Action kinds: `delete`, `move`, `promote`, `retire`, `fix-reference`, `add-pointer`, `import`.

### GET /rules, GET /rules/{id}

```json
{
  "id": "MNA006", "name": "blind-reference",
  "summary": "A link or path with no word on when or why to read it.",
  "rationale": "…", "link": "https://arxiv.org/abs/2606.15828",
  "default_severity": "info", "fixable": true, "opt_in": false, "explain": "…"
}
```

### GET /stats

```json
{ "requests": 42, "scans": 7, "findings_by_rule": { "MNA007": 3, "MNA006": 5 }, "uptime_secs": 3600 }
```

## Clients

- CLI: `mnemoa --server http://localhost:8080 scan my-repo`
- Python: `mnemoa.Client("http://localhost:8080").scan("my-repo")`; see `sdk/python/README.md`.
