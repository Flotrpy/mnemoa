//! Rationale comments: why a rule exists and when it can be retired.
//!
//! ```markdown
//! <!-- why: CI failed on unformatted imports | seen: 4x in 2 weeks | outcome: zero since | retire-when: check:fmt-imports -->
//! - Always run `make lint` before committing.
//! ```
//!
//! Claude Code strips block-level HTML comments before the model sees them, so these cost no
//! context there. For other targets the same fields can live in `.mnemoa/rationale.toml`.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::diag::fnv1a;
use crate::doc::{Doc, Rule};

/// Sidecar file for rationale that should not sit in the instruction file.
pub const SIDECAR: &str = ".mnemoa/rationale.toml";

/// When a rule may be deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum RetireWhen {
    /// A check with this ledger id exists and passes.
    Check(String),
    /// This path no longer exists.
    PathGone(String),
    /// On or after this date (YYYY-MM-DD).
    Date(String),
    /// Only by a person.
    Manual,
}

impl fmt::Display for RetireWhen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RetireWhen::Check(id) => write!(f, "check:{id}"),
            RetireWhen::PathGone(p) => write!(f, "path-gone:{p}"),
            RetireWhen::Date(d) => write!(f, "date:{d}"),
            RetireWhen::Manual => f.write_str("manual"),
        }
    }
}

impl From<RetireWhen> for String {
    fn from(r: RetireWhen) -> String {
        r.to_string()
    }
}

impl TryFrom<String> for RetireWhen {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

fn valid_date(d: &str) -> bool {
    let parts: Vec<&str> = d.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return false;
    }
    let nums: Option<Vec<u32>> = parts.iter().map(|p| p.parse().ok()).collect();
    matches!(nums.as_deref(), Some([_, m, d]) if (1..=12).contains(m) && (1..=31).contains(d))
}

impl FromStr for RetireWhen {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("manual") {
            return Ok(RetireWhen::Manual);
        }
        let (kind, arg) = s.split_once(':').ok_or_else(|| {
            format!("expected check:<id>, path-gone:<path>, date:<YYYY-MM-DD> or manual, got `{s}`")
        })?;
        let arg = arg.trim();
        if arg.is_empty() {
            return Err(format!("`{kind}:` needs a value"));
        }
        match kind.trim() {
            "check" => Ok(RetireWhen::Check(arg.to_string())),
            "path-gone" => Ok(RetireWhen::PathGone(arg.trim_end_matches('/').to_string())),
            "date" if valid_date(arg) => Ok(RetireWhen::Date(arg.to_string())),
            "date" => Err(format!("`{arg}` is not a YYYY-MM-DD date")),
            other => Err(format!("unknown retire condition `{other}`")),
        }
    }
}

/// Parsed rationale.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rationale {
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(
        default,
        rename = "retire-when",
        alias = "retire_when",
        skip_serializing_if = "Option::is_none"
    )]
    pub retire_when: Option<RetireWhen>,
    /// Problems found while parsing (unknown keys, bad retire condition).
    #[serde(skip)]
    pub errors: Vec<String>,
}

/// Parse a comment body such as `why: x | seen: y | retire-when: manual`.
/// Returns `None` when there is no `why:` field.
pub fn parse(body: &str) -> Option<Rationale> {
    let mut r = Rationale::default();
    let mut has_why = false;
    for part in body.split('|') {
        let Some((k, v)) = part.split_once(':') else {
            continue;
        };
        let v = v.trim().to_string();
        match k.trim().to_ascii_lowercase().as_str() {
            "why" => {
                has_why = true;
                r.why = v;
            }
            "seen" => r.seen = Some(v),
            "outcome" => r.outcome = Some(v),
            "retire-when" | "retire_when" | "retire" => match v.parse() {
                Ok(c) => r.retire_when = Some(c),
                Err(e) => r.errors.push(e),
            },
            other => r.errors.push(format!("unknown field `{other}`")),
        }
    }
    has_why.then_some(r)
}

/// Stable hash of a rule's text, used by the sidecar and the ledger.
pub fn rule_hash(text: &str) -> String {
    let norm: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    format!("{:016x}", fnv1a(norm.as_bytes()))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SidecarFile {
    #[serde(default)]
    rule: Vec<SidecarEntry>,
}

/// One entry of `.mnemoa/rationale.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarEntry {
    /// [`rule_hash`] of the rule text.
    pub hash: String,
    /// The rule text, for humans.
    #[serde(default)]
    pub text: String,
    #[serde(flatten)]
    pub rationale: Rationale,
}

