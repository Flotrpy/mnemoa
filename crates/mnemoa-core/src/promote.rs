//! Promotion: turn enforceable prose rules into executable checks, then remove the prose.
//!
//! 1. Classify each rule with the catalog in `rules/promotions.toml` plus a few structural
//!    patterns (banned calls, import boundaries, "run X before committing").
//! 2. Emit artifacts: EditorConfig keys, Cargo `[lints]`, Ruff, mypy or Prettier settings,
//!    ast-grep rules in `.mnemoa/checks/` with `sgconfig.yml`, or pre-commit hooks.
//! 3. Record each promotion in `.mnemoa/ledger.toml`.
//! 4. Dry run prints a unified diff; [`apply`] writes it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::catalog::{Catalog, Topic};
use crate::config::Config;
use crate::doc::{Doc, Polarity, Rule, RuleKind};
use crate::drafter::{DraftRequest, Drafter, Validation};
use crate::enforcers::{self, Enforcer};
use crate::ledger::{self, Entry, LEDGER_FILE, Ledger, State};
use crate::rationale::{self, RetireWhen};
use crate::rules::Context;
use crate::workspace::Workspace;

/// Directory for generated ast-grep rules.
pub const CHECKS_DIR: &str = ".mnemoa/checks";

/// How an artifact is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Create,
    Update,
    /// Cannot be merged safely; the snippet must be added by hand.
    Manual,
}

/// A file change that implements a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Artifact {
    pub path: String,
    pub action: Action,
    /// Full new content (Create/Update) or the snippet to add (Manual).
    pub content: String,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// A configured tool already enforces it; only the prose goes.
    AlreadyEnforced,
    /// mnemoa emits a check.
    Emit,
    /// Drafted by the LLM and validated against samples.
    Drafted,
}

/// One promoted rule.
#[derive(Debug, Clone, Serialize)]
pub struct Promotion {
    pub id: String,
    pub source: String,
    pub line: usize,
    pub rule: String,
    pub hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    pub enforcer: String,
    pub status: Status,
    pub artifacts: Vec<Artifact>,
    /// Whether `apply` deletes the prose (list items without nested lists).
    pub removes_prose: bool,
    pub retire_when: RetireWhen,
    #[serde(skip)]
    span: std::ops::Range<usize>,
}

/// A rule that looks enforceable but matched nothing in the catalog.
#[derive(Debug, Clone, Serialize)]
pub struct Unmatched {
    pub source: String,
    pub line: usize,
    pub rule: String,
    /// What the LLM drafter produced, when it ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftOutcome>,
}

/// A draft that was not applied, and why.
#[derive(Debug, Clone, Serialize)]
pub struct DraftOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    pub validation: Validation,
}

/// The full plan.
#[derive(Debug, Clone, Serialize)]
pub struct PromotionPlan {
    pub promotions: Vec<Promotion>,
    pub unmatched: Vec<Unmatched>,
    /// Unified diff of every file the plan changes.
    pub diff: String,
    /// New content per changed file (None = delete).
    #[serde(skip)]
    pub files: BTreeMap<String, String>,
    pub files_changed: Vec<String>,
}

/// File contents with pending edits layered on top of the workspace.
struct Vfs<'a> {
    ws: &'a Workspace,
    overlay: BTreeMap<String, String>,
}

impl<'a> Vfs<'a> {
    fn read(&self, path: &str) -> Option<String> {
        self.overlay
            .get(path)
            .cloned()
            .or_else(|| self.ws.read_file(path))
    }
    fn exists(&self, path: &str) -> bool {
        self.overlay.contains_key(path) || self.ws.discovery.has_file(path)
    }
    fn write(&mut self, path: &str, content: String) -> Action {
        let action = if self.exists(path) {
            Action::Update
        } else {
            Action::Create
        };
        self.overlay.insert(path.to_string(), content);
        action
    }
}

