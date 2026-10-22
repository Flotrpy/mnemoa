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
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default = "yes")]
    restates: bool,
}

fn yes() -> bool {
    true
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
    /// Rules matching any of these are about something else (a commit message's length, not
    /// code's), so the topic does not apply.
    pub exclude: Vec<Regex>,
    /// Whether a matching rule restates what the enforcer checks. `false` for rules that tell
    /// the agent which tool to run: MNA002 ignores them and `promote` keeps their prose.
    pub restates: bool,
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
            let exclude = t
                .exclude
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
                exclude,
                restates: t.restates,
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
        self.matches_in(rule_text, &[])
    }

    /// Topics a rule's text matches, given the headings it sits under. Patterns look at the
    /// rule alone; excludes also look at the headings, since "Keep the first line under 72
    /// characters" is about commits only because it sits under "Commits".
    pub fn matches_in<'a>(&'a self, rule_text: &str, headings: &[&str]) -> Vec<&'a Topic> {
        let text = rule_text.replace('`', "");
        let context = format!("{text}\n{}", headings.join("\n"));
        self.topics
            .iter()
            .filter(|t| t.patterns.iter().any(|p| p.is_match(&text)))
            .filter(|t| !t.exclude.iter().any(|x| x.is_match(&context)))
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
                    Some(req) if !requirement_met(&r.id, req, &e.config_text) => None,
                    _ => Some(e),
                }
            })
            .collect()
    }
}

/// The severity written right after a lint's name: `"no-console": "warn"`,
/// `'naming-convention': [\n 'warn', ...`, `"noExplicitAny": { "level": "error" }`,
/// `unwrap_used = "deny"`.
static LEVEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^["']?\s*[:=]?\s*\[?\s*(?:\{\s*["']?level["']?\s*:\s*)?["']?(off|warn|warning|error|info|allow|deny|forbid|none|0|1|2)\b"#)
        .unwrap()
});

/// Whether `req` matches the enforcer's configuration with a severity that fails a check. A lint
/// that is off never counts. For ESLint and Biome, `warn` does not fail CI by default, so it
/// does not count either; Clippy's `warn` does, since CI usually runs it with `-D warnings`.
/// When no severity follows the match (a preset name, a boolean option), the match counts.
fn requirement_met(id: &str, req: &Regex, config: &str) -> bool {
    let warn_fails = !matches!(id, "eslint" | "biome");
    req.find_iter(config)
        .any(|m| match LEVEL_RE.captures(&config[m.end()..]) {
            None => true,
            Some(c) => match c[1].to_lowercase().as_str() {
                "off" | "0" | "none" | "allow" => false,
                "warn" | "warning" | "info" | "1" => warn_fails,
                _ => true,
            },
        })
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
    fn warn_and_off_lints_do_not_enforce_in_eslint_and_biome() {
        let any = Catalog::builtin().topic("explicit-any").unwrap();
        let mut configured = BTreeMap::new();
        let biome = |text: &str| Enforcer {
            id: "biome".into(),
            name: "Biome".into(),
            evidence: vec!["biome.json".into()],
            config_text: text.into(),
        };
        // cal.com biome.json.
        configured.insert(
            "biome".into(),
            biome(r#""suspicious": { "noExplicitAny": "warn" }"#),
        );
        assert!(any.covered_by("avoid any type", &configured).is_empty());
        configured.insert(
            "biome".into(),
            biome(r#""noExplicitAny": { "level": "error" }"#),
        );
        assert_eq!(any.covered_by("avoid any type", &configured).len(), 1);
        // vscode eslint.config.js.
        let naming = Catalog::builtin().topic("naming-case").unwrap();
        configured.insert(
            "eslint".into(),
            Enforcer {
                id: "eslint".into(),
                name: "ESLint".into(),
                evidence: vec!["eslint.config.js".into()],
                config_text: "'@typescript-eslint/naming-convention': [\n\t\t'warn',\n".into(),
            },
        );
        assert!(
            naming
                .covered_by("Use camelCase for function names", &configured)
                .is_empty()
        );
    }

    #[test]
    fn excludes_drop_topics_about_something_else() {
        let c = Catalog::builtin();
        let ids = |t: &str| -> Vec<String> { c.matches(t).iter().map(|x| x.id.clone()).collect() };
        // PostHog and openai-agents-js commit-message rules (the headings say so);
        // astral-sh/ruff nesting; astro.
        let under = |t: &str, h: &[&str]| -> Vec<String> {
            c.matches_in(t, h).iter().map(|x| x.id.clone()).collect()
        };
        let posthog = ["Commits and Pull Requests", "Rules"];
        let line = "Keep the first line under 72 characters";
        assert!(ids(line).contains(&"line-length".into()));
        assert!(!under(line, &posthog).contains(&"line-length".into()));
        assert!(
            !under(
                "Keep summary under 80 characters.",
                &["Pull Request & Commit Guidelines"]
            )
            .contains(&"line-length".into())
        );
        assert!(!ids("Keep commit summaries under 50 characters").contains(&"line-length".into()));
        assert!(ids("Keep lines under 100 characters").contains(&"line-length".into()));
        assert!(
            !ids("Prefer let chains over nested if let statements to reduce indentation")
                .contains(&"indentation".into())
        );
        assert!(
            !ids("If you notice unrelated dead code, mention it - don't delete it.")
                .contains(&"unused".into())
        );
        assert!(ids("Remove unused imports").contains(&"unused".into()));
        // PostHog: mypy's Any is not TypeScript's any.
        assert!(
            !ids("Write as if mypy --strict were on. Annotate every signature, avoid Any.")
                .contains(&"explicit-any".into())
        );
        // cline: `===` inside an example, not a rule about equality.
        assert!(
            !ids(r#"Avoid hardcoded provider branches such as providerId === "x"."#)
                .contains(&"strict-equality".into())
        );
        assert!(ids("Use === instead of ==").contains(&"strict-equality".into()));
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
