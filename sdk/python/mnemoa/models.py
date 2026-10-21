"""Typed views of the main responses. Other endpoints return plain dicts."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


@dataclass
class Finding:
    rule: str
    name: str
    severity: str
    message: str
    path: str
    line: int
    column: int
    end_line: int
    end_column: int
    snippet: str
    fingerprint: str
    help: Optional[str] = None

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "Finding":
        return cls(
            rule=d["rule"],
            name=d["name"],
            severity=d["severity"],
            message=d["message"],
            path=d["path"],
            line=d["line"],
            column=d["column"],
            end_line=d["end_line"],
            end_column=d["end_column"],
            snippet=d.get("snippet", ""),
            fingerprint=d.get("fingerprint", ""),
            help=d.get("help"),
        )


@dataclass
class Summary:
    errors: int = 0
    warnings: int = 0
    infos: int = 0

    @property
    def total(self) -> int:
        return self.errors + self.warnings + self.infos


@dataclass
class ScanReport:
    version: str
    root: str
    targets: List[str]
    findings: List[Finding]
    summary: Summary
    strict: bool = False
    baselined: int = 0
    files: List[str] = field(default_factory=list)
    config_source: str = ""
    raw: Dict[str, Any] = field(default_factory=dict, repr=False)

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "ScanReport":
        return cls(
            version=d["version"],
            root=d["root"],
            targets=list(d.get("targets", [])),
            findings=[Finding.from_dict(f) for f in d.get("findings", [])],
            summary=Summary(**d.get("summary", {})),
            strict=d.get("strict", False),
            baselined=d.get("baselined", 0),
            files=list(d.get("files", [])),
            config_source=d.get("config_source", ""),
            raw=d,
        )

    @property
    def ok(self) -> bool:
        """True when the scan would exit 0 (no errors, and no warnings in strict mode)."""
        return self.summary.errors == 0 and not (self.strict and self.summary.warnings > 0)
