//! Findings (diagnostics) reported by rules.

use std::fmt;
use std::ops::Range;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::doc::Doc;

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    #[serde(alias = "warning")]
    Warn,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Error => "error",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Severity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "info" | "note" => Ok(Severity::Info),
            "warn" | "warning" => Ok(Severity::Warn),
            "error" => Ok(Severity::Error),
            other => Err(format!(
                "unknown severity `{other}` (expected info, warn or error)"
            )),
        }
    }
}

/// One problem found by a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub rule: String,
    pub name: String,
    pub severity: Severity,
    pub message: String,
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    /// The first line of the flagged text, trimmed.
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Stable identity used by baselines: rule, path and normalized snippet.
    pub fingerprint: String,
    /// Byte range in the document, when the finding points at text.
    #[serde(skip)]
    pub span: Option<Range<usize>>,
}

impl Finding {
    /// A finding pointing at `span` in `doc`.
    pub fn at(
        rule: &str,
        name: &str,
        severity: Severity,
        doc: &Doc,
        span: Range<usize>,
        message: impl Into<String>,
    ) -> Finding {
        let (line, column) = doc.line_col(span.start);
        let end = span.end.max(span.start + 1).min(doc.text.len().max(1));
        let (end_line, end_column) = doc.line_col(end.min(doc.text.len()));
        let snippet = doc
            .text
            .get(span.clone())
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let mut f = Finding::new(rule, name, severity, &doc.path, message)
            .with_position(line, column, end_line, end_column)
            .with_snippet(snippet);
        f.span = Some(span);
        f
    }

    /// A finding on a whole file (line 1).
    pub fn new(
        rule: &str,
        name: &str,
        severity: Severity,
        path: &str,
        message: impl Into<String>,
    ) -> Finding {
        let mut f = Finding {
            rule: rule.to_string(),
            name: name.to_string(),
            severity,
            message: message.into(),
            path: path.to_string(),
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 1,
            snippet: String::new(),
            help: None,
            fingerprint: String::new(),
            span: None,
        };
        f.refresh_fingerprint();
        f
    }

    pub fn with_position(
        mut self,
        line: usize,
        column: usize,
        end_line: usize,
        end_column: usize,
    ) -> Self {
        self.line = line;
        self.column = column;
        self.end_line = end_line.max(line);
        self.end_column = if self.end_line == line {
            end_column.max(column)
        } else {
            end_column
        };
        self
    }

    pub fn with_snippet(mut self, snippet: impl Into<String>) -> Self {
        self.snippet = snippet.into();
        self.refresh_fingerprint();
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    fn refresh_fingerprint(&mut self) {
        let norm: String = self
            .snippet
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let key = if norm.is_empty() {
            format!("{}|{}|{}", self.rule, self.path, self.message)
        } else {
            format!("{}|{}|{}", self.rule, self.path, norm)
        };
        self.fingerprint = format!("{:016x}", fnv1a(key.as_bytes()));
    }
}

/// 64-bit FNV-1a hash. Stable across runs and platforms.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Counts of findings by severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
}

impl Summary {
    pub fn of(findings: &[Finding]) -> Summary {
        let mut s = Summary::default();
        for f in findings {
            match f.severity {
                Severity::Error => s.errors += 1,
                Severity::Warn => s.warnings += 1,
                Severity::Info => s.infos += 1,
            }
        }
        s
    }

    pub fn total(&self) -> usize {
        self.errors + self.warnings + self.infos
    }
}

/// Sort findings by path, line, column, rule.
pub fn sort(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        (a.path.as_str(), a.line, a.column, a.rule.as_str()).cmp(&(
            b.path.as_str(),
            b.line,
            b.column,
            b.rule.as_str(),
        ))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_positions_and_snippet() {
        let doc = Doc::parse("AGENTS.md", "# T\n\n- Use npm.\n");
        let f = Finding::at("MNA004", "x", Severity::Info, &doc, 5..15, "msg");
        assert_eq!((f.line, f.column), (3, 1));
        assert_eq!(f.snippet, "- Use npm.");
        assert_eq!(f.fingerprint.len(), 16);
    }

    #[test]
    fn fingerprint_ignores_position_and_whitespace() {
        let a = Doc::parse("A.md", "- Use   npm.\n");
        let b = Doc::parse("A.md", "\n\n\n- Use npm.\n");
        let fa = Finding::at("R", "r", Severity::Warn, &a, 0..12, "m");
        let fb = Finding::at("R", "r", Severity::Warn, &b, 3..13, "m");
        assert_eq!(fa.fingerprint, fb.fingerprint);
    }

    #[test]
    fn severity_parsing_and_order() {
        assert_eq!("warning".parse::<Severity>().unwrap(), Severity::Warn);
        assert!(Severity::Error > Severity::Warn && Severity::Warn > Severity::Info);
        assert!("fatal".parse::<Severity>().is_err());
    }

    #[test]
    fn summary_counts() {
        let doc = Doc::parse("a.md", "x\n");
        let fs = vec![
            Finding::at("A", "a", Severity::Error, &doc, 0..1, "m"),
            Finding::at("B", "b", Severity::Info, &doc, 0..1, "m"),
        ];
        let s = Summary::of(&fs);
        assert_eq!((s.errors, s.warnings, s.infos, s.total()), (1, 0, 1, 2));
    }
}
