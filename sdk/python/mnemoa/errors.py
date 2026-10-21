"""Errors raised by the mnemoa SDK."""

from __future__ import annotations


class MnemoaError(Exception):
    """Base class for SDK errors."""


class APIError(MnemoaError):
    """The API returned a non-2xx response."""

    def __init__(self, status_code: int, message: str) -> None:
        super().__init__(f"mnemoa-api returned {status_code}: {message}")
        self.status_code = status_code
        self.message = message


class CLIError(MnemoaError):
    """The mnemoa binary failed (exit code 2) or printed something that is not JSON."""

    def __init__(self, returncode: int, stderr: str) -> None:
        super().__init__(f"mnemoa exited with {returncode}: {stderr.strip()}")
        self.returncode = returncode
        self.stderr = stderr
