//! Git history behind a trait, with a `gix` backend and an in-memory fake for tests.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileChange {
    pub kind: ChangeKind,
    /// Path after the change, relative to the scanned root.
    pub path: String,
    /// Path before a rename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

impl FileChange {
    pub fn new(kind: ChangeKind, path: &str) -> Self {
        FileChange {
            kind,
            path: path.to_string(),
            old_path: None,
        }
    }

    pub fn renamed(old: &str, new: &str) -> Self {
        FileChange {
            kind: ChangeKind::Renamed,
            path: new.to_string(),
            old_path: Some(old.to_string()),
        }
    }

    /// Paths this change touches (both sides of a rename).
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.path.as_str()).chain(self.old_path.as_deref())
    }
}

/// A commit and the files it changed relative to its first parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitInfo {
    pub id: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
    pub changes: Vec<FileChange>,
}

impl CommitInfo {
    pub fn touches(&self, path: &str) -> bool {
        self.changes.iter().any(|c| c.paths().any(|p| p == path))
    }

    /// Whether any changed file is not Markdown or plain text.
    pub fn touches_code(&self) -> bool {
        self.changes.iter().any(|c| c.paths().any(is_code_path))
    }
}

/// Whether a path counts as code for freshness purposes (not docs).
pub fn is_code_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    !(lower.ends_with(".md")
        || lower.ends_with(".mdc")
        || lower.ends_with(".txt")
        || lower.ends_with(".rst")
        || lower.ends_with(".adoc")
        || lower.starts_with("docs/")
        || lower.contains("/docs/"))
}

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("git: {0}")]
    Git(String),
    #[error("unknown revision `{0}`")]
    UnknownRevision(String),
}

/// Read access to commit history.
pub trait History: Send + Sync {
    /// Commits reachable from HEAD, newest first.
    fn commits(&self) -> &[CommitInfo];

    /// Whether the clone is shallow, so history is incomplete.
    fn is_shallow(&self) -> bool;

    /// Files changed between `base` and HEAD (`git diff base...HEAD` semantics are not needed;
    /// this compares the two trees directly).
    fn diff_from(&self, base: &str) -> Result<Vec<FileChange>, HistoryError>;

    /// Contents of `path` (relative to the scanned root) at revision `rev`, if it existed there.
    fn file_at(&self, rev: &str, path: &str) -> Result<Option<String>, HistoryError>;

    /// Indices (into [`History::commits`]) of commits that touch `path`, newest first.
    fn touching(&self, path: &str) -> Vec<usize> {
        self.commits()
            .iter()
            .enumerate()
            .filter(|(_, c)| c.touches(path))
            .map(|(i, _)| i)
            .collect()
    }

    /// Number of commits newer than the commit at `index` that change code.
    fn code_commits_since(&self, index: usize) -> usize {
        self.commits()[..index.min(self.commits().len())]
            .iter()
            .filter(|c| c.touches_code())
            .count()
    }

    /// Number of commits newer than `index` that touch any of `paths` (or anything under them).
    fn commits_touching_since(&self, index: usize, paths: &[String]) -> usize {
        self.commits()[..index.min(self.commits().len())]
            .iter()
            .filter(|c| {
                c.changes.iter().any(|ch| {
                    ch.paths().any(|p| {
                        paths.iter().any(|q| {
                            p == q || p.starts_with(&format!("{}/", q.trim_end_matches('/')))
                        })
                    })
                })
            })
            .count()
    }
}

/// History held in memory. Used by tests and benches.
#[derive(Debug, Clone, Default)]
pub struct MemoryHistory {
    pub commits: Vec<CommitInfo>,
    pub shallow: bool,
    pub diffs: BTreeMap<String, Vec<FileChange>>,
    /// File contents by (revision, path).
    pub files: BTreeMap<(String, String), String>,
}

