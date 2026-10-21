"""Run the local ``mnemoa`` binary and parse its JSON output (no server needed)."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from typing import Any, Dict, List, Optional, Sequence

from .errors import CLIError, MnemoaError
from .models import ScanReport


class CLI:
    """Wrapper around the ``mnemoa`` command.

    ``binary`` defaults to ``$MNEMOA_BIN`` or ``mnemoa`` on ``PATH``.
    """

    def __init__(self, binary: Optional[str] = None) -> None:
        found = binary or os.environ.get("MNEMOA_BIN") or shutil.which("mnemoa")
        if not found:
            raise MnemoaError("mnemoa binary not found; install it or set MNEMOA_BIN")
        self.binary = found

    def _run(self, args: Sequence[str]) -> Any:
        proc = subprocess.run([self.binary, *args], capture_output=True, text=True, check=False)
        # Exit 1 means "findings over threshold"; the JSON is still valid.
        if proc.returncode not in (0, 1):
            raise CLIError(proc.returncode, proc.stderr)
        try:
            return json.loads(proc.stdout)
        except json.JSONDecodeError as e:
            raise CLIError(proc.returncode, f"invalid JSON output: {e}: {proc.stderr}") from e

    @staticmethod
    def _targets(targets: Optional[List[str]]) -> List[str]:
        return ["--targets", ",".join(targets)] if targets else []

    def scan(
        self,
        path: str = ".",
        *,
        targets: Optional[List[str]] = None,
        rules: Optional[List[str]] = None,
        strict: bool = False,
        history: bool = True,
    ) -> ScanReport:
        args = ["scan", path, "--format", "json", *self._targets(targets)]
        if rules:
            args += ["--rules", ",".join(rules)]
        if strict:
            args.append("--strict")
        if not history:
            args.append("--no-history")
        return ScanReport.from_dict(self._run(args))

    def budget(self, path: str = ".", *, targets: Optional[List[str]] = None) -> Dict[str, Any]:
        return self._run(["budget", path, "--format", "json", *self._targets(targets)])

    def drift(self, path: str = ".", *, base: Optional[str] = None) -> Dict[str, Any]:
        args = ["drift", path, "--format", "json"]
        if base:
            args += ["--base", base]
        return self._run(args)

    def promote(self, path: str = ".", *, apply: bool = False) -> Dict[str, Any]:
        return self._run(["promote", path, "--format", "json", *(["--apply"] if apply else [])])

    def forget(self, path: str = ".", *, apply: bool = False) -> Dict[str, Any]:
        return self._run(["forget", path, "--format", "json", *(["--apply", "--yes"] if apply else [])])

    def rules(self) -> List[Dict[str, Any]]:
        return self._run(["rules", "--format", "json"])
