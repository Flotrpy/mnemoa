import asyncio
import json

import httpx
import pytest

from mnemoa import APIError, AsyncClient, Client, ScanReport

SCAN = {
    "version": "0.1.0",
    "root": "/w/repo",
    "targets": ["claude-code"],
    "config_source": "defaults",
    "strict": False,
    "findings": [
        {
            "rule": "MNA007",
            "name": "broken-reference",
            "severity": "error",
            "message": "Link target `x.md` does not exist.",
            "path": "AGENTS.md",
            "line": 1,
            "column": 8,
            "end_line": 1,
            "end_column": 17,
            "snippet": "- [x](x.md)",
            "fingerprint": "abc",
        }
    ],
    "summary": {"errors": 1, "warnings": 0, "infos": 0},
    "baselined": 0,
    "files": ["AGENTS.md"],
}


def handler(request: httpx.Request) -> httpx.Response:
    if request.url.path == "/scan":
        body = json.loads(request.content)
        assert body["path"] == "repo"
        assert "rules" not in body  # None values are dropped
        return httpx.Response(200, json=SCAN)
    if request.url.path == "/health":
        return httpx.Response(200, json={"status": "ok", "version": "0.1.0"})
    if request.url.path == "/promote":
        body = json.loads(request.content)
        if body.get("apply"):
            return httpx.Response(403, json={"error": "apply is disabled"})
        return httpx.Response(200, json={"promotions": [], "unmatched": [], "written": []})
    if request.url.path == "/rules/MNA999":
        return httpx.Response(404, json={"error": "unknown rule `MNA999`"})
    return httpx.Response(404, text="not found")


def test_sync_scan_returns_typed_report():
    with Client("http://test", transport=httpx.MockTransport(handler)) as c:
        r = c.scan("repo", targets=["claude-code"])
        assert isinstance(r, ScanReport)
        assert r.summary.errors == 1 and not r.ok
        assert r.findings[0].rule == "MNA007"
        assert r.findings[0].line == 1
        assert c.health()["status"] == "ok"


def test_errors_carry_status_and_message():
    with Client("http://test", transport=httpx.MockTransport(handler)) as c:
        with pytest.raises(APIError) as e:
            c.promote("repo", apply=True)
        assert e.value.status_code == 403
        assert "apply is disabled" in e.value.message
        with pytest.raises(APIError) as e:
            c.rule("MNA999")
        assert e.value.status_code == 404


def test_async_client():
    async def go():
        async with AsyncClient("http://test", transport=httpx.MockTransport(handler)) as c:
            r = await c.scan("repo")
            plan = await c.promote("repo")
            return r, plan

    r, plan = asyncio.run(go())
    assert r.summary.errors == 1
    assert plan["written"] == []