impl MemoryHistory {
    /// Build from (id, changed paths) pairs given oldest first. Every path counts as modified,
    /// except that the first time a path appears it counts as added.
    pub fn linear(commits: &[(&str, &[&str])]) -> Self {
        let mut seen = std::collections::BTreeSet::new();
        let mut out: Vec<CommitInfo> = commits
            .iter()
            .enumerate()
            .map(|(i, (id, paths))| CommitInfo {
                id: id.to_string(),
                time: 1_700_000_000 + i as i64 * 60,
                changes: paths
                    .iter()
                    .map(|p| {
                        let kind = if seen.insert(p.to_string()) {
                            ChangeKind::Added
                        } else {
                            ChangeKind::Modified
                        };
                        FileChange::new(kind, p)
                    })
                    .collect(),
            })
            .collect();
        out.reverse();
        MemoryHistory {
            commits: out,
            shallow: false,
            diffs: BTreeMap::new(),
            files: BTreeMap::new(),
        }
    }
}

impl History for MemoryHistory {
    fn commits(&self) -> &[CommitInfo] {
        &self.commits
    }
    fn is_shallow(&self) -> bool {
        self.shallow
    }
    fn diff_from(&self, base: &str) -> Result<Vec<FileChange>, HistoryError> {
        self.diffs
            .get(base)
            .cloned()
            .ok_or_else(|| HistoryError::UnknownRevision(base.to_string()))
    }
    fn file_at(&self, rev: &str, path: &str) -> Result<Option<String>, HistoryError> {
        Ok(self
            .files
            .get(&(rev.to_string(), path.to_string()))
            .cloned())
    }
}

/// History read from a git repository with `gix`.
pub struct GixHistory {
    /// Directory used to reopen the repository (the scanned root).
    root: std::path::PathBuf,
    /// Path of the scanned root inside the repository ("" when they are the same), `/`-separated.
    prefix: String,
    commits: Vec<CommitInfo>,
    shallow: bool,
}

impl std::fmt::Debug for GixHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GixHistory")
            .field("prefix", &self.prefix)
            .field("commits", &self.commits.len())
            .field("shallow", &self.shallow)
            .finish()
    }
}

fn git_err(e: impl std::fmt::Display) -> HistoryError {
    HistoryError::Git(e.to_string())
}

impl GixHistory {
    /// Open the repository containing `root` and read up to `max_commits` commits from HEAD.
    /// Returns `Ok(None)` when `root` is not inside a git work tree or HEAD has no commits.
    pub fn open(root: &Path, max_commits: usize) -> Result<Option<GixHistory>, HistoryError> {
        let Ok(repo) = gix::discover(root) else {
            return Ok(None);
        };
        let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
            return Ok(None);
        };
        let canon_root = root.canonicalize().map_err(git_err)?;
        let canon_work = workdir.canonicalize().map_err(git_err)?;
        let prefix = canon_root
            .strip_prefix(&canon_work)
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default();
        let Ok(head) = repo.head_id() else {
            return Ok(None);
        };
        let shallow = repo.is_shallow();
        let mut commits = Vec::new();
        let walk = repo
            .rev_walk([head.detach()])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .first_parent_only()
            .all()
            .map_err(git_err)?;
        for info in walk.take(max_commits) {
            let info = info.map_err(git_err)?;
            let commit = info.object().map_err(git_err)?;
            let time = commit.time().map_err(git_err)?.seconds;
            let tree = commit.tree().map_err(git_err)?;
            let parent_tree = match info.parent_ids().next() {
                Some(pid) => Some(
                    pid.object()
                        .map_err(git_err)?
                        .into_commit()
                        .tree()
                        .map_err(git_err)?,
                ),
                None => None,
            };
            let changes = diff_trees(&repo, parent_tree.as_ref(), &tree, None, &prefix)?;
            commits.push(CommitInfo {
                id: info.id.to_string(),
                time,
                changes,
            });
        }
        Ok(Some(GixHistory {
            root: root.to_path_buf(),
            prefix,
            commits,
            shallow,
        }))
    }
}

