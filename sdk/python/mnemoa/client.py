"""HTTP clients for mnemoa-api."""

from __future__ import annotations

from typing import Any, Dict, List, Optional

import httpx

from .errors import APIError
from .models import ScanReport

DEFAULT_URL = "http://localhost:8080"
DEFAULT_TIMEOUT = 600.0


def _drop_none(d: Dict[str, Any]) -> Dict[str, Any]:
    return {k: v for k, v in d.items() if v is not None}


def _check(resp: httpx.Response) -> Any:
    if resp.is_success:
        return resp.json()
    try:
        message = resp.json().get("error", resp.text)
    except ValueError:
        message = resp.text
    raise APIError(resp.status_code, message)


class Client:
    """Synchronous client.

    Paths are relative to the server's ``MNEMOA_ROOT``.
    """

    def __init__(
        self,
        base_url: str = DEFAULT_URL,
        *,
        timeout: float = DEFAULT_TIMEOUT,
        transport: Optional[httpx.BaseTransport] = None,
    ) -> None:
        self._http = httpx.Client(base_url=base_url, timeout=timeout, transport=transport)

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> "Client":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def _get(self, path: str) -> Any:
        return _check(self._http.get(path))

    def _post(self, path: str, body: Dict[str, Any]) -> Any:
        return _check(self._http.post(path, json=_drop_none(body)))

    def health(self) -> Dict[str, Any]:
        return self._get("/health")

    def scan(
        self,
        path: str = ".",
        *,
        targets: Optional[List[str]] = None,
        rules: Optional[List[str]] = None,
        strict: Optional[bool] = None,
    ) -> ScanReport:
        return ScanReport.from_dict(
            self._post("/scan", {"path": path, "targets": targets, "rules": rules, "strict": strict})
        )

    def budget(self, path: str = ".", *, targets: Optional[List[str]] = None) -> Dict[str, Any]:
        return self._post("/budget", {"path": path, "targets": targets})

    def drift(self, path: str = ".", *, base: Optional[str] = None) -> Dict[str, Any]:
        return self._post("/drift", {"path": path, "base": base})

    def promote(self, path: str = ".", *, llm: bool = False, apply: bool = False) -> Dict[str, Any]:
        return self._post("/promote", {"path": path, "llm": llm, "apply": apply})

    def forget(self, path: str = ".", *, apply: bool = False) -> Dict[str, Any]:
        return self._post("/forget", {"path": path, "apply": apply})

    def rules(self) -> List[Dict[str, Any]]:
        return self._get("/rules")

    def rule(self, rule_id: str) -> Dict[str, Any]:
        return self._get(f"/rules/{rule_id}")

    def stats(self) -> Dict[str, Any]:
        return self._get("/stats")


class AsyncClient:
    """Asynchronous client with the same methods as :class:`Client`."""

    def __init__(
        self,
        base_url: str = DEFAULT_URL,
        *,
        timeout: float = DEFAULT_TIMEOUT,
        transport: Optional[httpx.AsyncBaseTransport] = None,
    ) -> None:
        self._http = httpx.AsyncClient(base_url=base_url, timeout=timeout, transport=transport)

    async def aclose(self) -> None:
        await self._http.aclose()

    async def __aenter__(self) -> "AsyncClient":
        return self

    async def __aexit__(self, *exc: object) -> None:
        await self.aclose()

    async def _get(self, path: str) -> Any:
        return _check(await self._http.get(path))

    async def _post(self, path: str, body: Dict[str, Any]) -> Any:
        return _check(await self._http.post(path, json=_drop_none(body)))

    async def health(self) -> Dict[str, Any]:
        return await self._get("/health")

    async def scan(
        self,
        path: str = ".",
        *,
        targets: Optional[List[str]] = None,
        rules: Optional[List[str]] = None,
        strict: Optional[bool] = None,
    ) -> ScanReport:
        return ScanReport.from_dict(
            await self._post("/scan", {"path": path, "targets": targets, "rules": rules, "strict": strict})
        )

    async def budget(self, path: str = ".", *, targets: Optional[List[str]] = None) -> Dict[str, Any]:
        return await self._post("/budget", {"path": path, "targets": targets})

    async def drift(self, path: str = ".", *, base: Optional[str] = None) -> Dict[str, Any]:
        return await self._post("/drift", {"path": path, "base": base})

    async def promote(self, path: str = ".", *, llm: bool = False, apply: bool = False) -> Dict[str, Any]:
        return await self._post("/promote", {"path": path, "llm": llm, "apply": apply})

    async def forget(self, path: str = ".", *, apply: bool = False) -> Dict[str, Any]:
        return await self._post("/forget", {"path": path, "apply": apply})

    async def rules(self) -> List[Dict[str, Any]]:
        return await self._get("/rules")

    async def rule(self, rule_id: str) -> Dict[str, Any]:
        return await self._get(f"/rules/{rule_id}")

    async def stats(self) -> Dict[str, Any]:
        return await self._get("/stats")
