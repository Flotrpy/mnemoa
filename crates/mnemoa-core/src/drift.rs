//! Drift between agent docs (instructions, plans, specs) and the code.
//!
//! Reference drift resolves every path, backticked symbol and command a doc mentions against the
//! current tree. With a base revision, files and symbols that the diff removed or renamed fail
//! even when the doc itself was not touched.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::diag::{Severity, Summary};
use crate::doc::Doc;
use crate::history::{ChangeKind, History, HistoryError};
use crate::rules::Context;
use crate::rules::commands::stale_commands;
use crate::rules::references::{has_extension_or_slash, looks_like_path, percent_decode};
use crate::symbols::{self, SymbolIndex};
use crate::workspace::{Workspace, resolve_relative};

/// What kind of drift an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DriftKind {
    MissingPath,
    MissingSymbol,
    StaleCommand,
    RemovedInDiff,
    RenamedInDiff,
    SymbolRemovedInDiff,
    /// `- [x]` task whose file does not exist.
    DoneTaskMissingFile,
    /// `- [ ]` task whose file exists and changed after the task list.
    OpenTaskFileChanged,
    MalformedTask,
    /// Memory-bank file not updated while code moved on.
    StaleMemoryBank,
}

impl DriftKind {
    pub fn severity(self) -> Severity {
        match self {
            DriftKind::MissingPath
            | DriftKind::RemovedInDiff
            | DriftKind::RenamedInDiff
            | DriftKind::SymbolRemovedInDiff
            | DriftKind::DoneTaskMissingFile => Severity::Error,
            DriftKind::MissingSymbol | DriftKind::StaleCommand | DriftKind::StaleMemoryBank => {
                Severity::Warn
            }
            DriftKind::OpenTaskFileChanged | DriftKind::MalformedTask => Severity::Info,
        }
    }
}

/// One drifted reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftItem {
    pub kind: DriftKind,
    pub severity: Severity,
    /// The doc that holds the reference.
    pub path: String,
    pub line: usize,
    pub column: usize,
    /// The referenced path, symbol or command.
    pub reference: String,
    pub message: String,
}

impl DriftItem {
    pub fn new(
        kind: DriftKind,
        doc: &Doc,
        offset: usize,
        reference: &str,
        message: impl Into<String>,
    ) -> Self {
        let (line, column) = doc.line_col(offset);
        DriftItem {
            kind,
            severity: kind.severity(),
            path: doc.path.clone(),
            line,
            column,
            reference: reference.to_string(),
            message: message.into(),
        }
    }
}

/// How far code referenced by a doc moved since the doc last changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freshness {
    pub path: String,
    /// Commit that last changed the doc.
    pub last_commit: String,
    /// Commits since then that touched paths the doc references.
    pub commits_since: usize,
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftReport {
    pub version: String,
    pub root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub items: Vec<DriftItem>,
    pub freshness: Vec<Freshness>,
    pub summary: Summary,
    /// Whether git history was available.
    pub history: bool,
}