fn strip_prefix(prefix: &str, path: &str) -> Option<String> {
    if prefix.is_empty() {
        return Some(path.to_string());
    }
    path.strip_prefix(prefix)?
        .strip_prefix('/')
        .map(str::to_string)
}

fn diff_trees(
    repo: &gix::Repository,
    old: Option<&gix::Tree<'_>>,
    new: &gix::Tree<'_>,
    rewrites: Option<gix::diff::Rewrites>,
    prefix: &str,
) -> Result<Vec<FileChange>, HistoryError> {
    use gix::object::tree::diff::ChangeDetached as C;
    let opts = gix::diff::Options::default().with_rewrites(rewrites);
    let changes = repo
        .diff_tree_to_tree(old, Some(new), Some(opts))
        .map_err(git_err)?;
    let mut out = Vec::new();
    for ch in changes {
        let (kind, path, old_path, is_tree) = match &ch {
            C::Addition {
                location,
                entry_mode,
                ..
            } => (
                ChangeKind::Added,
                location.to_string(),
                None,
                entry_mode.is_tree(),
            ),
            C::Deletion {
                location,
                entry_mode,
                ..
            } => (
                ChangeKind::Deleted,
                location.to_string(),
                None,
                entry_mode.is_tree(),
            ),
            C::Modification {
                location,
                entry_mode,
                ..
            } => (
                ChangeKind::Modified,
                location.to_string(),
                None,
                entry_mode.is_tree(),
            ),
            C::Rewrite {
                source_location,
                location,
                entry_mode,
                copy,
                ..
            } => {
                if *copy {
                    (
                        ChangeKind::Added,
                        location.to_string(),
                        None,
                        entry_mode.is_tree(),
                    )
                } else {
                    (
                        ChangeKind::Renamed,
                        location.to_string(),
                        Some(source_location.to_string()),
                        entry_mode.is_tree(),
                    )
                }
            }
        };
        if is_tree {
            continue;
        }
        let Some(path) = strip_prefix(prefix, &path) else {
            continue;
        };
        let old_path = old_path.and_then(|p| strip_prefix(prefix, &p));
        out.push(FileChange {
            kind,
            path,
            old_path,
        });
    }
    Ok(out)
}

impl History for GixHistory {
    fn commits(&self) -> &[CommitInfo] {
        &self.commits
    }

    fn is_shallow(&self) -> bool {
        self.shallow
    }

    fn diff_from(&self, base: &str) -> Result<Vec<FileChange>, HistoryError> {
        let repo = gix::discover(&self.root).map_err(git_err)?;
        let base_id = repo
            .rev_parse_single(base)
            .map_err(|_| HistoryError::UnknownRevision(base.to_string()))?;
        let base_tree = base_id
            .object()
            .map_err(git_err)?
            .peel_to_commit()
            .map_err(git_err)?
            .tree()
            .map_err(git_err)?;
        let head_tree = repo
            .head_commit()
            .map_err(git_err)?
            .tree()
            .map_err(git_err)?;
        diff_trees(
            &repo,
            Some(&base_tree),
            &head_tree,
            Some(Default::default()),
            &self.prefix,
        )
    }