static NUM_SPACES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d+|two|four|eight)[- ]spaces?\b").unwrap());
static NUMBER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{2,3})\b").unwrap());
static PROCESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:run|execute)\s+`([^`]+)`\s+(?:before|prior\s+to)\s+(?:every\s+|each\s+)?(?:committing|commit|commits|pushing|push|opening\s+a\s+(?:pr|pull\s+request))").unwrap()
});
static BOUNDARY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)`([\w./-]+?)/?`\s+(?:must|should|may|can)\s*(?:not|n't)\s+import(?:\s+anything)?\s+(?:from\s+)?`([\w./-]+?)/?`").unwrap()
});

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-')
        .chars()
        .take(40)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

/// Directories a rule scopes itself to, from code spans such as `src/` or `src/api`.
fn scope_dirs(rule: &Rule, doc: &Doc, ws: &Workspace) -> Vec<String> {
    doc.code_spans
        .iter()
        .filter(|c| rule.span.contains(&c.span.start))
        .map(|c| {
            c.code
                .trim()
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_string()
        })
        .filter(|p| !p.is_empty() && ws.discovery.has_dir(p))
        .collect()
}

fn ext_lang(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1;
    Some(match ext {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        _ => return None,
    })
}

/// The most common ast-grep language among source files under `scope` (or the whole repo).
fn dominant_language(ws: &Workspace, scope: &[String]) -> Option<&'static str> {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for f in &ws.discovery.files {
        if !scope.is_empty() && !scope.iter().any(|s| f.starts_with(&format!("{s}/"))) {
            continue;
        }
        if let Some(l) = ext_lang(f) {
            *counts.entry(l).or_default() += 1;
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(l, _)| l)
}

fn has_lang(ws: &Workspace, marker: &[&str]) -> bool {
    marker.iter().any(|m| ws.discovery.has_file(m))
}

fn yaml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// An ast-grep rule file.
pub fn ast_grep_rule(
    id: &str,
    language: &str,
    message: &str,
    rule_body: &str,
    files: &[String],
) -> String {
    let mut out = format!(
        "# Promoted by mnemoa from a prose rule; see .mnemoa/ledger.toml.\nid: {id}\nlanguage: {language}\nseverity: error\nmessage: {}\nrule:\n",
        yaml_str(message)
    );
    for line in rule_body.lines() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    if !files.is_empty() {
        out.push_str("files:\n");
        for f in files {
            out.push_str(&format!("  - {}\n", yaml_str(&format!("{f}/**"))));
        }
    }
    out
}

/// Set keys in the `[*]` section of an EditorConfig file. Existing keys are left alone.
fn editorconfig_set(existing: Option<&str>, keys: &[(&str, String)]) -> (String, Vec<String>) {
    let mut conflicts = Vec::new();
    let Some(text) = existing else {
        let mut s = String::from("root = true\n\n[*]\n");
        for (k, v) in keys {
            s.push_str(&format!("{k} = {v}\n"));
        }
        return (s, conflicts);
    };
    let lines: Vec<&str> = text.lines().collect();
    let star = lines.iter().position(|l| l.trim() == "[*]");
    let section_end = |start: usize| {
        lines[start + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map_or(lines.len(), |i| start + 1 + i)
    };
    let mut add = Vec::new();
    for (k, v) in keys {
        let current = star.and_then(|s| {
            lines[s + 1..section_end(s)].iter().find_map(|l| {
                let (lk, lv) = l.split_once('=')?;
                (lk.trim() == *k).then(|| lv.trim().to_string())
            })
        });
        match current {
            Some(c) if c.eq_ignore_ascii_case(v) => {}
            Some(c) => conflicts.push(format!(
                "{k} is `{c}` in .editorconfig, the rule says `{v}`"
            )),
            None => add.push(format!("{k} = {v}")),
        }
    }
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match star {
        Some(s) => {
            let end = section_end(s);
            let mut insert_at = end;
            while insert_at > s + 1 && out[insert_at - 1].trim().is_empty() {
                insert_at -= 1;
            }
            for (i, l) in add.into_iter().enumerate() {
                out.insert(insert_at + i, l);
            }
        }
        None if !add.is_empty() => {
            out.push(String::new());
            out.push("[*]".into());
            out.extend(add);
        }
        None => {}
    }
    (out.join("\n") + "\n", conflicts)
}

/// Add `key = value` under a TOML table header, creating the header at the end if missing.
/// Returns `None` when the key already exists in that table (leave it alone).
fn toml_set(existing: &str, header: &str, key: &str, value: &str) -> Option<String> {
    let lines: Vec<&str> = existing.lines().collect();
    let target = format!("[{header}]");
    if let Some(start) = lines.iter().position(|l| l.trim() == target) {
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map_or(lines.len(), |i| start + 1 + i);
        if lines[start + 1..end]
            .iter()
            .any(|l| l.split_once('=').is_some_and(|(k, _)| k.trim() == key))
        {
            return None;
        }
        let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        out.insert(start + 1, format!("{key} = {value}"));
        return Some(out.join("\n") + "\n");
    }
    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&format!("{target}\n{key} = {value}\n"));
    Some(out)
}

fn pre_commit_hook(vfs: &Vfs<'_>, id: &str, name: &str, entry: &str) -> Artifact {
    let block = format!(
        "  - repo: local\n    hooks:\n      - id: {id}\n        name: {}\n        entry: {}\n        language: system\n        pass_filenames: false\n",
        yaml_str(name),
        yaml_str(entry)
    );
    let path = ".pre-commit-config.yaml";
    match vfs.read(path) {
        None => Artifact {
            path: path.into(),
            action: Action::Create,
            content: format!("repos:\n{block}"),
            note: format!("pre-commit hook `{id}` runs `{entry}`"),
        },
        Some(text) => {
            let candidate = format!("{}\n{block}", text.trim_end());
            let count = |t: &str| {
                serde_yaml_ng::from_str::<serde_yaml_ng::Value>(t)
                    .ok()
                    .and_then(|v| {
                        v.get("repos")
                            .and_then(|r| r.as_sequence())
                            .map(|s| s.len())
                    })
            };
            match (count(&text), count(&candidate)) {
                (Some(a), Some(b)) if b == a + 1 => Artifact {
                    path: path.into(),
                    action: Action::Update,
                    content: candidate,
                    note: format!("pre-commit hook `{id}` runs `{entry}`"),
                },
                _ => Artifact {
                    path: path.into(),
                    action: Action::Manual,
                    content: block,
                    note: "add this hook under `repos:` by hand".into(),
                },
            }
        }
    }
}

struct Planner<'a> {
    ws: &'a Workspace,
    vfs: Vfs<'a>,
    ledger: Ledger,
    taken: Vec<String>,
    configured: BTreeMap<String, Enforcer>,
    ast_grep_rules: usize,
}

impl<'a> Planner<'a> {
    fn id(&mut self, base: &str) -> String {
        let id = self.ledger.unique_id(&slug(base), &self.taken);
        self.taken.push(id.clone());
        id
    }

    fn write(&mut self, path: &str, content: String, note: impl Into<String>) -> Artifact {
        let action = self.vfs.write(path, content.clone());
        Artifact {
            path: path.to_string(),
            action,
            content,
            note: note.into(),
        }
    }

    fn ast_grep(
        &mut self,
        id: &str,
        lang: &str,
        message: &str,
        body: &str,
        scope: &[String],
    ) -> Vec<Artifact> {
        let mut arts = Vec::new();
        let path = format!("{CHECKS_DIR}/{id}.yml");
        let content = ast_grep_rule(id, lang, message, body, scope);
        arts.push(self.write(&path, content, format!("ast-grep rule `{id}` ({lang})")));
        match self.vfs.read("sgconfig.yml") {
            None => {
                arts.push(self.write(
                    "sgconfig.yml",
                    format!("ruleDirs:\n  - {CHECKS_DIR}\n"),
                    "ast-grep project config",
                ));
            }
            Some(t) if !t.contains(CHECKS_DIR) => arts.push(Artifact {
                path: "sgconfig.yml".into(),
                action: Action::Manual,
                content: format!("  - {CHECKS_DIR}\n"),
                note: "add this entry under `ruleDirs:`".into(),
            }),
            Some(_) => {}
        }
        self.ast_grep_rules += 1;
        arts
    }

    fn editorconfig(&mut self, keys: &[(&str, String)]) -> Option<Vec<Artifact>> {
        let existing = self.vfs.read(".editorconfig");
        let (content, conflicts) = editorconfig_set(existing.as_deref(), keys);
        if !conflicts.is_empty() {
            return Some(vec![Artifact {
                path: ".editorconfig".into(),
                action: Action::Manual,
                content: String::new(),
                note: format!("conflict: {}", conflicts.join("; ")),
            }]);
        }
        let note = keys
            .iter()
            .map(|(k, v)| format!("{k} = {v}"))
            .collect::<Vec<_>>()
            .join(", ");
        Some(vec![self.write(".editorconfig", content, note)])
    }

    fn toml_key(&mut self, path: &str, header: &str, key: &str, value: &str) -> Artifact {
        let existing = self.vfs.read(path).unwrap_or_default();
        match toml_set(&existing, header, key, value) {
            Some(content) => self.write(path, content, format!("[{header}] {key} = {value}")),
            None => Artifact {
                path: path.into(),
                action: Action::Manual,
                content: format!("{key} = {value}\n"),
                note: format!("`{key}` is already set in [{header}]; check it matches"),
            },
        }
    }

    fn cargo_lint(&mut self, group: &str, lints: &[&str]) -> Vec<Artifact> {
        let cargo = self.vfs.read("Cargo.toml").unwrap_or_default();
        let header = if cargo.contains("[workspace]") && !cargo.contains("[package]") {
            format!("workspace.lints.{group}")
        } else {
            format!("lints.{group}")
        };
        lints
            .iter()
            .map(|l| self.toml_key("Cargo.toml", &header, l, "\"deny\""))
            .collect()
    }

    fn ruff_select(&mut self, codes: &[&str]) -> Vec<Artifact> {
        let (path, header) = if self.vfs.exists("ruff.toml") {
            ("ruff.toml", "lint")
        } else if self.vfs.exists(".ruff.toml") {
            (".ruff.toml", "lint")
        } else {
            ("pyproject.toml", "tool.ruff.lint")
        };
        let list = codes
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        vec![self.toml_key(path, header, "extend-select", &format!("[{list}]"))]
    }

    fn prettier(&mut self, key: &str, value: serde_json::Value) -> Option<Vec<Artifact>> {
        if !self.vfs.exists("package.json") {
            return None;
        }
        let path = [".prettierrc.json", ".prettierrc"]
            .into_iter()
            .find(|p| self.vfs.exists(p))
            .unwrap_or(".prettierrc.json");
        let existing = self.vfs.read(path);
        let mut obj = match existing
            .as_deref()
            .map(serde_json::from_str::<serde_json::Value>)
        {
            None => serde_json::json!({}),
            Some(Ok(v)) if v.is_object() => v,
            _ => {
                return Some(vec![Artifact {
                    path: path.into(),
                    action: Action::Manual,
                    content: format!("\"{key}\": {value}"),
                    note: "Prettier config is not plain JSON; add this key by hand".into(),
                }]);
            }
        };
        if let Some(cur) = obj.get(key)
            && cur != &value
        {
            return Some(vec![Artifact {
                path: path.into(),
                action: Action::Manual,
                content: String::new(),
                note: format!("conflict: {key} is {cur} in {path}, the rule says {value}"),
            }]);
        }
        obj[key] = value.clone();
        let content = serde_json::to_string_pretty(&obj).expect("json") + "\n";
        Some(vec![self.write(path, content, format!("{key}: {value}"))])
    }

    /// Emit a check for a catalog topic. Returns (enforcer, artifacts) or `None` if this
    /// repo has no way to check it.
    fn emit_topic(
        &mut self,
        topic: &Topic,
        rule: &Rule,
        scope: &[String],
        id: &str,
    ) -> Option<(String, Vec<Artifact>)> {
        let text = rule.text.replace('`', "");
        let lower = text.to_lowercase();
        let rust = has_lang(self.ws, &["Cargo.toml"]);
        let python = has_lang(self.ws, &["pyproject.toml", "setup.py", "requirements.txt"]);
        let js = has_lang(self.ws, &["package.json"]);
        let lang = dominant_language(self.ws, scope);
        let neg = rule.polarity == Polarity::Dont;
        match topic.id.as_str() {
            "indentation" => {
                let mut keys = Vec::new();
                if let Some(c) = NUM_SPACES_RE.captures(&text) {
                    let n = match c[1].to_lowercase().as_str() {
                        "two" => "2".to_string(),
                        "four" => "4".to_string(),
                        "eight" => "8".to_string(),
                        d => d.to_string(),
                    };
                    keys.push(("indent_style", "space".to_string()));
                    keys.push(("indent_size", n));
                } else if lower.contains("tab") && !neg && !lower.contains("space") {
                    keys.push(("indent_style", "tab".to_string()));
                } else if lower.contains("space") {
                    keys.push(("indent_style", "space".to_string()));
                } else {
                    return None;
                }
                Some(("editorconfig".into(), self.editorconfig(&keys)?))
            }
            "line-length" => {
                let n = NUMBER_RE.captures(&text)?[1].to_string();
                Some((
                    "editorconfig".into(),
                    self.editorconfig(&[("max_line_length", n)])?,
                ))
            }
            "whitespace" => {
                let mut keys = Vec::new();
                if lower.contains("trailing")
                    && (lower.contains("whitespace") || lower.contains("space"))
                {
                    keys.push(("trim_trailing_whitespace", "true".to_string()));
                }
                if lower.contains("newline") {
                    keys.push(("insert_final_newline", "true".to_string()));
                }
                if lower.contains("crlf") {
                    keys.push(("end_of_line", "crlf".to_string()));
                } else if lower.contains("lf") && lower.contains("line ending") {
                    keys.push(("end_of_line", "lf".to_string()));
                }
                if keys.is_empty() {
                    return None;
                }
                Some(("editorconfig".into(), self.editorconfig(&keys)?))
            }
            "quotes" => {
                let single = lower.contains("single") && !neg || lower.contains("double") && neg;
                Some((
                    "prettier".into(),
                    self.prettier("singleQuote", serde_json::json!(single))?,
                ))
            }
            "semicolons" => {
                let semi = !(neg || lower.contains("no semicolon") || lower.contains("omit"));
                Some((
                    "prettier".into(),
                    self.prettier("semi", serde_json::json!(semi))?,
                ))
            }
            "trailing-commas" => {
                if js {
                    let v = if neg { "none" } else { "all" };
                    Some((
                        "prettier".into(),
                        self.prettier("trailingComma", serde_json::json!(v))?,
                    ))
                } else {
                    None
                }
            }
            "import-order" => {
                if python {
                    Some(("ruff".into(), self.ruff_select(&["I"])))
                } else if rust {
                    let a = pre_commit_hook(
                        &self.vfs,
                        id,
                        "rustfmt (sorts imports)",
                        "cargo fmt --all --check",
                    );
                    Some(("rustfmt".into(), self.commit(a)))
                } else {
                    None
                }
            }
            "unused" => match lang? {
                "rust" => Some((
                    "rustc".into(),
                    self.cargo_lint("rust", &["unused_imports", "unused_variables"]),
                )),
                "python" => Some(("ruff".into(), self.ruff_select(&["F401", "F841"]))),
                _ => None,
            },
            "naming-case" => match lang? {
                "rust" => Some((
                    "rustc".into(),
                    self.cargo_lint("rust", &["non_snake_case", "non_camel_case_types"]),
                )),
                "python" => Some(("ruff".into(), self.ruff_select(&["N"]))),
                _ => None,
            },
            "unwrap" if rust => {
                let mut any = vec!["- pattern: $X.unwrap()".to_string()];
                if lower.contains("expect") {
                    any.push("- pattern: $X.expect($$$)".into());
                }
                let body = format!("any:\n  {}", any.join("\n  "));
                Some((
                    "ast-grep".into(),
                    self.ast_grep(id, "rust", &rule.text, &body, scope),
                ))
            }
            "console-log" if js => {
                let l = if matches!(lang, Some("typescript") | Some("tsx")) {
                    "typescript"
                } else {
                    "javascript"
                };
                Some((
                    "ast-grep".into(),
                    self.ast_grep(id, l, &rule.text, "pattern: console.log($$$)", scope),
                ))
            }
            "print-calls" if python => Some((
                "ast-grep".into(),
                self.ast_grep(id, "python", &rule.text, "pattern: print($$$)", scope),
            )),
            "explicit-any" if matches!(lang, Some("typescript") | Some("tsx")) => Some((
                "ast-grep".into(),
                self.ast_grep(
                    id,
                    "typescript",
                    &rule.text,
                    "kind: predefined_type\nregex: ^any$",
                    scope,
                ),
            )),
            "type-hints" if python => {
                let a = self.toml_key(
                    "pyproject.toml",
                    "tool.mypy",
                    "disallow_untyped_defs",
                    "true",
                );
                Some(("mypy".into(), vec![a]))
            }
            "strict-equality" if js => {
                let l = if matches!(lang, Some("typescript") | Some("tsx")) {
                    "typescript"
                } else {
                    "javascript"
                };
                Some((
                    "ast-grep".into(),
                    self.ast_grep(
                        id,
                        l,
                        &rule.text,
                        "any:\n  - pattern: $A == $B\n  - pattern: $A != $B",
                        scope,
                    ),
                ))
            }
            "no-var" if js => {
                let l = if matches!(lang, Some("typescript") | Some("tsx")) {
                    "typescript"
                } else {
                    "javascript"
                };
                Some((
                    "ast-grep".into(),
                    self.ast_grep(id, l, &rule.text, "kind: variable_declaration", scope),
                ))
            }
            "format-with-tool" => {
                let (tool, cmd) = [
                    ("prettier", "npx prettier --check ."),
                    ("black", "black --check ."),
                    ("rustfmt", "cargo fmt --all --check"),
                    ("gofmt", "test -z \"$(gofmt -l .)\""),
                    ("ruff", "ruff format --check ."),
                    ("biome", "npx biome format ."),
                    ("clang-format", "clang-format --dry-run --Werror $(git ls-files '*.c' '*.h' '*.cpp' '*.hpp')"),
                ]
                .into_iter()
                .find(|(t, _)| lower.contains(t))?;
                let a = pre_commit_hook(&self.vfs, id, &format!("{tool} check"), cmd);
                Some((tool.to_string(), self.commit(a)))
            }
            _ => None,
        }
    }

    /// Record a pre-built artifact in the virtual file system.
    fn commit(&mut self, a: Artifact) -> Vec<Artifact> {
        if a.action != Action::Manual {
            self.vfs.overlay.insert(a.path.clone(), a.content.clone());
        }
        vec![a]
    }

    /// Structural patterns outside the topic catalog.
    fn emit_structural(
        &mut self,
        rule: &Rule,
        doc: &Doc,
        scope: &[String],
    ) -> Option<(String, String, Vec<Artifact>)> {
        if let Some(c) = PROCESS_RE.captures(&rule.text) {
            let cmd = c[1].trim().to_string();
            let id = self.id(&format!("run-{cmd}"));
            let a = pre_commit_hook(&self.vfs, &id, &cmd, &cmd);
            let arts = self.commit(a);
            return Some((id, "pre-commit".into(), arts));
        }
        if let Some(c) = BOUNDARY_RE.captures(&rule.text) {
            let from = c[1].trim_start_matches("./").to_string();
            let to = c[2]
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_string();
            if !self.ws.discovery.has_dir(&from) {
                return None;
            }
            let lang = dominant_language(self.ws, std::slice::from_ref(&from))?;
            let last = to.rsplit('/').next().unwrap_or(&to).to_string();
            let body = match lang {
                "rust" => format!(
                    "kind: use_declaration\nregex: {}",
                    yaml_str(&format!(r"\b{last}\b"))
                ),
                "python" => format!(
                    "any:\n  - kind: import_statement\n  - kind: import_from_statement\nregex: {}",
                    yaml_str(&format!(r"\b{}\b", to.replace('/', ".")))
                ),
                "go" => format!("kind: import_spec\nregex: {}", yaml_str(&to)),
                _ => format!(
                    "kind: import_statement\nregex: {}",
                    yaml_str(&format!("{last}/|{last}['\\\"]"))
                ),
            };
            let id = self.id(&format!("no-import-{to}-in-{from}"));
            let arts = self.ast_grep(&id, lang, &rule.text, &body, &[from]);
            return Some((id, "ast-grep".into(), arts));
        }
        if rule.polarity == Polarity::Dont {
            // A banned call such as `eval()` or `.unwrap()`.
            let call = doc
                .code_spans
                .iter()
                .filter(|c| rule.span.contains(&c.span.start))
                .map(|c| c.code.trim())
                .find(|c| c.ends_with("()") && !c.contains(' ') && c.len() > 2)?;
            let lang = dominant_language(self.ws, scope)?;
            let name = call.trim_end_matches("()");
            let pattern = match name.strip_prefix('.') {
                Some(m) => format!("$R.{m}($$$)"),
                None => format!("{name}($$$)"),
            };
            if !name
                .trim_start_matches('.')
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
            {
                return None;
            }
            let id = self.id(&format!("no-{name}"));
            let arts = self.ast_grep(&id, lang, &rule.text, &format!("pattern: {pattern}"), scope);
            return Some((id, "ast-grep".into(), arts));
        }
        None
    }
}

