//! Find agent context surfaces in a repository and model how each target agent loads them.
//!
//! The loading model assumes the agent is started at the repository root. Sources for each
//! target's behavior are recorded in `docs/research.md`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::frontmatter;

/// A coding agent whose loading behavior mnemoa models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    ClaudeCode,
    Codex,
    Cursor,
    Copilot,
    Kiro,
    Cline,
    Gemini,
}

impl Target {
    /// Every modeled target.
    pub const ALL: [Target; 7] = [
        Target::ClaudeCode,
        Target::Codex,
        Target::Cursor,
        Target::Copilot,
        Target::Kiro,
        Target::Cline,
        Target::Gemini,
    ];

    /// The default targets when none are configured.
    pub const DEFAULT: [Target; 3] = [Target::ClaudeCode, Target::Codex, Target::Cursor];

    /// Kebab-case name used in config and output.
    pub fn as_str(self) -> &'static str {
        match self {
            Target::ClaudeCode => "claude-code",
            Target::Codex => "codex",
            Target::Cursor => "cursor",
            Target::Copilot => "copilot",
            Target::Kiro => "kiro",
            Target::Cline => "cline",
            Target::Gemini => "gemini",
        }
    }

    /// Whether this target strips block-level HTML comments before the model sees them.
    /// Only verified for Claude Code; everything else is treated as "not stripped".
    pub fn strips_html_comments(self) -> bool {
        matches!(self, Target::ClaudeCode)
    }

    /// Parse a comma-separated list of targets.
    pub fn parse_list(s: &str) -> Result<Vec<Target>, String> {
        s.split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(Target::from_str)
            .collect()
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Target {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let norm = s.trim().to_ascii_lowercase().replace('_', "-");
        Target::ALL
            .into_iter()
            .find(|t| t.as_str() == norm || (norm == "claude" && *t == Target::ClaudeCode))
            .ok_or_else(|| {
                format!(
                    "unknown target `{s}` (expected one of: {})",
                    Target::ALL.map(|t| t.as_str()).join(", ")
                )
            })
    }
}

/// What kind of file a surface is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceKind {
    ClaudeMd,
    ClaudeLocalMd,
    ClaudeRule,
    AgentsMd,
    AgentsOverrideMd,
    CursorRule,
    CursorRuleIgnored,
    CursorRulesLegacy,
    CopilotInstructions,
    CopilotScoped,
    GeminiMd,
    KiroSteering,
    KiroSpec,
    ClineRule,
    ClineMemoryBank,
    SpecKitTasks,
    SpecKitDoc,
    SpecKitMemory,
    /// An extra document named in config (`scan.include`).
    Extra,
}

impl SurfaceKind {
    /// Plans and specs: checked for drift, not loaded as instructions.
    pub fn is_plan(self) -> bool {
        matches!(
            self,
            SurfaceKind::KiroSpec
                | SurfaceKind::ClineMemoryBank
                | SurfaceKind::SpecKitTasks
                | SurfaceKind::SpecKitDoc
                | SurfaceKind::SpecKitMemory
        )
    }
}

/// How one target loads one surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum Loading {
    /// In context from the first turn.
    Always,
    /// Loaded only when the trigger happens.
    OnDemand { trigger: String },
    /// The target would normally read this kind of file but skips this one.
    NotLoaded { reason: String },
}

impl Loading {
    pub fn is_always(&self) -> bool {
        matches!(self, Loading::Always)
    }
}

/// One agent context file and how each target loads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub kind: SurfaceKind,
    /// Path relative to the repository root, with `/` separators.
    pub path: String,
    /// Targets that read this kind of file. Targets that ignore the kind are absent.
    pub loading: BTreeMap<Target, Loading>,
}

impl Surface {
    fn new(kind: SurfaceKind, path: &str) -> Self {
        Surface {
            kind,
            path: path.to_string(),
            loading: BTreeMap::new(),
        }
    }

    fn set(&mut self, target: Target, loading: Loading) {
        self.loading.insert(target, loading);
    }

    /// Loading for a target, if the target reads this kind of file.
    pub fn loading_for(&self, target: Target) -> Option<&Loading> {
        self.loading.get(&target)
    }

    /// Directory of the surface relative to the root ("" for the root).
    pub fn dir(&self) -> &str {
        match self.path.rfind('/') {
            Some(i) => &self.path[..i],
            None => "",
        }
    }
}