/// Rationale kept outside instruction files, keyed by rule hash.
#[derive(Debug, Clone, Default)]
pub struct Sidecar {
    pub entries: BTreeMap<String, SidecarEntry>,
}

impl Sidecar {
    pub fn parse(text: &str) -> Result<Sidecar, String> {
        let f: SidecarFile = toml::from_str(text).map_err(|e| e.to_string())?;
        Ok(Sidecar {
            entries: f.rule.into_iter().map(|e| (e.hash.clone(), e)).collect(),
        })
    }

    pub fn to_toml(&self) -> String {
        let f = SidecarFile {
            rule: self.entries.values().cloned().collect(),
        };
        toml::to_string_pretty(&f).unwrap_or_default()
    }

    pub fn get(&self, rule_text: &str) -> Option<&Rationale> {
        self.entries
            .get(&rule_hash(rule_text))
            .map(|e| &e.rationale)
    }
}

/// The rationale for `rule`: a comment directly above it, else a sidecar entry.
pub fn rationale_for(doc: &Doc, rule: &Rule, sidecar: Option<&Sidecar>) -> Option<Rationale> {
    if let Some(c) = doc.comment_above(rule.span.start)
        && let Some(r) = parse(&c.body)
    {
        return Some(r);
    }
    sidecar.and_then(|s| s.get(&rule.text)).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_fields() {
        let r = parse("why: CI failed on imports | seen: 4x in 2 weeks | outcome: zero since | retire-when: check:fmt-imports").unwrap();
        assert_eq!(r.why, "CI failed on imports");
        assert_eq!(r.seen.as_deref(), Some("4x in 2 weeks"));
        assert_eq!(r.outcome.as_deref(), Some("zero since"));
        assert_eq!(r.retire_when, Some(RetireWhen::Check("fmt-imports".into())));
        assert!(r.errors.is_empty());
    }

    #[test]
    fn non_rationale_comment_is_none() {
        assert!(parse("TODO: tidy this up").is_none());
        assert!(parse("just a note").is_none());
    }

    #[test]
    fn retire_conditions() {
        assert_eq!(
            "path-gone:src/legacy/".parse(),
            Ok(RetireWhen::PathGone("src/legacy".into()))
        );
        assert_eq!(
            "date:2027-01-31".parse(),
            Ok(RetireWhen::Date("2027-01-31".into()))
        );
        assert_eq!("manual".parse(), Ok(RetireWhen::Manual));
        assert!("date:2027-13-01".parse::<RetireWhen>().is_err());
        assert!("soon".parse::<RetireWhen>().is_err());
        assert!("check:".parse::<RetireWhen>().is_err());
        let r = parse("why: x | retire-when: whenever | color: blue").unwrap();
        assert_eq!(r.errors.len(), 2);
    }

    #[test]
    fn hash_ignores_case_and_spacing() {
        assert_eq!(rule_hash("- Use   pnpm."), rule_hash("- use pnpm."));
        assert_ne!(rule_hash("Use pnpm."), rule_hash("Use npm."));
    }

    #[test]
    fn rationale_from_comment_or_sidecar() {
        let doc = Doc::parse(
            "AGENTS.md",
            "<!-- why: broke prod | retire-when: manual -->\n- Never run migrations by hand.\n- Use pnpm.\n",
        );
        let first = &doc.rules[0];
        assert_eq!(rationale_for(&doc, first, None).unwrap().why, "broke prod");
        let second = &doc.rules[1];
        assert!(rationale_for(&doc, second, None).is_none());
        let sidecar = Sidecar::parse(&format!(
            "[[rule]]\nhash = \"{}\"\ntext = \"Use pnpm.\"\nwhy = \"lockfile churn\"\nretire-when = \"date:2027-01-01\"\n",
            rule_hash("Use pnpm.")
        ))
        .unwrap();
        let r = rationale_for(&doc, second, Some(&sidecar)).unwrap();
        assert_eq!(r.why, "lockfile churn");
        assert_eq!(r.retire_when, Some(RetireWhen::Date("2027-01-01".into())));
        assert!(sidecar.to_toml().contains("lockfile churn"));
    }
}