/// Whether a rule is phrased like something a tool could check.
fn looks_enforceable(rule: &Rule, doc: &Doc) -> bool {
    let has_code = doc
        .code_spans
        .iter()
        .any(|c| rule.span.contains(&c.span.start));
    let lower = rule.text.to_lowercase();
    has_code
        && (rule.polarity == Polarity::Dont
            || lower.contains("instead of")
            || lower.starts_with("only ")
            || lower.contains(" only "))
}

/// Byte range of the whole list item for `rule` if it can be removed cleanly: a list item with
/// no nested list. Includes a rationale comment on the line above.
pub(crate) fn removal_range(doc: &Doc, rule: &Rule) -> Option<std::ops::Range<usize>> {
    let RuleKind::ListItem { .. } = rule.kind else {
        return None;
    };
    let item = doc
        .lists
        .iter()
        .flat_map(|l| l.items.iter())
        .find(|i| i.start == rule.span.start)?;
    if doc
        .lists
        .iter()
        .any(|l| l.depth > 0 && item.contains(&l.span.start))
    {
        return None;
    }
    let mut start = doc.lines.line_start(doc.line_of(item.start));
    if let Some(c) = doc.comment_above(item.start)
        && rationale::parse(&c.body).is_some()
    {
        start = doc.lines.line_start(doc.line_of(c.span.start));
    }
    let end_line = doc.line_of(item.end.saturating_sub(1).max(item.start));
    let end = if end_line < doc.lines.len() {
        doc.lines.line_start(end_line + 1)
    } else {
        doc.text.len()
    };
    Some(start..end)
}