/// Options for [`discover`].
#[derive(Debug, Clone, Default)]
pub struct DiscoverOptions {
    /// Glob patterns (relative to the root) to skip.
    pub exclude: Vec<String>,
    /// Extra Markdown files to treat as on-demand surfaces.
    pub include: Vec<String>,
}

/// Everything discovery found.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Discovery {
    #[serde(skip)]
    pub root: PathBuf,
    pub surfaces: Vec<Surface>,
    /// Every non-ignored file in the repository, relative, sorted.
    #[serde(skip)]
    pub files: Vec<String>,
    /// Every directory that contains a non-ignored file, relative, sorted.
    #[serde(skip)]
    pub dirs: Vec<String>,
}

impl Discovery {
    /// The surface at a relative path.
    pub fn surface(&self, path: &str) -> Option<&Surface> {
        self.surfaces.iter().find(|s| s.path == path)
    }

    /// Whether a relative file exists in the walked tree.
    pub fn has_file(&self, path: &str) -> bool {
        self.files
            .binary_search_by(|f| f.as_str().cmp(path))
            .is_ok()
    }

    /// Whether a relative directory exists in the walked tree.
    pub fn has_dir(&self, path: &str) -> bool {
        path.is_empty() || self.dirs.binary_search_by(|d| d.as_str().cmp(path)).is_ok()
    }

    /// Whether a relative path is a file or a directory in the walked tree.
    pub fn exists(&self, path: &str) -> bool {
        self.has_file(path) || self.has_dir(path)
    }

    /// Surfaces loaded from the first turn by `target`.
    pub fn always_loaded(&self, target: Target) -> impl Iterator<Item = &Surface> {
        self.surfaces
            .iter()
            .filter(move |s| s.loading_for(target).is_some_and(Loading::is_always))
    }
}

/// Errors from discovery.
#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
    #[error("root `{0}` is not a directory")]
    NotADirectory(PathBuf),
    #[error("invalid glob `{pattern}`: {source}")]
    Glob {
        pattern: String,
        source: globset::Error,
    },
}

fn build_globset(patterns: &[String]) -> Result<GlobSet, DiscoverError> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let glob = Glob::new(p).map_err(|source| DiscoverError::Glob {
            pattern: p.clone(),
            source,
        })?;
        b.add(glob);
    }
    b.build().map_err(|source| DiscoverError::Glob {
        pattern: patterns.join(","),
        source,
    })
}

/// Normalize a path relative to `root` into a `/`-separated string.
pub fn rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

/// Walk `root`, respecting `.gitignore`, and classify every agent context surface.
pub fn discover(root: &Path, opts: &DiscoverOptions) -> Result<Discovery, DiscoverError> {
    if !root.is_dir() {
        return Err(DiscoverError::NotADirectory(root.to_path_buf()));
    }
    let exclude = build_globset(&opts.exclude)?;
    let include = build_globset(&opts.include)?;

    let mut files = Vec::new();
    let mut dirs = std::collections::BTreeSet::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();
    for entry in walker.flatten() {
        let Some(ft) = entry.file_type() else {
            continue;
        };
        if !(ft.is_file() || ft.is_symlink()) {
            continue;
        }
        let Some(rel) = rel_path(root, entry.path()) else {
            continue;
        };
        if rel.is_empty() || exclude.is_match(&rel) {
            continue;
        }
        let mut dir = rel.as_str();
        while let Some(i) = dir.rfind('/') {
            dir = &dir[..i];
            dirs.insert(dir.to_string());
        }
        files.push(rel);
    }
    files.sort();

    let mut d = Discovery {
        root: root.to_path_buf(),
        surfaces: Vec::new(),
        files,
        dirs: dirs.into_iter().collect(),
    };
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap_or_default();
    classify(&mut d, &include, &read);
    Ok(d)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.rfind('/').map(|i| &path[..i]).unwrap_or("")
}

fn on_demand(trigger: impl Into<String>) -> Loading {
    Loading::OnDemand {
        trigger: trigger.into(),
    }
}

fn not_loaded(reason: impl Into<String>) -> Loading {
    Loading::NotLoaded {
        reason: reason.into(),
    }
}

