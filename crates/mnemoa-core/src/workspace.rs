//! A loaded repository: discovery, parsed documents, and the per-target context set.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::discover::{self, DiscoverError, DiscoverOptions, Discovery, Loading, Target};
use crate::doc::{Doc, Import};

/// Claude Code expands imports at most this many hops deep.
pub const CLAUDE_IMPORT_MAX_DEPTH: usize = 4;

/// Codex stops adding project guidance past this many bytes (default `project_doc_max_bytes`).
pub const CODEX_MAX_BYTES: usize = 32 * 1024;

/// A file in a target's always-loaded context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoadedFile {
    pub path: String,
    /// The file that imported this one, when it was pulled in by an `@import`.
    pub via: Option<String>,
    /// Import depth (0 for files the target loads directly).
    pub depth: usize,
}

/// An import Claude Code will not follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DroppedImport {
    pub from: String,
    pub path: String,
    pub reason: String,
}

/// A repository with its surfaces parsed.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub discovery: Discovery,
    /// Parsed documents by relative path: every Markdown surface plus imported files.
    pub docs: BTreeMap<String, Doc>,
    /// In-memory file contents; when set, [`Workspace::read_file`] reads from here instead of disk.
    pub mem: Option<BTreeMap<String, String>>,
}

/// Resolve an import or link target relative to the importing file's directory.
/// Returns `None` for absolute paths, home-relative paths and paths that escape the root.
pub fn resolve_relative(from_file: &str, target: &str) -> Option<String> {
    if target.starts_with('/') || target.starts_with('~') {
        return None;
    }
    let mut parts: Vec<&str> = from_file.split('/').collect();
    parts.pop();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

impl Workspace {
    /// Discover and parse a repository.
    pub fn load(root: &Path, opts: &DiscoverOptions) -> Result<Self, DiscoverError> {
        let discovery = discover::discover(root, opts)?;
        Ok(Self::from_discovery(discovery, |p| {
            std::fs::read_to_string(root.join(p)).ok()
        }))
    }

    /// Build from an existing discovery, reading files through `read`.
    pub fn from_discovery(discovery: Discovery, read: impl Fn(&str) -> Option<String>) -> Self {
        let mut docs = BTreeMap::new();
        let mut queue: Vec<String> = discovery.surfaces.iter().map(|s| s.path.clone()).collect();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(path) = queue.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some(text) = read(&path) else { continue };
            let doc = Doc::parse(&path, &text);
            for imp in &doc.imports {
                if let Some(p) = resolve_relative(&path, &imp.path)
                    && discovery.has_file(&p)
                    && is_text_like(&p)
                {
                    queue.push(p);
                }
            }
            docs.insert(path, doc);
        }
        Workspace {
            root: discovery.root.clone(),
            discovery,
            docs,
            mem: None,
        }
    }

    /// Read any repository file (not just surfaces) by relative path.
    pub fn read_file(&self, path: &str) -> Option<String> {
        match &self.mem {
            Some(m) => m.get(path).cloned(),
            None => std::fs::read_to_string(self.root.join(path)).ok(),
        }
    }

    pub fn doc(&self, path: &str) -> Option<&Doc> {
        self.docs.get(path)
    }

    /// Files `target` loads from the first turn, including Claude Code imports, in load order.
    pub fn always_loaded(&self, target: Target) -> Vec<LoadedFile> {
        self.context(target).0
    }

    /// Imports Claude Code drops because they are too deep or point outside the repo.
    pub fn dropped_imports(&self) -> Vec<DroppedImport> {
        self.context(Target::ClaudeCode).1
    }

    fn context(&self, target: Target) -> (Vec<LoadedFile>, Vec<DroppedImport>) {
        let mut out: Vec<LoadedFile> = Vec::new();
        let mut dropped = Vec::new();
        let mut seen = BTreeSet::new();
        for s in &self.discovery.surfaces {
            if s.loading_for(target).is_some_and(Loading::is_always) && seen.insert(s.path.clone())
            {
                out.push(LoadedFile {
                    path: s.path.clone(),
                    via: None,
                    depth: 0,
                });
            }
        }
        if target != Target::ClaudeCode {
            return (out, dropped);
        }
        // Expand imports breadth-first from each directly loaded file.
        let mut i = 0;
        while i < out.len() {
            let file = out[i].clone();
            i += 1;
            let Some(doc) = self.docs.get(&file.path) else {
                continue;
            };
            for imp in &doc.imports {
                match self.resolve_import(&file.path, imp) {
                    Ok(p) => {
                        if file.depth >= CLAUDE_IMPORT_MAX_DEPTH {
                            dropped.push(DroppedImport {
                                from: file.path.clone(),
                                path: p,
                                reason: format!(
                                    "more than {CLAUDE_IMPORT_MAX_DEPTH} import hops from a loaded file"
                                ),
                            });
                        } else if seen.insert(p.clone()) {
                            out.push(LoadedFile {
                                path: p,
                                via: Some(file.path.clone()),
                                depth: file.depth + 1,
                            });
                        }
                    }
                    Err(Some(reason)) => dropped.push(DroppedImport {
                        from: file.path.clone(),
                        path: imp.path.clone(),
                        reason,
                    }),
                    Err(None) => {}
                }
            }
        }
        (out, dropped)
    }

    /// Resolve an import to a repo file. `Err(None)` means "not a file import" (stay silent).
    pub fn resolve_import(&self, from: &str, imp: &Import) -> Result<String, Option<String>> {
        if imp.path.starts_with('~') || imp.path.starts_with('/') {
            return Err(Some(
                "points outside the repository; it needs per-user approval".to_string(),
            ));
        }
        match resolve_relative(from, &imp.path) {
            Some(p) if self.discovery.has_file(&p) => Ok(p),
            _ => Err(None),
        }
    }

    /// Whether `path` is loaded from the first turn by any of `targets`.
    pub fn is_always_loaded(&self, path: &str, targets: &[Target]) -> bool {
        targets
            .iter()
            .any(|t| self.always_loaded(*t).iter().any(|f| f.path == path))
    }

    /// Paths always loaded by at least one of `targets`, deduplicated and sorted.
    pub fn always_loaded_union(&self, targets: &[Target]) -> Vec<String> {
        let mut set = BTreeSet::new();
        for t in targets {
            for f in self.always_loaded(*t) {
                set.insert(f.path);
            }
        }
        set.into_iter().collect()
    }
}

