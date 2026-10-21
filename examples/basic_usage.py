"""Basic mnemoa SDK usage.

Run a server first, for example:
    MNEMOA_ROOT=$(pwd) cargo run -p mnemoa-api
then:
    python examples/basic_usage.py [path-relative-to-root]
"""

import asyncio
import sys

from mnemoa import APIError, AsyncClient, Client

URL = "http://localhost:8080"


def sync_demo(path: str) -> None:
    with Client(URL) as client:
        health = client.health()
        print(f"mnemoa-api {health['version']} (config: {health['config_source']})")

        report = client.scan(path)
        s = report.summary
        print(f"scan: {s.errors} error(s), {s.warnings} warning(s), {s.infos} info")
        for f in report.findings[:10]:
            print(f"  {f.path}:{f.line} {f.rule} {f.message}")

        plan = client.forget(path)
        for t in plan["tokens"]:
            print(f"  {t['target']}: {t['before']} -> {t['after']} always-loaded tokens")

        try:
            client.forget(path, apply=True)
        except APIError as e:
            print(f"apply refused as expected: {e.message}")


async def async_demo(path: str) -> None:
    async with AsyncClient(URL) as client:
        budget, drift = await asyncio.gather(client.budget(path), client.drift(path))
        for t in budget["targets"]:
            print(f"budget {t['target']}: {t['total_tokens']} tokens ({budget['tokenizer']})")
        print(f"drift items: {len(drift['items'])}")


if __name__ == "__main__":
    target = sys.argv[1] if len(sys.argv) > 1 else "."
    sync_demo(target)
    asyncio.run(async_demo(target))
