"""Python SDK for mnemoa: keep repo memory for coding agents small, true and enforced.

Use :class:`Client` or :class:`AsyncClient` against a running ``mnemoa-api``, or :class:`CLI`
to run the local ``mnemoa`` binary.
"""

from .cli import CLI
from .client import AsyncClient, Client
from .errors import APIError, CLIError, MnemoaError
from .models import Finding, ScanReport, Summary

__all__ = [
    "APIError",
    "AsyncClient",
    "CLI",
    "CLIError",
    "Client",
    "Finding",
    "MnemoaError",
    "ScanReport",
    "Summary",
]
__version__ = "0.1.1"
