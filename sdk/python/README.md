# mnemoa-sdk

Python client for [mnemoa](https://github.com/Flotrpy/mnemoa).

- `Client` / `AsyncClient` talk to a running `mnemoa-api` over HTTP (httpx).
- `CLI` runs the local `mnemoa` binary and parses its JSON output, no server needed.

```bash
pip install mnemoa-sdk          # once published; until then:
pip install -e sdk/python
```

## Sync

```python
from mnemoa import Client

with Client("http://localhost:8080") as client:
    report = client.scan("repo")             # paths are relative to the server's MNEMOA_ROOT
    for f in report.findings:
        print(f.path, f.line, f.rule, f.message)
    plan = client.forget("repo")              # dry run
    for t in plan["tokens"]:
        print(t["target"], t["before"], "->", t["after"])
```

## Async

```python
import asyncio
from mnemoa import AsyncClient

async def main():
    async with AsyncClient("http://localhost:8080") as client:
        budget = await client.budget("repo", targets=["claude-code", "codex"])
        print(budget["targets"])

asyncio.run(main())
```

## Local binary

```python
from mnemoa import CLI

report = CLI().scan(".")    # uses $MNEMOA_BIN or `mnemoa` on PATH
print(report.ok, report.summary)
```

## Methods

| Method | Endpoint | Returns |
|---|---|---|
| `health()` | `GET /health` | dict |
| `scan(path, targets=, rules=, strict=)` | `POST /scan` | `ScanReport` |
| `budget(path, targets=)` | `POST /budget` | dict |
| `drift(path, base=)` | `POST /drift` | dict |
| `promote(path, llm=, apply=)` | `POST /promote` | dict |
| `forget(path, apply=)` | `POST /forget` | dict |
| `rules()` / `rule(id)` | `GET /rules`, `GET /rules/{id}` | list / dict |
| `stats()` | `GET /stats` | dict |

Errors raise `APIError` (with `status_code` and `message`) or `CLIError`.

## Tests

```bash
cd sdk/python
pip install -e '.[test]'
pytest -q        # integration tests run when target/debug/mnemoa and mnemoa-api are built
```