fn is_text_like(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md", ".mdc", ".txt", ".markdown"]
        .iter()
        .any(|e| lower.ends_with(e))
        || !lower.rsplit('/').next().unwrap_or("").contains('.')
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build a workspace from in-memory files (used by rule tests across the crate).
    pub(crate) fn ws(files: &[(&str, &str)]) -> Workspace {
        ws_with(files, &[])
    }

    pub(crate) fn ws_with(files: &[(&str, &str)], include: &[&str]) -> Workspace {
        let map: HashMap<String, String> = files
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect();
        let mut names: Vec<String> = map.keys().cloned().collect();
        names.sort();
        let mut dirs = BTreeSet::new();
        for n in &names {
            let mut d = n.as_str();
            while let Some(i) = d.rfind('/') {
                d = &d[..i];
                dirs.insert(d.to_string());
            }
        }
        let mut d = Discovery {
            files: names,
            dirs: dirs.into_iter().collect(),
            ..Default::default()
        };
        let mut b = globset::GlobSetBuilder::new();
        for i in include {
            b.add(globset::Glob::new(i).unwrap());
        }
        let inc = b.build().unwrap();
        crate::discover::classify(&mut d, &inc, &|p| map.get(p).cloned().unwrap_or_default());
        let mut w = Workspace::from_discovery(d, |p| map.get(p).cloned());
        w.mem = Some(map.into_iter().collect());
        w
    }

    #[test]
    fn resolves_relative_paths() {
        assert_eq!(
            resolve_relative("CLAUDE.md", "AGENTS.md").as_deref(),
            Some("AGENTS.md")
        );
        assert_eq!(
            resolve_relative("a/b/CLAUDE.md", "../docs/x.md").as_deref(),
            Some("a/docs/x.md")
        );
        assert_eq!(resolve_relative("CLAUDE.md", "../x.md"), None);
        assert_eq!(resolve_relative("CLAUDE.md", "~/x.md"), None);
    }

    #[test]
    fn claude_md_import_brings_agents_md_into_context() {
        let w = ws(&[("CLAUDE.md", "@AGENTS.md\n"), ("AGENTS.md", "# A\n")]);
        let loaded = w.always_loaded(Target::ClaudeCode);
        let paths: Vec<&str> = loaded.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["CLAUDE.md", "AGENTS.md"]);
        assert_eq!(loaded[1].via.as_deref(), Some("CLAUDE.md"));
        // Codex does not expand imports.
        let codex: Vec<String> = w
            .always_loaded(Target::Codex)
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(codex, vec!["AGENTS.md"]);
    }

    #[test]
    fn imports_past_four_hops_are_dropped() {
        let w = ws(&[
            ("CLAUDE.md", "@a.md"),
            ("a.md", "@b.md"),
            ("b.md", "@c.md"),
            ("c.md", "@d.md"),
            ("d.md", "@e.md"),
            ("e.md", "end"),
        ]);
        let paths: Vec<String> = w
            .always_loaded(Target::ClaudeCode)
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, vec!["CLAUDE.md", "a.md", "b.md", "c.md", "d.md"]);
        let dropped = w.dropped_imports();
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].path, "e.md");
    }

    #[test]
    fn import_cycles_terminate() {
        let w = ws(&[("CLAUDE.md", "@a.md"), ("a.md", "@CLAUDE.md")]);
        assert_eq!(w.always_loaded(Target::ClaudeCode).len(), 2);
    }
}
