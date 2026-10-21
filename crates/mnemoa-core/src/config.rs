//! `.mnemoa.toml` configuration with `MNEMOA_*` environment overrides.
//!
//! Precedence, highest first: environment variables, the file given with `--config`,
//! `.mnemoa.toml` at the scanned root, built-in defaults.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diag::Severity;
use crate::discover::{DiscoverOptions, Target};

/// File name looked up at the repository root.
pub const CONFIG_FILE: &str = ".mnemoa.toml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Agents to model.
    pub targets: Vec<Target>,
    pub scan: ScanConfig,
    pub rules: RulesConfig,
    pub drift: DriftConfig,
    pub llm: LlmConfig,
    pub api: ApiConfig,
    /// Where the active configuration came from (reported by `/health`).
    #[serde(skip)]
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ScanConfig {
    /// Glob patterns to skip.
    pub exclude: Vec<String>,
    /// Extra Markdown files to check as on-demand documents (for example `CONTEXT.md`).
    pub include: Vec<String>,
    /// Treat warnings as failures.
    pub strict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RulesConfig {
    /// MNA001: line threshold for one always-loaded file.
    pub max_lines: usize,
    /// MNA001: always-loaded token budget per target.
    pub token_budget: usize,
    /// MNA005: later code commits before a single-commit file counts as fossilized.
    pub fossil_commits: usize,
    /// Rule IDs to turn off.
    pub disable: Vec<String>,
    /// Opt-in rule IDs to turn on (for example `MNA011`).
    pub enable: Vec<String>,
    /// Severity overrides by rule ID.
    pub severity: BTreeMap<String, Severity>,
}

impl Default for RulesConfig {
    fn default() -> Self {
        RulesConfig {
            max_lines: 200,
            token_budget: 4000,
            fossil_commits: 25,
            disable: Vec::new(),
            enable: Vec::new(),
            severity: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriftConfig {
    /// Code commits since `activeContext.md` / `progress.md` last changed before they count as stale.
    pub memory_bank_max_commits: usize,
    /// Upper bound on commits read from history.
    pub max_history: usize,
}

impl Default for DriftConfig {
    fn default() -> Self {
        DriftConfig {
            memory_bank_max_commits: 10,
            max_history: 5000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub enabled: bool,
    /// `ollama`, `openai`, `anthropic` or `custom`.
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        LlmConfig {
            enabled: false,
            provider: "ollama".into(),
            base_url: "http://localhost:11434/v1".into(),
            model: "qwen2.5-coder:7b".into(),
            api_key: "ollama".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Root directory the API may read.
    pub root: PathBuf,
    pub port: u16,
    /// Allow `apply: true` over HTTP.
    pub allow_write: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            root: PathBuf::from("."),
            port: 8080,
            allow_write: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            targets: Target::DEFAULT.to_vec(),
            scan: ScanConfig::default(),
            rules: RulesConfig::default(),
            drift: DriftConfig::default(),
            llm: LlmConfig::default(),
            api: ApiConfig::default(),
            source: "defaults".into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config `{path}`: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config `{path}`: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("invalid value for {var}: {message}")]
    Env { var: String, message: String },
}

impl Config {
    /// Parse TOML text.
    pub fn from_toml(text: &str, path: &Path) -> Result<Config, ConfigError> {
        let mut c: Config = toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        c.source = format!("file:{}", path.display());
        Ok(c)
    }

    /// Load config for `root`: the explicit file if given, else `<root>/.mnemoa.toml` if present,
    /// else defaults; then apply `MNEMOA_*` variables from the process environment.
    pub fn load(root: &Path, explicit: Option<&Path>) -> Result<Config, ConfigError> {
        let mut c = Self::load_file(root, explicit)?;
        c.apply_env(std::env::vars())?;
        Ok(c)
    }

    /// Like [`Config::load`] without reading the environment.
    pub fn load_file(root: &Path, explicit: Option<&Path>) -> Result<Config, ConfigError> {
        let path = match explicit {
            Some(p) => Some(p.to_path_buf()),
            None => {
                let p = root.join(CONFIG_FILE);
                p.is_file().then_some(p)
            }
        };
        match path {
            Some(p) => {
                let text = std::fs::read_to_string(&p).map_err(|source| ConfigError::Read {
                    path: p.clone(),
                    source,
                })?;
                Config::from_toml(&text, &p)
            }
            None => Ok(Config::default()),
        }
    }

    /// Apply `MNEMOA_*` overrides from `vars`.
    pub fn apply_env(
        &mut self,
        vars: impl IntoIterator<Item = (String, String)>,
    ) -> Result<(), ConfigError> {
        let mut used = Vec::new();
        for (k, v) in vars {
            if !k.starts_with("MNEMOA_") {
                continue;
            }
            let err = |message: String| ConfigError::Env {
                var: k.clone(),
                message,
            };
            let num = |v: &str| v.trim().parse::<usize>().map_err(|e| err(e.to_string()));
            let flag = |v: &str| {
                parse_bool(v).ok_or_else(|| err(format!("expected true or false, got `{v}`")))
            };
            match k.as_str() {
                "MNEMOA_ROOT" => self.api.root = PathBuf::from(&v),
                "MNEMOA_PORT" => {
                    self.api.port = v
                        .trim()
                        .parse()
                        .map_err(|e: std::num::ParseIntError| err(e.to_string()))?
                }
                "MNEMOA_TARGETS" => self.targets = Target::parse_list(&v).map_err(err)?,
                "MNEMOA_MAX_LINES" => self.rules.max_lines = num(&v)?,
                "MNEMOA_TOKEN_BUDGET" => self.rules.token_budget = num(&v)?,
                "MNEMOA_FOSSIL_COMMITS" => self.rules.fossil_commits = num(&v)?,
                "MNEMOA_LLM_ENABLED" => self.llm.enabled = flag(&v)?,
                "MNEMOA_LLM_PROVIDER" => self.llm.provider = v.trim().to_string(),
                "MNEMOA_LLM_BASE_URL" => self.llm.base_url = v.trim().to_string(),
                "MNEMOA_LLM_MODEL" => self.llm.model = v.trim().to_string(),
                "MNEMOA_LLM_API_KEY" => self.llm.api_key = v,
                "MNEMOA_API_ALLOW_WRITE" => self.api.allow_write = flag(&v)?,
                _ => continue,
            }
            used.push(k);
        }
        if !used.is_empty() {
            used.sort();
            self.source = format!("{}+env:{}", self.source, used.join(","));
        }
        Ok(())
    }

    /// Discovery options derived from the scan section.
    pub fn discover_options(&self) -> DiscoverOptions {
        DiscoverOptions {
            exclude: self.scan.exclude.clone(),
            include: self.scan.include.clone(),
        }
    }

    /// Whether a rule runs: not disabled, and either on by default or explicitly enabled.
    pub fn rule_enabled(&self, id: &str, opt_in: bool) -> bool {
        let has = |list: &[String]| list.iter().any(|r| r.eq_ignore_ascii_case(id));
        if has(&self.rules.disable) {
            return false;
        }
        !opt_in || has(&self.rules.enable)
    }

    /// Severity for a rule after overrides.
    pub fn severity_for(&self, id: &str, default: Severity) -> Severity {
        self.rules
            .severity
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(id))
            .map(|(_, s)| *s)
            .unwrap_or(default)
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_docs() {
        let c = Config::default();
        assert_eq!(c.targets, Target::DEFAULT.to_vec());
        assert_eq!(c.rules.max_lines, 200);
        assert_eq!(c.rules.token_budget, 4000);
        assert_eq!(c.rules.fossil_commits, 25);
        assert_eq!(c.api.port, 8080);
        assert!(!c.llm.enabled && !c.api.allow_write);
        assert_eq!(c.source, "defaults");
    }

    #[test]
    fn parses_toml_partially() {
        let text = r#"
targets = ["claude-code", "kiro"]
[rules]
max_lines = 120
disable = ["MNA009"]
[rules.severity]
MNA006 = "warn"
[scan]
exclude = ["fixtures/**"]
"#;
        let c = Config::from_toml(text, Path::new(".mnemoa.toml")).unwrap();
        assert_eq!(c.targets, vec![Target::ClaudeCode, Target::Kiro]);
        assert_eq!(c.rules.max_lines, 120);
        assert_eq!(c.rules.token_budget, 4000);
        assert!(!c.rule_enabled("MNA009", false));
        assert!(c.rule_enabled("MNA001", false));
        assert!(!c.rule_enabled("MNA011", true));
        assert_eq!(c.severity_for("mna006", Severity::Info), Severity::Warn);
        assert_eq!(c.scan.exclude, vec!["fixtures/**"]);
        assert!(c.source.starts_with("file:"));
    }

    #[test]
    fn env_overrides_file() {
        let mut c = Config::from_toml("[rules]\nmax_lines = 120\n", Path::new("x.toml")).unwrap();
        c.apply_env(vec![
            ("MNEMOA_MAX_LINES".to_string(), "90".to_string()),
            ("MNEMOA_TARGETS".to_string(), "codex".to_string()),
            ("MNEMOA_LLM_ENABLED".to_string(), "true".to_string()),
            ("OTHER".to_string(), "1".to_string()),
        ])
        .unwrap();
        assert_eq!(c.rules.max_lines, 90);
        assert_eq!(c.targets, vec![Target::Codex]);
        assert!(c.llm.enabled);
        assert_eq!(
            c.source,
            "file:x.toml+env:MNEMOA_LLM_ENABLED,MNEMOA_MAX_LINES,MNEMOA_TARGETS"
        );
    }

    #[test]
    fn bad_env_value_is_an_error() {
        let mut c = Config::default();
        let e = c
            .apply_env(vec![("MNEMOA_PORT".to_string(), "lots".to_string())])
            .unwrap_err();
        assert!(e.to_string().contains("MNEMOA_PORT"));
        let e = c
            .apply_env(vec![("MNEMOA_TARGETS".to_string(), "vim".to_string())])
            .unwrap_err();
        assert!(e.to_string().contains("unknown target"));
    }

    #[test]
    fn invalid_toml_reports_path() {
        let e = Config::from_toml("targets = 5", Path::new("bad.toml")).unwrap_err();
        assert!(e.to_string().contains("bad.toml"));
    }

    #[test]
    fn load_reads_root_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CONFIG_FILE), "[rules]\ntoken_budget = 10\n").unwrap();
        let c = Config::load_file(dir.path(), None).unwrap();
        assert_eq!(c.rules.token_budget, 10);
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load_file(empty.path(), None).unwrap(),
            Config::default()
        );
    }
}
