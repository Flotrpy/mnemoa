//! The promotion catalog (`rules/promotions.toml`): prose topics and the enforcers that check them.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use serde::Deserialize;

use crate::enforcers::Enforcer;

/// The catalog shipped with mnemoa.
pub const BUILTIN: &str = include_str!("../../../rules/promotions.toml");

#[derive(Debug, Deserialize)]
struct RawCatalog {
    topic: Vec<RawTopic>,
}

#[derive(Debug, Deserialize)]
struct RawTopic {
    id: String,
    summary: String,
    patterns: Vec<String>,
    enforcers: Vec<String>,
    #[serde(default)]
    named: bool,
}

/// An enforcer a topic can rely on, with an optional requirement on its configuration.
#[derive(Debug, Clone)]
pub struct EnforcerRef {
    pub id: String,
    pub requires: Option<Regex>,
}

/// One prose topic.
#[derive(Debug, Clone)]
pub struct Topic {
    pub id: String,
    pub summary: String,
    pub patterns: Vec<Regex>,
    pub enforcers: Vec<EnforcerRef>,
    pub named: bool,
}

/// Parsed catalog.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub topics: Vec<Topic>,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("invalid catalog: {0}")]
    Toml(String),
    #[error("invalid pattern in topic `{topic}`: {source}")]
    Regex { topic: String, source: regex::Error },
}

fn ci(p: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(p).case_insensitive(true).build()
}

impl Catalog {
    pub fn parse(text: &str) -> Result<Catalog, CatalogError> {
        let raw: RawCatalog =
            toml::from_str(text).map_err(|e| CatalogError::Toml(e.to_string()))?;
        let mut topics = Vec::new();
        for t in raw.topic {
            let err = |source| CatalogError::Regex {
                topic: t.id.clone(),
                source,
            };
            let patterns = t
                .patterns
                .iter()
                .map(|p| ci(p))
                .collect::<Result<Vec<_>, _>>()
                .map_err(err)?;
            let mut enforcers = Vec::new();
            for e in &t.enforcers {
                let (id, req) = match e.split_once(':') {
                    Some((id, r)) => (id, Some(ci(r).map_err(err)?)),
                    None => (e.as_str(), None),
                };
                enforcers.push(EnforcerRef {
                    id: id.to_string(),
                    requires: req,
                });
            }
            topics.push(Topic {
                id: t.id,
                summary: t.summary,
                patterns,
                enforcers,
                named: t.named,
            });
        }
        Ok(Catalog { topics })
    }

    /// The built-in catalog.
    pub fn builtin() -> &'static Catalog {
        static CAT: LazyLock<Catalog> =
            LazyLock::new(|| Catalog::parse(BUILTIN).expect("built-in promotions.toml is valid"));
        &CAT
    }

    /// Topics a rule's text matches.
    pub fn matches<'a>(&'a self, rule_text: &str) -> Vec<&'a Topic> {
        let text = rule_text.replace('`', "");
        self.topics
            .iter()
            .filter(|t| t.patterns.iter().any(|p| p.is_match(&text)))
            .collect()
    }

    pub fn topic(&self, id: &str) -> Option<&Topic> {
        self.topics.iter().find(|t| t.id == id)
    }
}

impl Topic {
    /// Configured enforcers that cover this topic for `rule_text`.
    pub fn covered_by<'a>(
        &self,
        rule_text: &str,
        configured: &'a BTreeMap<String, Enforcer>,
    ) -> Vec<&'a Enforcer> {
        let lower = rule_text.to_lowercase();
        self.enforcers
            .iter()
            .filter(|r| !self.named || lower.contains(&r.id.replace("-format", "")))
            .filter_map(|r| {
                let e = configured.get(&r.id)?;
                match &r.requires {
                    Some(req) if !req.is_match(&e.config_text) => None,
                    _ => Some(e),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_parses() {
        let c = Catalog::builtin();
        assert!(c.topics.len() >= 15);
        assert!(c.topic("indentation").is_some());
    }

    #[test]
    fn matching_strips_backticks() {
        let c = Catalog::builtin();
        let ids: Vec<&str> = c
            .matches("Never call `.unwrap()` in library code")
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids, vec!["unwrap"]);
        assert!(
            c.matches("Use 2 spaces for indentation")
                .iter()
                .any(|t| t.id == "indentation")
        );
        assert!(c.matches("Write tests for new features").is_empty());
    }

    #[test]
    fn requirements_and_named_topics() {
        let c = Catalog::builtin();
        let mut configured = BTreeMap::new();
        configured.insert(
            "clippy".to_string(),
            Enforcer {
                id: "clippy".into(),
                name: "Clippy".into(),
                evidence: vec!["Cargo.toml".into()],
                config_text: "pedantic = \"warn\"".into(),
            },
        );
        let unwrap = c.topic("unwrap").unwrap();
        assert!(unwrap.covered_by("no unwrap", &configured).is_empty());
        configured
            .get_mut("clippy")
            .unwrap()
            .config_text
            .push_str("unwrap_used = \"deny\"");
        assert_eq!(unwrap.covered_by("no unwrap", &configured).len(), 1);

        configured.insert(
            "prettier".into(),
            Enforcer {
                id: "prettier".into(),
                name: "Prettier".into(),
                evidence: vec![".prettierrc".into()],
                config_text: String::new(),
            },
        );
        let fmt = c.topic("format-with-tool").unwrap();
        assert!(fmt.covered_by("format with black", &configured).is_empty());
        assert_eq!(fmt.covered_by("format with prettier", &configured).len(), 1);
    }

    #[test]
    fn bad_catalog_is_an_error() {
        assert!(Catalog::parse("topic = 1").is_err());
        assert!(
            Catalog::parse("[[topic]]\nid='x'\nsummary='s'\npatterns=['(']\nenforcers=[]\n")
                .is_err()
        );
    }
}