/// Classify discovered files into surfaces. `read` returns file contents for front matter.
pub(crate) fn classify(d: &mut Discovery, include: &GlobSet, read: &dyn Fn(&str) -> String) {
    let files = d.files.clone();
    let has = |p: &str| files.binary_search_by(|f| f.as_str().cmp(p)).is_ok();

    // Root-level Claude files decide whether Claude Code reads AGENTS.md at all.
    let claude_root_files: Vec<&str> = ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"]
        .into_iter()
        .filter(|p| has(p))
        .collect();
    let claude_shadows_agents = !claude_root_files.is_empty();
    let clinerules_mention_memory_bank = files
        .iter()
        .filter(|f| *f == ".clinerules" || f.starts_with(".clinerules/"))
        .any(|f| read(f).contains("memory-bank"));

    let mut surfaces = Vec::new();
    for path in &files {
        let name = file_name(path);
        let dir = parent(path);
        let lower = name.to_ascii_lowercase();
        let is_md = lower.ends_with(".md");

        // --- Claude Code ---
        if name == "CLAUDE.md" || name == "CLAUDE.local.md" {
            let kind = if name == "CLAUDE.md" {
                SurfaceKind::ClaudeMd
            } else {
                SurfaceKind::ClaudeLocalMd
            };
            let mut s = Surface::new(kind, path);
            if dir.is_empty() || (dir == ".claude" && name == "CLAUDE.md") {
                s.set(Target::ClaudeCode, Loading::Always);
            } else {
                s.set(
                    Target::ClaudeCode,
                    on_demand(format!("when Claude touches a file under `{dir}/`")),
                );
            }
            surfaces.push(s);
            continue;
        }
        if path.starts_with(".claude/rules/") {
            if is_md {
                let mut s = Surface::new(SurfaceKind::ClaudeRule, path);
                let fm = frontmatter::split(&read(path));
                let paths = fm.as_ref().map(|f| f.get_list("paths")).unwrap_or_default();
                if paths.is_empty() {
                    s.set(Target::ClaudeCode, Loading::Always);
                } else {
                    s.set(
                        Target::ClaudeCode,
                        on_demand(format!(
                            "when Claude works with files matching {}",
                            paths.join(", ")
                        )),
                    );
                }
                surfaces.push(s);
            }
            continue;
        }

        // --- AGENTS.md family ---
        if name == "AGENTS.md" || name == "AGENTS.override.md" {
            let is_override = name == "AGENTS.override.md";
            let kind = if is_override {
                SurfaceKind::AgentsOverrideMd
            } else {
                SurfaceKind::AgentsMd
            };
            let mut s = Surface::new(kind, path);
            let at_root = dir.is_empty();

            // Codex: one file per directory, override first.
            let override_here = if dir.is_empty() {
                "AGENTS.override.md".to_string()
            } else {
                format!("{dir}/AGENTS.override.md")
            };
            let codex = if !is_override && has(&override_here) {
                not_loaded(format!(
                    "`{override_here}` takes precedence in this directory"
                ))
            } else if at_root {
                Loading::Always
            } else {
                on_demand(format!("when Codex is started in `{dir}/` or below"))
            };
            s.set(Target::Codex, codex);

            if !is_override {
                // Claude Code reads AGENTS.md only when no root Claude file exists.
                let claude_agents = dir.is_empty() || dir == ".claude";
                let claude = if claude_shadows_agents && !claude_agents {
                    not_loaded(format!(
                        "Claude Code skips nested AGENTS.md files when {} exists; add `{dir}/CLAUDE.md` containing `@AGENTS.md`",
                        claude_root_files.join(", ")
                    ))
                } else if claude_shadows_agents {
                    not_loaded(format!(
                        "Claude Code reads {} instead, unless one imports `@AGENTS.md`",
                        claude_root_files.join(", ")
                    ))
                } else if claude_agents {
                    Loading::Always
                } else {
                    on_demand(format!("when Claude reads a file under `{dir}/`"))
                };
                s.set(Target::ClaudeCode, claude);

                let nested = if at_root {
                    Loading::Always
                } else {
                    on_demand(format!("when the agent edits files under `{dir}/`"))
                };
                s.set(Target::Cursor, nested.clone());
                s.set(Target::Copilot, nested.clone());
                if at_root {
                    s.set(Target::Kiro, Loading::Always);
                }
            }
            surfaces.push(s);
            continue;
        }

        // --- Cursor ---
        if path.starts_with(".cursor/rules/") {
            if lower.ends_with(".mdc") {
                let mut s = Surface::new(SurfaceKind::CursorRule, path);
                let fm = frontmatter::split(&read(path));
                let always = fm
                    .as_ref()
                    .and_then(|f| f.get_bool("alwaysApply"))
                    .unwrap_or(false);
                let globs = fm.as_ref().map(|f| f.get_list("globs")).unwrap_or_default();
                let desc = fm
                    .as_ref()
                    .and_then(|f| f.get_str("description"))
                    .is_some_and(|d| !d.trim().is_empty());
                let loading = if always {
                    Loading::Always
                } else if !globs.is_empty() {
                    on_demand(format!(
                        "when files matching {} are in context",
                        globs.join(", ")
                    ))
                } else if desc {
                    on_demand("when the agent judges the description relevant")
                } else {
                    on_demand("only when mentioned with @")
                };
                s.set(Target::Cursor, loading);
                surfaces.push(s);
            } else if is_md {
                let mut s = Surface::new(SurfaceKind::CursorRuleIgnored, path);
                s.set(
                    Target::Cursor,
                    not_loaded(
                        "Cursor reads `.mdc` files in `.cursor/rules/`; plain `.md` is ignored",
                    ),
                );
                surfaces.push(s);
            }
            continue;
        }
        if path == ".cursorrules" {
            let mut s = Surface::new(SurfaceKind::CursorRulesLegacy, path);
            s.set(Target::Cursor, Loading::Always);
            surfaces.push(s);
            continue;
        }

        // --- Copilot ---
        if path == ".github/copilot-instructions.md" {
            let mut s = Surface::new(SurfaceKind::CopilotInstructions, path);
            s.set(Target::Copilot, Loading::Always);
            surfaces.push(s);
            continue;
        }
        if path.starts_with(".github/instructions/") && lower.ends_with(".instructions.md") {
            let mut s = Surface::new(SurfaceKind::CopilotScoped, path);
            let fm = frontmatter::split(&read(path));
            let apply = fm
                .as_ref()
                .map(|f| f.get_list("applyTo"))
                .unwrap_or_default();
            let loading = if apply.iter().any(|g| g == "**" || g == "**/*") {
                Loading::Always
            } else if apply.is_empty() {
                not_loaded("no `applyTo` glob, so Copilot never applies it automatically")
            } else {
                on_demand(format!(
                    "when files matching {} are in context",
                    apply.join(", ")
                ))
            };
            s.set(Target::Copilot, loading);
            surfaces.push(s);
            continue;
        }

        // --- Gemini ---
        if name == "GEMINI.md" {
            let mut s = Surface::new(SurfaceKind::GeminiMd, path);
            s.set(Target::Gemini, Loading::Always);
            surfaces.push(s);
            continue;
        }

        // --- Kiro ---
        if path.starts_with(".kiro/steering/") && is_md {
            let mut s = Surface::new(SurfaceKind::KiroSteering, path);
            let fm = frontmatter::split(&read(path));
            let mode = fm
                .as_ref()
                .and_then(|f| f.get_str("inclusion"))
                .unwrap_or("always")
                .to_string();
            let loading = match mode.as_str() {
                "always" => Loading::Always,
                "fileMatch" => {
                    let pat = fm
                        .as_ref()
                        .map(|f| f.get_list("fileMatchPattern"))
                        .unwrap_or_default();
                    if pat.is_empty() {
                        not_loaded(
                            "`inclusion: fileMatch` without `fileMatchPattern` never matches",
                        )
                    } else {
                        on_demand(format!(
                            "when files matching {} are in context",
                            pat.join(", ")
                        ))
                    }
                }
                "manual" => on_demand("only when referenced with #name or /steering"),
                "auto" => on_demand("when the agent judges the description relevant"),
                other => not_loaded(format!("unknown inclusion mode `{other}`")),
            };
            s.set(Target::Kiro, loading);
            surfaces.push(s);
            continue;
        }
        if path.starts_with(".kiro/specs/") && is_md {
            surfaces.push(Surface::new(SurfaceKind::KiroSpec, path));
            continue;
        }

        // --- Cline ---
        if path == ".clinerules" || (path.starts_with(".clinerules/") && is_md) {
            let mut s = Surface::new(SurfaceKind::ClineRule, path);
            s.set(Target::Cline, Loading::Always);
            surfaces.push(s);
            continue;
        }
        if path.starts_with("memory-bank/") && is_md {
            let mut s = Surface::new(SurfaceKind::ClineMemoryBank, path);
            if clinerules_mention_memory_bank {
                s.set(Target::Cline, Loading::Always);
            } else {
                s.set(
                    Target::Cline,
                    on_demand("when a memory-bank rule tells Cline to read it"),
                );
            }
            surfaces.push(s);
            continue;
        }

        // --- Spec Kit ---
        if path.starts_with(".specify/memory/") && is_md {
            surfaces.push(Surface::new(SurfaceKind::SpecKitMemory, path));
            continue;
        }
        if let Some(rest) = path.strip_prefix("specs/")
            && rest.matches('/').count() == 1
            && is_md
        {
            let kind = if name == "tasks.md" {
                SurfaceKind::SpecKitTasks
            } else {
                SurfaceKind::SpecKitDoc
            };
            surfaces.push(Surface::new(kind, path));
            continue;
        }

        if is_md && include.is_match(path.as_str()) {
            surfaces.push(Surface::new(SurfaceKind::Extra, path));
        }
    }
    d.surfaces = surfaces;
}

