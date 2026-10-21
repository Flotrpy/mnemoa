//! The scan pipeline: discover, parse, run rules, apply the baseline.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Config, ConfigError};
use crate::diag::{Finding, Severity, Summary};
use crate::discover::{DiscoverError, Target};
use crate::history::{GixHistory, History, HistoryError};
use crate::rules::{self, Context};
use crate::workspace::Workspace;

/// Default location of the baseline file.
pub const BASELINE_FILE: &str = ".mnemoa/baseline.json";

/// Errors from the high-level API.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Discover(#[from] DiscoverError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0}")]
    Invalid(String),
}

/// Known findings to hide, so existing repos can adopt mnemoa gradually.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    pub version: u32,
    pub findings: Vec<BaselineEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BaselineEntry {
    pub rule: String,
    pub path: String,
    pub fingerprint: String,
}

impl Baseline {
    pub fn from_findings(findings: &[Finding]) -> Baseline {
        let mut entries: Vec<BaselineEntry> = findings
            .iter()
            .map(|f| BaselineEntry {
                rule: f.rule.clone(),
                path: f.path.clone(),
                fingerprint: f.fingerprint.clone(),
            })
            .collect();
        entries.sort();
        entries.dedup();
        Baseline {
            version: 1,
            findings: entries,
        }
    }

    pub fn load(path: &Path) -> Result<Baseline, Error> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))
    }

    pub fn save(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let json = serde_json::to_string_pretty(self).expect("baseline serializes");
        std::fs::write(path, json + "\n").map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    fn contains(&self, f: &Finding) -> bool {
        self.findings
            .iter()
            .any(|e| e.fingerprint == f.fingerprint && e.rule == f.rule && e.path == f.path)
    }
}

/// Options for one scan.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Override the configured targets.
    pub targets: Option<Vec<Target>>,
    /// Run only these rule IDs (enables opt-in rules too).
    pub rules: Option<Vec<String>>,
    /// Override `scan.strict`.
    pub strict: Option<bool>,
    pub baseline: Option<Baseline>,
    /// Read git history (needed for MNA005). On by default in the CLI.
    pub history: bool,
}

/// Result of a scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub version: String,
    pub root: String,
    pub targets: Vec<Target>,
    pub config_source: String,
    pub strict: bool,
    pub findings: Vec<Finding>,
    pub summary: Summary,
    /// Findings hidden by the baseline.
    pub baselined: usize,
    /// Files checked.
    pub files: Vec<String>,
}

impl ScanReport {
    /// 1 when any error, or any warning in strict mode; else 0.
    pub fn exit_code(&self) -> i32 {
        if self.summary.errors > 0 || (self.strict && self.summary.warnings > 0) {
            1
        } else {
            0
        }
    }
}

/// Load the workspace for `root` under `config`.
pub fn load_workspace(root: &Path, config: &Config) -> Result<Workspace, Error> {
    Ok(Workspace::load(root, &config.discover_options())?)
}

/// Open git history for `root`, if any.
pub fn open_history(root: &Path, config: &Config) -> Result<Option<GixHistory>, Error> {
    Ok(GixHistory::open(root, config.drift.max_history)?)
}

/// Scan a repository.
pub fn scan(root: &Path, config: &Config, opts: &ScanOptions) -> Result<ScanReport, Error> {
    let ws = load_workspace(root, config)?;
    let history = if opts.history {
        open_history(root, config)?
    } else {
        None
    };
    Ok(scan_workspace(
        &ws,
        config,
        opts,
        history.as_ref().map(|h| h as &dyn History),
    ))
}

/// Scan an already loaded workspace.
pub fn scan_workspace(
    ws: &Workspace,
    config: &Config,
    opts: &ScanOptions,
    history: Option<&dyn History>,
) -> ScanReport {
    let targets = opts
        .targets
        .clone()
        .unwrap_or_else(|| config.targets.clone());
    let cx = Context::new(ws, config, &targets).with_history(history);
    let registry = rules::registry();
    let mut findings = rules::run_with(&cx, &registry, opts.rules.as_deref());
    let before = findings.len();
    if let Some(b) = &opts.baseline {
        findings.retain(|f| !b.contains(f));
    }
    let baselined = before - findings.len();
    let files: BTreeSet<String> = cx.instruction_docs().map(|d| d.path.clone()).collect();
    ScanReport {
        version: crate::VERSION.to_string(),
        root: ws.root.display().to_string(),
        targets,
        config_source: config.source.clone(),
        strict: opts.strict.unwrap_or(config.scan.strict),
        summary: Summary::of(&findings),
        findings,
        baselined,
        files: files.into_iter().collect(),
    }
}

/// Count findings by rule ID.
pub fn by_rule(findings: &[Finding]) -> std::collections::BTreeMap<String, usize> {
    let mut m = std::collections::BTreeMap::new();
    for f in findings {
        *m.entry(f.rule.clone()).or_insert(0) += 1;
    }
    m
}

/// Highest severity present.
pub fn max_severity(findings: &[Finding]) -> Option<Severity> {
    findings.iter().map(|f| f.severity).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (p, c) in files {
            let path = dir.path().join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        }
        dir
    }

    #[test]
    fn scan_finds_broken_link_and_exits_1() {
        let dir = repo(&[("AGENTS.md", "Read [x](missing.md) before release.\n")]);
        let r = scan(dir.path(), &Config::default(), &ScanOptions::default()).unwrap();
        assert_eq!(r.summary.errors, 1);
        assert_eq!(r.exit_code(), 1);
        assert_eq!(r.files, vec!["AGENTS.md"]);
    }

    #[test]
    fn baseline_hides_known_findings() {
        let dir = repo(&[("AGENTS.md", "Read [x](missing.md) before release.\n")]);
        let first = scan(dir.path(), &Config::default(), &ScanOptions::default()).unwrap();
        let b = Baseline::from_findings(&first.findings);
        let path = dir.path().join(BASELINE_FILE);
        b.save(&path).unwrap();
        let opts = ScanOptions {
            baseline: Some(Baseline::load(&path).unwrap()),
            ..Default::default()
        };
        let second = scan(dir.path(), &Config::default(), &opts).unwrap();
        assert_eq!(second.findings.len(), 0);
        assert_eq!(second.baselined, 1);
        assert_eq!(second.exit_code(), 0);
    }

    #[test]
    fn strict_fails_on_warnings() {
        let dir = repo(&[("AGENTS.md", "- Run `make lint`.\n")]);
        let r = scan(
            dir.path(),
            &Config::default(),
            &ScanOptions {
                strict: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.summary.warnings, 1);
        assert_eq!(r.exit_code(), 1);
        let r = scan(dir.path(), &Config::default(), &ScanOptions::default()).unwrap();
        assert_eq!(r.exit_code(), 0);
    }

    #[test]
    fn only_rules_filter() {
        let dir = repo(&[("AGENTS.md", "- Run `make lint`.\n- [x](missing.md)\n")]);
        let r = scan(
            dir.path(),
            &Config::default(),
            &ScanOptions {
                rules: Some(vec!["MNA008".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(r.findings.iter().all(|f| f.rule == "MNA008"));
        assert_eq!(by_rule(&r.findings).get("MNA008"), Some(&1));
    }
}