    fn file_at(&self, rev: &str, path: &str) -> Result<Option<String>, HistoryError> {
        let repo = gix::discover(&self.root).map_err(git_err)?;
        let id = repo
            .rev_parse_single(rev)
            .map_err(|_| HistoryError::UnknownRevision(rev.to_string()))?;
        let tree = id
            .object()
            .map_err(git_err)?
            .peel_to_commit()
            .map_err(git_err)?
            .tree()
            .map_err(git_err)?;
        let full = if self.prefix.is_empty() {
            path.to_string()
        } else {
            format!("{}/{path}", self.prefix)
        };
        let Some(entry) = tree.lookup_entry_by_path(&full).map_err(git_err)? else {
            return Ok(None);
        };
        let obj = entry.object().map_err(git_err)?;
        Ok(Some(String::from_utf8_lossy(&obj.data).into_owned()))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::process::Command;

    /// Create a git repo in a temp dir with the given commits (oldest first): each commit
    /// writes the listed files (path, content). A `None` content deletes the file.
    pub(crate) fn git_repo(commits: &[&[(&str, Option<&str>)]]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let st = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap();
            assert!(
                st.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&st.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "commit.gpgsign", "false"]);
        for (i, files) in commits.iter().enumerate() {
            for (path, content) in files.iter() {
                let p = dir.path().join(path);
                match content {
                    Some(c) => {
                        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                        std::fs::write(&p, c).unwrap();
                    }
                    None => {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
            git(&["add", "-A"]);
            let date = format!("2024-01-01T00:{:02}:00Z", i % 60);
            let st = Command::new("git")
                .args(["commit", "-q", "--allow-empty", "-m", &format!("c{i}")])
                .current_dir(dir.path())
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date)
                .output()
                .unwrap();
            assert!(st.status.success());
            if i == 0 {
                git(&["tag", "base"]);
            }
        }
        dir
    }

    #[test]
    fn memory_history_helpers() {
        let h = MemoryHistory::linear(&[
            ("a", &["AGENTS.md", "src/a.rs"]),
            ("b", &["src/a.rs"]),
            ("c", &["README.md"]),
            ("d", &["src/b.rs"]),
        ]);
        assert_eq!(h.commits()[0].id, "d");
        let touching = h.touching("AGENTS.md");
        assert_eq!(touching, vec![3]);
        assert_eq!(h.code_commits_since(3), 2);
        assert_eq!(h.commits_touching_since(3, &["src".to_string()]), 2);
        assert!(h.diff_from("x").is_err());
    }

    #[test]
    fn gix_reads_real_history_and_diffs() {
        let body: String = (0..40).map(|i| format!("fn f{i}() {{ {i}; }}\n")).collect();
        let changed = format!("{body}fn extra() {{}}\n");
        let dir = git_repo(&[
            &[("AGENTS.md", Some("a")), ("src/a.rs", Some(body.as_str()))],
            &[("src/a.rs", Some(changed.as_str()))],
            &[("src/a.rs", None), ("src/b.rs", Some(changed.as_str()))],
        ]);
        let h = GixHistory::open(dir.path(), 100).unwrap().unwrap();
        assert_eq!(h.commits().len(), 3);
        assert_eq!(h.touching("AGENTS.md"), vec![2]);
        assert_eq!(h.code_commits_since(2), 2);
        let diff = h.diff_from("base").unwrap();
        assert!(
            diff.iter().any(|c| c.kind == ChangeKind::Renamed
                && c.old_path.as_deref() == Some("src/a.rs")
                && c.path == "src/b.rs"),
            "{diff:?}"
        );
        assert!(matches!(
            h.diff_from("nope"),
            Err(HistoryError::UnknownRevision(_))
        ));
        assert_eq!(
            h.file_at("base", "AGENTS.md").unwrap().as_deref(),
            Some("a")
        );
        assert!(h.file_at("base", "src/b.rs").unwrap().is_none());
        assert!(h.file_at("HEAD", "src/b.rs").unwrap().is_some());
    }

    #[test]
    fn gix_strips_subdirectory_prefix() {
        let dir = git_repo(&[&[("pkg/AGENTS.md", Some("a")), ("other.rs", Some("x"))]]);
        let h = GixHistory::open(&dir.path().join("pkg"), 10)
            .unwrap()
            .unwrap();
        let paths: Vec<&str> = h.commits()[0]
            .changes
            .iter()
            .map(|c| c.path.as_str())
            .collect();
        assert_eq!(paths, vec!["AGENTS.md"]);
    }

    #[test]
    fn non_repo_is_none() {
        let dir = tempfile::tempdir().unwrap();
        // A temp dir can sit inside another repo; only assert when discovery fails.
        if gix::discover(dir.path()).is_err() {
            assert!(GixHistory::open(dir.path(), 10).unwrap().is_none());
        }
    }
}