/// Rebuild a discovery from a list of files and a reader (used to model edits before they
/// are written).
pub fn rebuild(
    root: &Path,
    mut files: Vec<String>,
    include: &[String],
    read: &dyn Fn(&str) -> String,
) -> Result<Discovery, DiscoverError> {
    files.sort();
    files.dedup();
    let mut dirs = std::collections::BTreeSet::new();
    for f in &files {
        let mut d = f.as_str();
        while let Some(i) = d.rfind('/') {
            d = &d[..i];
            dirs.insert(d.to_string());
        }
    }
    let mut d = Discovery {
        root: root.to_path_buf(),
        surfaces: Vec::new(),
        files,
        dirs: dirs.into_iter().collect(),
    };
    let inc = build_globset(include)?;
    classify(&mut d, &inc, read);
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn run(files: &[(&str, &str)], include: &[&str]) -> Discovery {
        let map: HashMap<String, String> = files
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect();
        let mut names: Vec<String> = map.keys().cloned().collect();
        names.sort();
        let mut d = Discovery {
            files: names,
            ..Default::default()
        };
        let inc =
            build_globset(&include.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        classify(&mut d, &inc, &|p| map.get(p).cloned().unwrap_or_default());
        d
    }

    fn load(d: &Discovery, path: &str, t: Target) -> Option<Loading> {
        d.surface(path)?.loading_for(t).cloned()
    }

    #[test]
    fn agents_md_alone_is_always_loaded_by_claude_and_codex() {
        let d = run(&[("AGENTS.md", "# a")], &[]);
        assert_eq!(
            load(&d, "AGENTS.md", Target::ClaudeCode),
            Some(Loading::Always)
        );
        assert_eq!(load(&d, "AGENTS.md", Target::Codex), Some(Loading::Always));
        assert_eq!(load(&d, "AGENTS.md", Target::Kiro), Some(Loading::Always));
    }

    #[test]
    fn claude_md_shadows_agents_md_for_claude_only() {
        let d = run(&[("AGENTS.md", "# a"), ("CLAUDE.md", "# c")], &[]);
        assert!(matches!(
            load(&d, "AGENTS.md", Target::ClaudeCode),
            Some(Loading::NotLoaded { .. })
        ));
        assert_eq!(load(&d, "AGENTS.md", Target::Codex), Some(Loading::Always));
        assert_eq!(
            load(&d, "CLAUDE.md", Target::ClaudeCode),
            Some(Loading::Always)
        );
    }

    #[test]
    fn claude_local_md_also_shadows_agents_md() {
        let d = run(&[("AGENTS.md", "# a"), ("CLAUDE.local.md", "# c")], &[]);
        assert!(matches!(
            load(&d, "AGENTS.md", Target::ClaudeCode),
            Some(Loading::NotLoaded { .. })
        ));
    }

    #[test]
    fn nested_claude_md_is_on_demand() {
        let d = run(&[("pkg/a/CLAUDE.md", "# c")], &[]);
        assert!(matches!(
            load(&d, "pkg/a/CLAUDE.md", Target::ClaudeCode),
            Some(Loading::OnDemand { .. })
        ));
    }

    #[test]
    fn codex_override_shadows_agents_md() {
        let d = run(&[("AGENTS.md", "a"), ("AGENTS.override.md", "o")], &[]);
        assert!(matches!(
            load(&d, "AGENTS.md", Target::Codex),
            Some(Loading::NotLoaded { .. })
        ));
        assert_eq!(
            load(&d, "AGENTS.override.md", Target::Codex),
            Some(Loading::Always)
        );
    }

    #[test]
    fn claude_rules_with_paths_are_on_demand() {
        let d = run(
            &[
                (
                    ".claude/rules/api.md",
                    "---\npaths:\n  - \"src/api/**\"\n---\n- x",
                ),
                (".claude/rules/style.md", "- y"),
            ],
            &[],
        );
        assert!(matches!(
            load(&d, ".claude/rules/api.md", Target::ClaudeCode),
            Some(Loading::OnDemand { .. })
        ));
        assert_eq!(
            load(&d, ".claude/rules/style.md", Target::ClaudeCode),
            Some(Loading::Always)
        );
    }

    #[test]
    fn cursor_rule_modes() {
        let d = run(
            &[
                (".cursor/rules/a.mdc", "---\nalwaysApply: true\n---\n"),
                (".cursor/rules/b.mdc", "---\nglobs: src/**/*.ts\n---\n"),
                (".cursor/rules/c.md", "plain"),
            ],
            &[],
        );
        assert_eq!(
            load(&d, ".cursor/rules/a.mdc", Target::Cursor),
            Some(Loading::Always)
        );
        assert!(matches!(
            load(&d, ".cursor/rules/b.mdc", Target::Cursor),
            Some(Loading::OnDemand { .. })
        ));
        assert!(matches!(
            load(&d, ".cursor/rules/c.md", Target::Cursor),
            Some(Loading::NotLoaded { .. })
        ));
    }

    #[test]
    fn kiro_inclusion_modes() {
        let d = run(
            &[
                (".kiro/steering/a.md", "no front matter"),
                (".kiro/steering/b.md", "---\ninclusion: manual\n---\n"),
                (".kiro/steering/c.md", "---\ninclusion: fileMatch\n---\n"),
            ],
            &[],
        );
        assert_eq!(
            load(&d, ".kiro/steering/a.md", Target::Kiro),
            Some(Loading::Always)
        );
        assert!(matches!(
            load(&d, ".kiro/steering/b.md", Target::Kiro),
            Some(Loading::OnDemand { .. })
        ));
        assert!(matches!(
            load(&d, ".kiro/steering/c.md", Target::Kiro),
            Some(Loading::NotLoaded { .. })
        ));
    }

    #[test]
    fn plan_surfaces_and_memory_bank() {
        let d = run(
            &[
                ("specs/001-login/tasks.md", "- [ ] T001 x"),
                ("specs/001-login/spec.md", "x"),
                ("memory-bank/activeContext.md", "x"),
                (".clinerules/memory.md", "Read memory-bank/ first"),
                ("docs/guide.md", "x"),
                ("CONTEXT.md", "x"),
            ],
            &["CONTEXT.md"],
        );
        assert_eq!(
            d.surface("specs/001-login/tasks.md").unwrap().kind,
            SurfaceKind::SpecKitTasks
        );
        assert!(
            d.surface("specs/001-login/tasks.md")
                .unwrap()
                .kind
                .is_plan()
        );
        assert_eq!(
            load(&d, "memory-bank/activeContext.md", Target::Cline),
            Some(Loading::Always)
        );
        assert!(d.surface("docs/guide.md").is_none());
        assert_eq!(d.surface("CONTEXT.md").unwrap().kind, SurfaceKind::Extra);
    }

    #[test]
    fn target_parsing() {
        assert_eq!(
            Target::parse_list("claude-code, codex,cursor").unwrap(),
            vec![Target::ClaudeCode, Target::Codex, Target::Cursor]
        );
        assert_eq!("claude".parse::<Target>().unwrap(), Target::ClaudeCode);
        assert!("vim".parse::<Target>().is_err());
    }

    #[test]
    fn discover_walks_real_directory_and_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path();
        std::fs::create_dir_all(tmp.join(".claude/rules")).unwrap();
        std::fs::create_dir_all(tmp.join("build")).unwrap();
        std::fs::write(tmp.join("AGENTS.md"), "# a").unwrap();
        std::fs::write(tmp.join(".claude/rules/x.md"), "- y").unwrap();
        std::fs::write(tmp.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(tmp.join("build/AGENTS.md"), "# ignored").unwrap();
        let d = discover(tmp, &DiscoverOptions::default()).unwrap();
        assert!(d.surface("AGENTS.md").is_some());
        assert!(d.surface(".claude/rules/x.md").is_some());
        assert!(d.surface("build/AGENTS.md").is_none());
        assert!(d.has_dir(".claude/rules"));
    }
}
