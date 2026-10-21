//! LLM-drafted ast-grep rules for prose the catalog cannot match.
//!
//! This module is offline: it builds prompts, parses replies and validates drafts by running the
//! `ast-grep` CLI on the draft's own positive and negative samples. The HTTP client lives in
//! [`crate::llm`] behind the `llm` feature. A draft is used only when validation passes; if
//! `ast-grep` is not installed the draft is reported as unvalidated and never applied.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// What to draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DraftRequest {
    pub rule: String,
    /// ast-grep language name (rust, typescript, python, go, javascript, tsx).
    pub language: String,
    /// Directories the rule applies to.
    pub scope: Vec<String>,
}

/// An ast-grep rule plus samples, as returned by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    /// YAML for the `rule:` key (for example `pattern: eval($$$)`).
    pub rule: String,
    /// Code that must match.
    #[serde(default)]
    pub positive: Vec<String>,
    /// Code that must not match.
    #[serde(default)]
    pub negative: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error("LLM drafting is disabled (set llm.enabled = true and build with the `llm` feature)")]
    Disabled,
    #[error("LLM request failed: {0}")]
    Request(String),
    #[error("could not parse the model's reply: {0}")]
    Parse(String),
    #[error("the model declined: {0}")]
    Refused(String),
}

/// Something that can draft rules (an HTTP client, or a fake in tests).
pub trait Drafter: Sync {
    fn draft(&self, req: &DraftRequest) -> Result<Draft, DraftError>;
}

/// Outcome of validating a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", content = "detail", rename_all = "kebab-case")]
pub enum Validation {
    Passed,
    Failed(String),
    /// ast-grep is not available; the draft must not be applied.
    Unvalidated(String),
}

/// System prompt for the drafter.
pub const SYSTEM_PROMPT: &str = "You convert one coding rule from a repository's agent instructions into an ast-grep rule. \
Reply with a single JSON object and nothing else: {\"rule\": \"<YAML for the ast-grep `rule:` key>\", \"positive\": [\"<code that violates the rule>\", ...], \"negative\": [\"<similar code that follows the rule>\", ...]}. \
Give at least two positive and two negative samples, each a complete, parseable snippet in the requested language. \
Use `pattern:`, `kind:`, `regex:`, `has:`, `inside:`, `any:`, `all:` or `not:` as ast-grep defines them. \
If the rule cannot be checked by matching syntax, reply {\"rule\": \"\", \"positive\": [], \"negative\": []}.";

/// User prompt for one request.
pub fn user_prompt(req: &DraftRequest) -> String {
    let scope = if req.scope.is_empty() {
        "the whole repository".to_string()
    } else {
        req.scope.join(", ")
    };
    format!(
        "Language: {}\nApplies to: {scope}\nRule: {}",
        req.language, req.rule
    )
}

/// Extract the first JSON object from a reply (tolerates code fences and surrounding prose).
pub fn parse_draft(text: &str) -> Result<Draft, DraftError> {
    let start = text
        .find('{')
        .ok_or_else(|| DraftError::Parse("no JSON object in reply".into()))?;
    let end = text
        .rfind('}')
        .ok_or_else(|| DraftError::Parse("unterminated JSON object".into()))?;
    if end < start {
        return Err(DraftError::Parse("unterminated JSON object".into()));
    }
    let d: Draft =
        serde_json::from_str(&text[start..=end]).map_err(|e| DraftError::Parse(e.to_string()))?;
    if d.rule.trim().is_empty() {
        return Err(DraftError::Refused(
            "the model said the rule cannot be checked syntactically".into(),
        ));
    }
    Ok(d)
}

/// File extension for samples in an ast-grep language.
pub fn extension(language: &str) -> &'static str {
    match language {
        "rust" => "rs",
        "typescript" => "ts",
        "tsx" => "tsx",
        "javascript" => "js",
        "python" => "py",
        "go" => "go",
        "java" => "java",
        "ruby" => "rb",
        _ => "txt",
    }
}

/// Find the ast-grep CLI. `sg` on Linux is often an unrelated tool, so the version output is
/// checked. `MNEMOA_AST_GREP` overrides the search.
pub fn find_ast_grep() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("MNEMOA_AST_GREP") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("ast-grep"));
    candidates.push(PathBuf::from("sg"));
    candidates.into_iter().find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("ast-grep"))
    })
}