/// Build a promotion plan for the workspace with the deterministic catalog only.
pub fn plan(ws: &Workspace, config: &Config) -> PromotionPlan {
    plan_with(ws, config, None, None)
}

/// Build a promotion plan. With a drafter, rules the catalog cannot match are sent to it, and
/// drafts that pass validation with `ast_grep` become promotions.
pub fn plan_with(
    ws: &Workspace,
    config: &Config,
    drafter: Option<&dyn Drafter>,
    ast_grep: Option<&std::path::Path>,
) -> PromotionPlan {
    let ledger = ws
        .read_file(LEDGER_FILE)
        .and_then(|t| Ledger::parse(&t).ok())
        .unwrap_or_default();
    let mut p = Planner {
        ws,
        vfs: Vfs {
            ws,
            overlay: BTreeMap::new(),
        },
        taken: Vec::new(),
        configured: enforcers::detect(ws),
        ledger,
        ast_grep_rules: 0,
    };
    let catalog = Catalog::builtin();
    let targets = config.targets.clone();
    let cx = Context::new(ws, config, &targets);
    let mut promotions: Vec<Promotion> = Vec::new();
    let mut unmatched = Vec::new();
    let mut seen_hashes = BTreeSet::new();

    for doc in cx.instruction_docs() {
        for rule in doc.rules.iter().filter(|r| r.imperative) {
            let hash = rationale::rule_hash(&rule.text);
            if p.ledger.by_hash(&hash).is_some() || !seen_hashes.insert(hash.clone()) {
                continue;
            }
            let scope = scope_dirs(rule, doc, ws);
            let line = doc.line_of(rule.span.start);
            let removes_prose = removal_range(doc, rule).is_some();
            let mut push = |id: String,
                            topic: Option<String>,
                            enforcer: String,
                            status: Status,
                            artifacts: Vec<Artifact>| {
                promotions.push(Promotion {
                    retire_when: RetireWhen::Check(id.clone()),
                    id,
                    source: doc.path.clone(),
                    line,
                    rule: rule.text.clone(),
                    hash: hash.clone(),
                    topic,
                    enforcer,
                    status,
                    artifacts,
                    removes_prose,
                    span: rule.span.clone(),
                });
            };

            let topics = catalog.matches(&rule.text);
            let mut handled = false;
            for topic in topics {
                let covered = topic
                    .covered_by(&rule.text, &p.configured)
                    .first()
                    .map(|e| e.id.clone());
                if let Some(enforcer) = covered {
                    let id = p.id(&topic.id);
                    push(
                        id,
                        Some(topic.id.clone()),
                        enforcer,
                        Status::AlreadyEnforced,
                        Vec::new(),
                    );
                    handled = true;
                    break;
                }
                let id_base = topic.id.clone();
                let id = p.ledger.unique_id(&slug(&id_base), &p.taken);
                if let Some((enforcer, arts)) = p.emit_topic(topic, rule, &scope, &id) {
                    p.taken.push(id.clone());
                    push(id, Some(topic.id.clone()), enforcer, Status::Emit, arts);
                    handled = true;
                    break;
                }
            }
            if handled {
                continue;
            }
            if let Some((id, enforcer, arts)) = p.emit_structural(rule, doc, &scope) {
                push(id, None, enforcer, Status::Emit, arts);
                continue;
            }
            if !looks_enforceable(rule, doc) {
                continue;
            }
            let mut outcome = None;
            if let Some(d) = drafter
                && let Some(lang) = dominant_language(ws, &scope)
            {
                let req = DraftRequest {
                    rule: rule.text.clone(),
                    language: lang.to_string(),
                    scope: scope.clone(),
                };
                match d.draft(&req) {
                    Ok(draft) => match crate::drafter::validate(&draft, lang, ast_grep) {
                        Validation::Passed => {
                            let id = p.id(&format!("draft-{}", rule.text));
                            let arts = p.ast_grep(&id, lang, &rule.text, &draft.rule, &scope);
                            push(id, None, "ast-grep".into(), Status::Drafted, arts);
                            continue;
                        }
                        v => {
                            outcome = Some(DraftOutcome {
                                rule: Some(draft.rule),
                                validation: v,
                            })
                        }
                    },
                    Err(e) => {
                        outcome = Some(DraftOutcome {
                            rule: None,
                            validation: Validation::Failed(e.to_string()),
                        })
                    }
                }
            }
            unmatched.push(Unmatched {
                source: doc.path.clone(),
                line,
                rule: rule.text.clone(),
                draft: outcome,
            });
        }
    }
    finish(ws, p.vfs.overlay, p.ledger, promotions, unmatched)
}