impl DriftReport {
    pub fn exit_code(&self, strict: bool) -> i32 {
        if self.summary.errors > 0 || (strict && self.summary.warnings > 0) {
            1
        } else {
            0
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DriftOptions {
    /// Base revision for diff-aware mode.
    pub base: Option<String>,
}

/// A reference extracted from a doc.
#[derive(Debug, Clone)]
struct Ref {
    text: String,
    /// Resolved repo-relative path, for path references.
    resolved: Option<String>,
    offset: usize,
    symbol: Option<String>,
    from_link: bool,
}

fn refs(doc: &Doc) -> Vec<Ref> {
    let mut out = Vec::new();
    for l in &doc.links {
        let dest = l.dest.trim();
        if dest.is_empty()
            || dest.starts_with('#')
            || dest.contains("://")
            || dest.starts_with("mailto:")
            || dest.contains(['<', '>', '{', '}', '$', '*'])
        {
            continue;
        }
        let path = percent_decode(dest.split(['#', '?']).next().unwrap_or(""));
        if path.is_empty() {
            continue;
        }
        let resolved = match path.strip_prefix('/') {
            Some(abs) => Some(abs.trim_end_matches('/').to_string()),
            None => resolve_relative(&doc.path, path.trim_end_matches('/')),
        };
        out.push(Ref {
            text: path,
            resolved,
            offset: l.span.start,
            symbol: None,
            from_link: true,
        });
    }
    for i in &doc.imports {
        if i.path.starts_with('~') || i.path.starts_with('/') || !has_extension_or_slash(&i.path) {
            continue;
        }
        out.push(Ref {
            text: i.path.clone(),
            resolved: resolve_relative(&doc.path, &i.path),
            offset: i.span.start,
            symbol: None,
            from_link: true,
        });
    }
    for c in &doc.code_spans {
        let code = c.code.trim();
        if looks_like_path(code) {
            out.push(Ref {
                text: code.to_string(),
                resolved: Some(
                    code.trim_start_matches("./")
                        .trim_end_matches('/')
                        .to_string(),
                ),
                offset: c.span.start,
                symbol: None,
                from_link: false,
            });
        } else if let Some(sym) = symbols::symbol_in_span(code) {
            out.push(Ref {
                text: code.to_string(),
                resolved: None,
                offset: c.span.start,
                symbol: Some(sym.to_string()),
                from_link: false,
            });
        }
    }
    out
}

fn is_plan(ws: &Workspace, path: &str) -> bool {
    ws.discovery.surface(path).is_some_and(|s| s.kind.is_plan())
}

/// Docs checked for drift: instruction docs and plan docs.
fn drift_docs(ws: &Workspace) -> Vec<&Doc> {
    ws.docs.values().collect()
}

/// Compute drift for a workspace.
pub fn drift(
    ws: &Workspace,
    config: &Config,
    history: Option<&dyn History>,
    opts: &DriftOptions,
) -> Result<DriftReport, HistoryError> {
    let index = SymbolIndex::build(ws);
    let targets = config.targets.clone();
    let cx = Context::new(ws, config, &targets).with_history(history);
    let disc = &ws.discovery;
    let mut items = Vec::new();

    // Diff-aware sets.
    let mut deleted: BTreeSet<String> = BTreeSet::new();
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    let mut removed_symbols: BTreeMap<String, String> = BTreeMap::new();
    if let (Some(base), Some(h)) = (&opts.base, history) {
        for ch in h.diff_from(base)? {
            match ch.kind {
                ChangeKind::Deleted => {
                    deleted.insert(ch.path.clone());
                }
                ChangeKind::Renamed => {
                    if let Some(old) = &ch.old_path {
                        renamed.insert(old.clone(), ch.path.clone());
                    }
                }
                _ => {}
            }
            let old_path = ch.old_path.as_deref().unwrap_or(&ch.path);
            if ch.kind != ChangeKind::Added
                && symbols::is_source(old_path)
                && let Some(text) = h.file_at(base, old_path)?
            {
                for name in symbols::declarations(old_path, &text) {
                    if !index.declared.contains_key(&name) && !index.knows(&name) {
                        removed_symbols.insert(name, old_path.to_string());
                    }
                }
            }
        }
    }

    for doc in drift_docs(ws) {
        let plan = is_plan(ws, &doc.path);
        for r in refs(doc) {
            if let Some(sym) = &r.symbol {
                if let Some(file) = removed_symbols.get(sym) {
                    items.push(DriftItem::new(
                        DriftKind::SymbolRemovedInDiff,
                        doc,
                        r.offset,
                        &r.text,
                        format!(
                            "`{}` was removed from {file} since {}.",
                            r.text,
                            opts.base.as_deref().unwrap_or("base")
                        ),
                    ));
                } else if !plan && index.files > 0 && !index.knows(sym) {
                    items.push(DriftItem::new(
                        DriftKind::MissingSymbol,
                        doc,
                        r.offset,
                        &r.text,
                        format!("`{}` does not appear anywhere in the source code.", r.text),
                    ));
                }
                continue;
            }
            let Some(p) = r.resolved.clone() else {
                continue;
            };
            if disc.exists(&p) {
                continue;
            }
            if let Some(new) = renamed.get(&p) {
                items.push(DriftItem::new(
                    DriftKind::RenamedInDiff,
                    doc,
                    r.offset,
                    &r.text,
                    format!(
                        "`{}` was renamed to `{new}` since {}.",
                        r.text,
                        opts.base.as_deref().unwrap_or("base")
                    ),
                ));
                continue;
            }
            let prefix = format!("{p}/");
            if deleted.contains(&p) || deleted.iter().any(|d| d.starts_with(&prefix)) {
                items.push(DriftItem::new(
                    DriftKind::RemovedInDiff,
                    doc,
                    r.offset,
                    &r.text,
                    format!(
                        "`{}` was deleted since {}.",
                        r.text,
                        opts.base.as_deref().unwrap_or("base")
                    ),
                ));
                continue;
            }
            // Without a diff, only report paths we are confident about: links, or code paths
            // outside plans whose top-level directory exists.
            let first = p.split('/').next().unwrap_or("");
            let confident = r.from_link
                || (!plan
                    && has_extension_or_slash(&r.text)
                    && p.contains('/')
                    && disc.has_dir(first));
            if confident {
                items.push(DriftItem::new(
                    DriftKind::MissingPath,
                    doc,
                    r.offset,
                    &r.text,
                    format!("`{}` does not exist.", r.text),
                ));
            }
        }
        if !plan {
            for (cmd, span, msg) in stale_commands(&cx, doc) {
                items.push(DriftItem::new(
                    DriftKind::StaleCommand,
                    doc,
                    span.start,
                    &cmd,
                    msg,
                ));
            }
        }
    }

    // Freshness per doc.
    let mut freshness = Vec::new();
    if let Some(h) = history {
        for doc in drift_docs(ws) {
            let touching = h.touching(&doc.path);
            let Some(&last) = touching.first() else {
                continue;
            };
            let existing: BTreeSet<String> = refs(doc)
                .into_iter()
                .filter_map(|r| r.resolved)
                .filter(|p| disc.exists(p) && p != &doc.path)
                .collect();
            let existing: Vec<String> = existing.into_iter().collect();
            freshness.push(Freshness {
                path: doc.path.clone(),
                last_commit: h.commits()[last].id.clone(),
                commits_since: if existing.is_empty() {
                    0
                } else {
                    h.commits_touching_since(last, &existing)
                },
                references: existing,
            });
        }
    }

    items.sort_by(|a, b| {
        (a.path.as_str(), a.line, a.column).cmp(&(b.path.as_str(), b.line, b.column))
    });
    items.dedup_by(|a, b| {
        a.path == b.path && a.line == b.line && a.column == b.column && a.reference == b.reference
    });
    let summary = summarize(&items);
    Ok(DriftReport {
        version: crate::VERSION.to_string(),
        root: ws.root.display().to_string(),
        base: opts.base.clone(),
        items,
        freshness,
        summary,
        history: history.is_some(),
    })
}

pub(crate) fn summarize(items: &[DriftItem]) -> Summary {
    let mut s = Summary::default();
    for i in items {
        match i.severity {
            Severity::Error => s.errors += 1,
            Severity::Warn => s.warnings += 1,
            Severity::Info => s.infos += 1,
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{FileChange, MemoryHistory};
    use crate::workspace::tests::ws;

    fn run(files: &[(&str, &str)], h: Option<&MemoryHistory>, base: Option<&str>) -> DriftReport {
        let w = ws(files);
        drift(
            &w,
            &Config::default(),
            h.map(|h| h as &dyn History),
            &DriftOptions {
                base: base.map(str::to_string),
            },
        )
        .unwrap()
    }

    #[test]
    fn missing_link_and_symbol_and_command() {
        let r = run(
            &[
                (
                    "AGENTS.md",
                    "- See [arch](docs/arch.md) before refactors.\n- Call `load_config()` first.\n- `ScanReport` holds results.\n- Run `make check`.\n",
                ),
                ("src/lib.rs", "pub struct ScanReport;\n"),
                ("Makefile", "test:\n\tx\n"),
            ],
            None,
            None,
        );
        let kinds: Vec<DriftKind> = r.items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DriftKind::MissingPath,
                DriftKind::MissingSymbol,
                DriftKind::StaleCommand
            ],
            "{:?}",
            r.items
        );
        assert_eq!(r.summary.errors, 1);
        assert_eq!(r.exit_code(false), 1);
    }

    #[test]
    fn clean_docs_have_no_drift() {
        let r = run(
            &[
                (
                    "AGENTS.md",
                    "- Read [x](src/lib.rs) when changing `scan_repo()`.\n- `HashMap` is fine.\n",
                ),
                (
                    "src/lib.rs",
                    "use std::collections::HashMap;\npub fn scan_repo() {}\n",
                ),
            ],
            None,
            None,
        );
        assert!(r.items.is_empty(), "{:?}", r.items);
    }

    #[test]
    fn plans_do_not_report_future_symbols_or_paths() {
        let r = run(
            &[
                ("specs/001-x/plan.md", "Add `NewThing` in `src/new.rs`.\n"),
                ("src/lib.rs", "fn a() {}\n"),
            ],
            None,
            None,
        );
        assert!(r.items.is_empty(), "{:?}", r.items);
    }

    #[test]
    fn diff_aware_reports_removed_and_renamed() {
        let mut h = MemoryHistory::linear(&[
            ("a", &["AGENTS.md", "src/old.rs", "src/gone.rs"]),
            ("b", &["src/new.rs"]),
        ]);
        h.diffs.insert(
            "main".into(),
            vec![
                FileChange::renamed("src/old.rs", "src/new.rs"),
                FileChange::new(ChangeKind::Deleted, "src/gone.rs"),
            ],
        );
        h.files.insert(
            ("main".into(), "src/gone.rs".into()),
            "pub fn legacy_entry() {}\n".into(),
        );
        h.files.insert(
            ("main".into(), "src/old.rs".into()),
            "pub fn kept() {}\n".into(),
        );
        let r = run(
            &[
                (
                    "AGENTS.md",
                    "- Edit `src/old.rs` for parsing.\n- `src/gone.rs` holds `legacy_entry()`.\n",
                ),
                ("src/new.rs", "pub fn kept() {}\n"),
            ],
            Some(&h),
            Some("main"),
        );
        let kinds: BTreeSet<DriftKind> = r.items.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&DriftKind::RenamedInDiff), "{:?}", r.items);
        assert!(kinds.contains(&DriftKind::RemovedInDiff));
        assert!(kinds.contains(&DriftKind::SymbolRemovedInDiff));
        assert!(r.items.iter().any(|i| i.message.contains("src/new.rs")));
    }

    #[test]
    fn unknown_base_is_an_error() {
        let w = ws(&[("AGENTS.md", "x")]);
        let h = MemoryHistory::default();
        let e = drift(
            &w,
            &Config::default(),
            Some(&h),
            &DriftOptions {
                base: Some("nope".into()),
            },
        );
        assert!(e.is_err());
    }

    #[test]
    fn freshness_counts_commits_touching_references() {
        let h = MemoryHistory::linear(&[
            ("a", &["AGENTS.md", "src/lib.rs"]),
            ("b", &["src/lib.rs"]),
            ("c", &["src/lib.rs"]),
            ("d", &["other.rs"]),
        ]);
        let r = run(
            &[
                (
                    "AGENTS.md",
                    "- Read [lib](src/lib.rs) first when editing.\n",
                ),
                ("src/lib.rs", "fn a(){}"),
                ("other.rs", ""),
            ],
            Some(&h),
            None,
        );
        let f = r.freshness.iter().find(|f| f.path == "AGENTS.md").unwrap();
        assert_eq!(f.commits_since, 2);
        assert_eq!(f.references, vec!["src/lib.rs"]);
    }
}
