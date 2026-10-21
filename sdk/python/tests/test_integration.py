import asyncio

import pytest

from mnemoa import APIError, AsyncClient, Client


def test_against_real_server(server):
    with Client(server) as c:
        assert c.health()["status"] == "ok"
        r = c.scan("repo")
        assert r.summary.errors == 1
        assert c.drift("repo")["items"][0]["kind"] == "missing-path"
        assert c.rule("MNA006")["name"] == "blind-reference"
        with pytest.raises(APIError) as e:
            c.scan("../")
        assert e.value.status_code == 403
        with pytest.raises(APIError):
            c.forget("repo", apply=True)
        assert c.stats()["scans"] == 1


def test_async_against_real_server(server):
    async def go():
        async with AsyncClient(server) as c:
            return await c.budget("repo", targets=["codex"])

    b = asyncio.run(go())
    assert b["targets"][0]["target"] == "codex"