/// Assemble doc edits, the ledger and the diff.
fn finish(
    ws: &Workspace,
    mut files: BTreeMap<String, String>,
    mut ledger: Ledger,
    promotions: Vec<Promotion>,
    unmatched: Vec<Unmatched>,
) -> PromotionPlan {
    // Remove promoted prose from docs.
    let mut removals: BTreeMap<String, Vec<std::ops::Range<usize>>> = BTreeMap::new();
    for pr in &promotions {
        let fully_applied = pr.artifacts.iter().all(|a| a.action != Action::Manual);
        if !pr.removes_prose || !fully_applied {
            continue;
        }
        if let Some(doc) = ws.doc(&pr.source)
            && let Some(rule) = doc.rules.iter().find(|r| r.span == pr.span)
            && let Some(r) = removal_range(doc, rule)
        {
            removals.entry(pr.source.clone()).or_default().push(r);
        }
    }
    for (path, mut ranges) in removals {
        let Some(doc) = ws.doc(&path) else { continue };
        ranges.sort_by_key(|r| r.start);
        let mut out = String::new();
        let mut pos = 0;
        for r in ranges {
            if r.start >= pos {
                out.push_str(&doc.text[pos..r.start]);
                pos = r.end;
            }
        }
        out.push_str(&doc.text[pos..]);
        files.insert(path, out);
    }

    // Ledger.
    let date = ledger::today();
    for pr in &promotions {
        let applied = pr.artifacts.iter().all(|a| a.action != Action::Manual);
        ledger.upsert(Entry {
            id: pr.id.clone(),
            hash: pr.hash.clone(),
            text: pr.rule.clone(),
            source: pr.source.clone(),
            enforcer: pr.enforcer.clone(),
            artifacts: pr
                .artifacts
                .iter()
                .filter(|a| a.action != Action::Manual)
                .map(|a| a.path.clone())
                .collect(),
            date: date.clone(),
            state: if applied {
                State::Applied
            } else {
                State::Proposed
            },
            retire_when: Some(pr.retire_when.clone()),
        });
    }
    if !promotions.is_empty() {
        files.insert(LEDGER_FILE.to_string(), ledger.to_toml());
    }

    let mut diff = String::new();
    for (path, after) in &files {
        let before = ws.read_file(path).unwrap_or_default();
        if &before == after {
            continue;
        }
        let a = if ws.discovery.has_file(path) {
            format!("a/{path}")
        } else {
            "/dev/null".to_string()
        };
        diff.push_str(&similar::udiff::unified_diff(
            similar::Algorithm::Myers,
            &before,
            after,
            3,
            Some((&a, &format!("b/{path}"))),
        ));
    }
    let files_changed = files.keys().cloned().collect();
    PromotionPlan {
        promotions,
        unmatched,
        diff,
        files,
        files_changed,
    }
}