fn count_matches(ast_grep: &Path, rule_file: &Path, sample: &Path) -> Result<usize, String> {
    let out = Command::new(ast_grep)
        .arg("scan")
        .arg("--rule")
        .arg(rule_file)
        .arg("--json=compact")
        .arg(sample)
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first = stdout.lines().next().unwrap_or("[]");
    let v: serde_json::Value = serde_json::from_str(first).map_err(|e| {
        format!(
            "ast-grep output was not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
    })?;
    Ok(v.as_array().map_or(0, Vec::len))
}

/// Run `draft` against its samples with ast-grep.
pub fn validate(draft: &Draft, language: &str, ast_grep: Option<&Path>) -> Validation {
    let Some(ag) = ast_grep else {
        return Validation::Unvalidated("ast-grep is not installed".into());
    };
    if draft.positive.is_empty() || draft.negative.is_empty() {
        return Validation::Failed("needs at least one positive and one negative sample".into());
    }
    let Ok(dir) = tempfile_dir() else {
        return Validation::Unvalidated("could not create a temporary directory".into());
    };
    let rule_file = dir.path().join("rule.yml");
    let yaml = crate::promote::ast_grep_rule("draft", language, "draft", &draft.rule, &[]);
    if std::fs::write(&rule_file, yaml).is_err() {
        return Validation::Unvalidated("could not write the rule file".into());
    }
    let ext = extension(language);
    for (kind, samples, want_match) in [
        ("positive", &draft.positive, true),
        ("negative", &draft.negative, false),
    ] {
        for (i, code) in samples.iter().enumerate() {
            let sample = dir.path().join(format!("{kind}_{i}.{ext}"));
            if std::fs::write(&sample, code).is_err() {
                return Validation::Unvalidated("could not write a sample".into());
            }
            match count_matches(ag, &rule_file, &sample) {
                Err(e) => return Validation::Failed(e),
                Ok(n) if want_match && n == 0 => {
                    return Validation::Failed(format!("{kind} sample {} did not match", i + 1));
                }
                Ok(n) if !want_match && n > 0 => {
                    return Validation::Failed(format!("{kind} sample {} matched", i + 1));
                }
                Ok(_) => {}
            }
        }
    }
    Validation::Passed
}

/// A temporary directory removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tempfile_dir() -> std::io::Result<TempDir> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let p = std::env::temp_dir().join(format!(
        "mnemoa-draft-{}-{}-{nanos}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&p)?;
    Ok(TempDir(p))
}

/// A drafter that returns canned drafts (tests and offline demos).
#[derive(Debug, Clone, Default)]
pub struct StaticDrafter {
    pub reply: String,
}

impl Drafter for StaticDrafter {
    fn draft(&self, _req: &DraftRequest) -> Result<Draft, DraftError> {
        parse_draft(&self.reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = "Here you go:\n```json\n{\"rule\": \"pattern: eval($$$)\", \"positive\": [\"x = eval('1')\", \"eval(s)\"], \"negative\": [\"x = evaluate(1)\", \"literal_eval(s)\"]}\n```";

    #[test]
    fn parses_fenced_reply() {
        let d = parse_draft(REPLY).unwrap();
        assert_eq!(d.rule, "pattern: eval($$$)");
        assert_eq!(d.positive.len(), 2);
    }

    #[test]
    fn empty_rule_is_a_refusal_and_garbage_is_parse_error() {
        assert!(matches!(
            parse_draft("{\"rule\": \"\"}"),
            Err(DraftError::Refused(_))
        ));
        assert!(matches!(
            parse_draft("no json here"),
            Err(DraftError::Parse(_))
        ));
        assert!(matches!(
            parse_draft("{\"positive\": 3}"),
            Err(DraftError::Parse(_))
        ));
    }

    #[test]
    fn prompt_mentions_language_and_scope() {
        let p = user_prompt(&DraftRequest {
            rule: "Never use eval".into(),
            language: "python".into(),
            scope: vec!["app".into()],
        });
        assert!(p.contains("Language: python") && p.contains("Applies to: app"));
    }

    #[test]
    fn without_ast_grep_drafts_are_unvalidated() {
        let d = parse_draft(REPLY).unwrap();
        assert!(matches!(
            validate(&d, "python", None),
            Validation::Unvalidated(_)
        ));
    }

    #[test]
    fn validation_with_ast_grep_when_installed() {
        let Some(ag) = find_ast_grep() else {
            eprintln!("ast-grep not installed; skipping");
            return;
        };
        let good = parse_draft(REPLY).unwrap();
        assert_eq!(validate(&good, "python", Some(&ag)), Validation::Passed);
        let bad = Draft {
            rule: "pattern: eval($$$)".into(),
            positive: vec!["x = 1".into()],
            negative: vec!["y = 2".into()],
        };
        assert!(matches!(
            validate(&bad, "python", Some(&ag)),
            Validation::Failed(_)
        ));
        let too_broad = Draft {
            rule: "kind: call".into(),
            positive: vec!["eval(s)".into()],
            negative: vec!["print(s)".into()],
        };
        assert!(matches!(
            validate(&too_broad, "python", Some(&ag)),
            Validation::Failed(_)
        ));
    }
}
