//! `.mnemoa/ledger.toml`: which prose rules were promoted to which executable checks.

use serde::{Deserialize, Serialize};

use crate::rationale::RetireWhen;

/// Location of the ledger.
pub const LEDGER_FILE: &str = ".mnemoa/ledger.toml";

/// Lifecycle of a promotion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Planned (dry run or manual step outstanding).
    Proposed,
    /// Artifacts written; the check exists.
    Applied,
}

/// One promoted rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Check id, unique within the ledger.
    pub id: String,
    /// `rationale::rule_hash` of the rule text.
    pub hash: String,
    /// The rule text as it was in the doc.
    pub text: String,
    /// Doc the rule came from.
    pub source: String,
    /// Tool that enforces it (editorconfig, clippy, ast-grep, pre-commit ...).
    pub enforcer: String,
    /// Files that implement the check.
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// Date promoted (YYYY-MM-DD, UTC).
    pub date: String,
    pub state: State,
    #[serde(
        rename = "retire-when",
        alias = "retire_when",
        skip_serializing_if = "Option::is_none"
    )]
    pub retire_when: Option<RetireWhen>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default, rename = "check")]
    pub checks: Vec<Entry>,
}

impl Ledger {
    pub fn parse(text: &str) -> Result<Ledger, String> {
        toml::from_str(text).map_err(|e| format!("{LEDGER_FILE}: {e}"))
    }

    pub fn to_toml(&self) -> String {
        let body = toml::to_string_pretty(self).unwrap_or_default();
        format!(
            "# Managed by `mnemoa promote`. Each entry maps a prose rule to the check that enforces it.\n\n{body}"
        )
    }

    pub fn by_hash(&self, hash: &str) -> Option<&Entry> {
        self.checks.iter().find(|e| e.hash == hash)
    }

    pub fn by_id(&self, id: &str) -> Option<&Entry> {
        self.checks.iter().find(|e| e.id == id)
    }

    /// Insert or replace by id.
    pub fn upsert(&mut self, entry: Entry) {
        match self.checks.iter_mut().find(|e| e.id == entry.id) {
            Some(e) => *e = entry,
            None => self.checks.push(entry),
        }
    }

    /// A check id not yet used, based on `base`.
    pub fn unique_id(&self, base: &str, taken: &[String]) -> String {
        let used = |id: &str| self.by_id(id).is_some() || taken.iter().any(|t| t == id);
        if !used(base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|c| !used(c))
            .expect("some id is free")
    }
}

/// Today's date (UTC) as YYYY-MM-DD, without a date library.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil_date(secs.div_euclid(86_400))
}

/// Convert days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
pub fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> Entry {
        Entry {
            id: id.into(),
            hash: "abc".into(),
            text: "Use 2 spaces.".into(),
            source: "AGENTS.md".into(),
            enforcer: "editorconfig".into(),
            artifacts: vec![".editorconfig".into()],
            date: "2026-10-21".into(),
            state: State::Applied,
            retire_when: Some(RetireWhen::Check(id.into())),
        }
    }

    #[test]
    fn round_trip() {
        let mut l = Ledger::default();
        l.upsert(entry("indentation"));
        let text = l.to_toml();
        assert!(text.contains("[[check]]") && text.contains("retire-when = \"check:indentation\""));
        assert_eq!(Ledger::parse(&text).unwrap(), l);
    }

    #[test]
    fn unique_ids_and_upsert() {
        let mut l = Ledger::default();
        l.upsert(entry("a"));
        assert_eq!(l.unique_id("a", &[]), "a-2");
        assert_eq!(l.unique_id("a", &["a-2".into()]), "a-3");
        assert_eq!(l.unique_id("b", &[]), "b");
        let mut e = entry("a");
        e.state = State::Proposed;
        l.upsert(e);
        assert_eq!(l.checks.len(), 1);
        assert_eq!(l.checks[0].state, State::Proposed);
        assert!(l.by_hash("abc").is_some());
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(19_723), "2024-01-01");
        assert_eq!(civil_date(20_747), "2026-10-21");
        assert_eq!(today().len(), 10);
    }

    #[test]
    fn bad_ledger_is_error() {
        assert!(Ledger::parse("check = 3").is_err());
    }
}