/// Write every Create/Update artifact, the doc edits and the ledger. Returns the paths written.
pub fn apply(ws: &Workspace, plan: &PromotionPlan) -> std::io::Result<Vec<String>> {
    let mut written = Vec::new();
    for (path, content) in &plan.files {
        let full = ws.root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full, content)?;
        written.push(path.clone());
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::ws;

    fn plan_for(files: &[(&str, &str)]) -> PromotionPlan {
        let w = ws(files);
        plan(&w, &Config::default())
    }

    #[test]
    fn indentation_creates_editorconfig_and_removes_prose() {
        let p = plan_for(&[(
            "AGENTS.md",
            "# Rules\n\n- Use 2 spaces for indentation.\n- Keep PRs small.\n",
        )]);
        assert_eq!(p.promotions.len(), 1, "{:?}", p.promotions);
        let pr = &p.promotions[0];
        assert_eq!(pr.status, Status::Emit);
        assert_eq!(pr.enforcer, "editorconfig");
        let ec = &p.files[".editorconfig"];
        assert!(ec.contains("indent_style = space") && ec.contains("indent_size = 2"));
        assert_eq!(p.files["AGENTS.md"], "# Rules\n\n- Keep PRs small.\n");
        assert!(p.files[LEDGER_FILE].contains("id = \"indentation\""));
        assert!(p.diff.contains("+++ b/.editorconfig") && p.diff.contains("-- Use 2 spaces"));
    }

    #[test]
    fn already_enforced_only_removes_prose() {
        let p = plan_for(&[
            (
                ".editorconfig",
                "root = true\n\n[*]\nindent_style = space\nindent_size = 2\n",
            ),
            ("AGENTS.md", "- Use 2 spaces for indentation.\n"),
        ]);
        assert_eq!(p.promotions[0].status, Status::AlreadyEnforced);
        assert!(p.promotions[0].artifacts.is_empty());
        assert!(!p.files.contains_key(".editorconfig"));
        assert_eq!(p.files["AGENTS.md"], "");
    }

    #[test]
    fn editorconfig_merge_and_conflict() {
        let (merged, c) = editorconfig_set(
            Some("root = true\n\n[*]\ncharset = utf-8\n\n[*.md]\nx = 1\n"),
            &[("indent_size", "2".into())],
        );
        assert!(c.is_empty());
        assert_eq!(
            merged,
            "root = true\n\n[*]\ncharset = utf-8\nindent_size = 2\n\n[*.md]\nx = 1\n"
        );
        let (_, c) = editorconfig_set(
            Some("[*]\nindent_size = 4\n"),
            &[("indent_size", "2".into())],
        );
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn unwrap_in_src_becomes_scoped_ast_grep_rule() {
        let p = plan_for(&[
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
            ("src/lib.rs", "fn a() {}\n"),
            ("AGENTS.md", "- Never call `.unwrap()` in `src/`.\n"),
        ]);
        let pr = &p.promotions[0];
        assert_eq!(pr.enforcer, "ast-grep");
        let rule = &p.files[".mnemoa/checks/unwrap.yml"];
        assert!(
            rule.contains("language: rust")
                && rule.contains("pattern: $X.unwrap()")
                && rule.contains("\"src/**\""),
            "{rule}"
        );
        assert!(p.files["sgconfig.yml"].contains(".mnemoa/checks"));
    }

    #[test]
    fn process_rule_becomes_pre_commit_hook() {
        let p = plan_for(&[
            ("Makefile", "lint:\n\tx\n"),
            ("AGENTS.md", "- Always run `make lint` before committing.\n"),
        ]);
        let pr = &p.promotions[0];
        assert_eq!(pr.enforcer, "pre-commit");
        let pc = &p.files[".pre-commit-config.yaml"];
        assert!(pc.contains("entry: \"make lint\""));
        let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(pc).unwrap();
        assert_eq!(v["repos"].as_sequence().unwrap().len(), 1);
    }

    #[test]
    fn pre_commit_append_keeps_existing_hooks() {
        let p = plan_for(&[
            (
                ".pre-commit-config.yaml",
                "repos:\n  - repo: https://github.com/x/y\n    rev: v1\n    hooks:\n      - id: z\n",
            ),
            ("AGENTS.md", "- Run `cargo test` before pushing.\n"),
        ]);
        let v: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&p.files[".pre-commit-config.yaml"]).unwrap();
        assert_eq!(v["repos"].as_sequence().unwrap().len(), 2);
    }

    #[test]
    fn import_boundary_becomes_ast_grep_rule() {
        let p = plan_for(&[
            ("src/api/mod.ts", "export const a = 1;\n"),
            ("src/internal/x.ts", "export const b = 1;\n"),
            (
                "AGENTS.md",
                "- `src/api/` must not import from `src/internal/`.\n",
            ),
        ]);
        let pr = &p.promotions[0];
        assert_eq!(pr.enforcer, "ast-grep");
        let rule = p
            .files
            .iter()
            .find(|(k, _)| k.starts_with(CHECKS_DIR))
            .unwrap()
            .1;
        assert!(
            rule.contains("kind: import_statement") && rule.contains("\"src/api/**\""),
            "{rule}"
        );
    }

    #[test]
    fn banned_call_and_unmatched() {
        let p = plan_for(&[
            ("app/main.py", "x = 1\n"),
            ("pyproject.toml", "[project]\nname = \"x\"\n"),
            (
                "AGENTS.md",
                "- Never use `eval()` in `app/`.\n- Never edit `schema.sql` by hand.\n- Write clear commit messages.\n",
            ),
        ]);
        assert_eq!(p.promotions.len(), 1, "{:?}", p.promotions);
        assert!(
            p.files
                .iter()
                .any(|(k, v)| k.starts_with(CHECKS_DIR) && v.contains("pattern: eval($$$)"))
        );
        assert_eq!(p.unmatched.len(), 1);
        assert!(p.unmatched[0].rule.contains("schema.sql"));
    }

    #[test]
    fn rules_in_ledger_are_skipped() {
        let hash = rationale::rule_hash("Use 2 spaces for indentation.");
        let ledger = format!(
            "[[check]]\nid = \"indentation\"\nhash = \"{hash}\"\ntext = \"x\"\nsource = \"AGENTS.md\"\nenforcer = \"editorconfig\"\ndate = \"2026-01-01\"\nstate = \"applied\"\n"
        );
        let p = plan_for(&[
            (LEDGER_FILE, ledger.as_str()),
            ("AGENTS.md", "- Use 2 spaces for indentation.\n"),
        ]);
        assert!(p.promotions.is_empty());
    }

    #[test]
    fn nested_items_and_sentences_keep_prose() {
        let p = plan_for(&[
            (
                "AGENTS.md",
                "- Use 2 spaces for indentation.\n  - except in Makefiles\n\nAlways use single quotes.\n",
            ),
            ("package.json", "{}"),
        ]);
        assert_eq!(p.promotions.len(), 2);
        assert!(p.promotions.iter().all(|p| !p.removes_prose));
        assert!(!p.files.contains_key("AGENTS.md"));
    }

    #[test]
    fn rationale_comment_is_removed_with_rule() {
        let p = plan_for(&[(
            "AGENTS.md",
            "<!-- why: diffs | retire-when: manual -->\n- Use 4 spaces for indentation.\n- Other.\n",
        )]);
        assert_eq!(p.files["AGENTS.md"], "- Other.\n");
    }

    #[test]
    fn apply_writes_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "- Use 2 spaces for indentation.\n- Keep it short.\n",
        )
        .unwrap();
        let w = Workspace::load(dir.path(), &Default::default()).unwrap();
        let p = plan(&w, &Config::default());
        let written = apply(&w, &p).unwrap();
        assert!(written.contains(&".editorconfig".to_string()));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(),
            "- Keep it short.\n"
        );
        assert!(dir.path().join(LEDGER_FILE).is_file());
        // Running again finds nothing new.
        let w = Workspace::load(dir.path(), &Default::default()).unwrap();
        assert!(plan(&w, &Config::default()).promotions.is_empty());
    }

    #[test]
    fn drafter_results_need_validation() {
        let w = ws(&[
            ("app/main.py", "x = 1\n"),
            ("AGENTS.md", "- Never edit `settings.SECRET` directly.\n"),
        ]);
        let d = crate::drafter::StaticDrafter {
            reply: r#"{"rule": "pattern: settings.SECRET = $V", "positive": ["settings.SECRET = 1"], "negative": ["x = settings.SECRET"]}"#.into(),
        };
        // Without ast-grep the draft stays unmatched and unapplied.
        let p = plan_with(&w, &Config::default(), Some(&d), None);
        assert!(p.promotions.is_empty());
        assert!(matches!(
            p.unmatched[0].draft.as_ref().unwrap().validation,
            Validation::Unvalidated(_)
        ));
        assert!(!p.files.contains_key("AGENTS.md"));
        if let Some(ag) = crate::drafter::find_ast_grep() {
            let p = plan_with(&w, &Config::default(), Some(&d), Some(&ag));
            assert_eq!(p.promotions.len(), 1, "{:?}", p.unmatched);
            assert_eq!(p.promotions[0].status, Status::Drafted);
            assert_eq!(p.files["AGENTS.md"], "");
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("run-make lint"), "run-make-lint");
        assert_eq!(slug("no-.unwrap"), "no-unwrap");
    }
}
