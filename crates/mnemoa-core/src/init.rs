//! `mnemoa init`: minimal, pointer-style repo memory for a new repository.
//!
//! Writes an `AGENTS.md` (one-sentence description, the repo's own build and test commands,
//! boundaries, pointers to on-demand docs), a `CLAUDE.md` that imports it, `.mnemoa.toml`,
//! `.mnemoa/checks/` and optionally a GitHub workflow. Existing files are never overwritten.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::repo::RepoFacts;
use crate::workspace::Workspace;

#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    /// Also write `.github/workflows/mnemoa.yml`.
    pub ci: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitPlan {
    /// Files to create with their content.
    #[serde(skip)]
    pub create: BTreeMap<String, String>,
    pub created: Vec<String>,
    /// Files that already exist and are left alone.
    pub skipped: Vec<String>,
}

/// Default `.mnemoa.toml` written by init.
pub const DEFAULT_CONFIG: &str = r#"# mnemoa configuration. Environment variables (MNEMOA_*) override these values.
targets = ["claude-code", "codex", "cursor"]

[scan]
exclude = []
include = []
strict = false

[rules]
max_lines = 200
token_budget = 4000
fossil_commits = 25
disable = []
# Opt-in: require a rationale comment above every rule.
enable = []

[llm]
enabled = false
"#;

const WORKFLOW: &str = r#"name: mnemoa

on:
  pull_request:
  push:
    branches: [main]

jobs:
  mnemoa:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install --locked --git https://github.com/Flotrpy/mnemoa mnemoa-cli
      - run: mnemoa scan . --format github
      - run: mnemoa drift . --format github --base origin/${{ github.base_ref || 'main' }}
        if: github.event_name == 'pull_request'
"#;

fn description(ws: &Workspace) -> Option<String> {
    let from_toml = |path: &str, keys: &[&[&str]]| -> Option<String> {
        let v: toml::Value = toml::from_str(&ws.read_file(path)?).ok()?;
        keys.iter().find_map(|k| {
            let mut cur = &v;
            for p in *k {
                cur = cur.get(*p)?;
            }
            cur.as_str().map(str::to_string)
        })
    };
    from_toml(
        "Cargo.toml",
        &[
            &["package", "description"],
            &["workspace", "package", "description"],
        ],
    )
    .or_else(|| {
        from_toml(
            "pyproject.toml",
            &[
                &["project", "description"],
                &["tool", "poetry", "description"],
            ],
        )
    })
    .or_else(|| {
        let v: serde_json::Value = serde_json::from_str(&ws.read_file("package.json")?).ok()?;
        v.get("description")?.as_str().map(str::to_string)
    })
    .map(|d| d.trim().trim_end_matches('.').to_string())
    .filter(|d| !d.is_empty())
}

/// Commands worth listing: the repo's own task-runner entries for checking and testing.
fn commands(facts: &RepoFacts) -> Vec<String> {
    const WANT: &[&str] = &[
        "check",
        "lint",
        "test",
        "fmt",
        "format",
        "typecheck",
        "build",
    ];
    let mut out = Vec::new();
    if let Some(r) = facts.make.get("") {
        out.extend(
            WANT.iter()
                .filter(|w| r.has(w))
                .map(|w| format!("make {w}")),
        );
    }
    if out.is_empty()
        && let Some(r) = facts.just.get("")
    {
        out.extend(
            WANT.iter()
                .filter(|w| r.has(w))
                .map(|w| format!("just {w}")),
        );
    }
    if out.is_empty()
        && let Some(r) = facts.npm.get("")
    {
        let pm = if facts.lockfiles.contains("pnpm-lock.yaml") {
            "pnpm"
        } else if facts.lockfiles.contains("yarn.lock") {
            "yarn"
        } else if facts.lockfiles.contains("bun.lockb") || facts.lockfiles.contains("bun.lock") {
            "bun"
        } else {
            "npm"
        };
        out.extend(
            WANT.iter()
                .filter(|w| r.has(w))
                .map(|w| format!("{pm} run {w}")),
        );
    }
    out.truncate(4);
    out
}

/// Render the starter AGENTS.md.
pub fn agents_md(ws: &Workspace) -> String {
    let facts = RepoFacts::collect(ws);
    let mut s = String::from("# AGENTS.md\n\n");
    match description(ws) {
        Some(d) => s.push_str(&format!("{d}.\n")),
        None => s.push_str("Describe what this project does in one sentence.\n"),
    }
    let cmds = commands(&facts);
    if !cmds.is_empty() {
        s.push_str("\n## Commands\n\n");
        for c in &cmds {
            s.push_str(&format!("- `{c}`\n"));
        }
    }
    s.push_str("\n## Boundaries\n\n- Never commit secrets or credentials.\n- Ask before changing public APIs or database schemas.\n");
    if ws.discovery.has_dir("docs/agents") {
        s.push_str("\n## Read when needed\n\n- Task guides live in `docs/agents/`; read the one that matches your task before starting it.\n");
    }
    s
}

/// Plan the files init would create.
pub fn plan(ws: &Workspace, opts: &InitOptions) -> InitPlan {
    let mut wanted: Vec<(String, String)> = vec![
        ("AGENTS.md".into(), agents_md(ws)),
        ("CLAUDE.md".into(), "@AGENTS.md\n".into()),
        (crate::config::CONFIG_FILE.into(), DEFAULT_CONFIG.into()),
        (".mnemoa/checks/.gitkeep".into(), String::new()),
    ];
    if opts.ci {
        wanted.push((".github/workflows/mnemoa.yml".into(), WORKFLOW.into()));
    }
    let mut create = BTreeMap::new();
    let mut skipped = Vec::new();
    let claude_exists =
        ws.discovery.has_file("CLAUDE.md") || ws.discovery.has_file(".claude/CLAUDE.md");
    for (path, content) in wanted {
        let exists = ws.discovery.has_file(&path) || (path == "CLAUDE.md" && claude_exists);
        if exists {
            skipped.push(path);
        } else {
            create.insert(path, content);
        }
    }
    InitPlan {
        created: create.keys().cloned().collect(),
        create,
        skipped,
    }
}

/// Create the planned files. Fails rather than overwrite if a file appeared meanwhile.
pub fn apply(ws: &Workspace, plan: &InitPlan) -> std::io::Result<Vec<String>> {
    let mut written = Vec::new();
    for (path, content) in &plan.create {
        let full = ws.root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full)?;
        std::io::Write::write_all(&mut f, content.as_bytes())?;
        written.push(path.clone());
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::scan::{ScanOptions, scan};
    use crate::workspace::tests::ws;

    #[test]
    fn agents_md_uses_description_and_existing_commands() {
        let w = ws(&[
            (
                "Cargo.toml",
                "[package]\nname = \"x\"\ndescription = \"A tiny parser\"\n",
            ),
            ("Makefile", "lint:\n\tx\ntest:\n\ty\nrelease:\n\tz\n"),
        ]);
        let a = agents_md(&w);
        assert!(a.contains("A tiny parser.\n"));
        assert!(a.contains("- `make lint`\n- `make test`\n"));
        assert!(!a.contains("release"));
    }

    #[test]
    fn package_manager_follows_lockfile() {
        let w = ws(&[
            (
                "package.json",
                r#"{"description":"Web app","scripts":{"test":"vitest","dev":"vite"}}"#,
            ),
            ("pnpm-lock.yaml", ""),
        ]);
        let a = agents_md(&w);
        assert!(a.contains("- `pnpm run test`"));
        assert!(a.contains("Web app."));
    }

    #[test]
    fn existing_files_are_skipped() {
        let w = ws(&[("AGENTS.md", "mine"), (".claude/CLAUDE.md", "mine")]);
        let p = plan(&w, &InitOptions { ci: true });
        assert_eq!(p.skipped, vec!["AGENTS.md", "CLAUDE.md"]);
        assert!(
            p.created
                .contains(&".github/workflows/mnemoa.yml".to_string())
        );
        assert!(p.created.contains(&".mnemoa.toml".to_string()));
    }

    #[test]
    fn scaffold_passes_its_own_scan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Makefile"), "test:\n\tcargo test\n").unwrap();
        let w = Workspace::load(dir.path(), &Default::default()).unwrap();
        let p = plan(&w, &InitOptions::default());
        apply(&w, &p).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
        let cfg = Config::load_file(dir.path(), None).unwrap();
        let r = scan(dir.path(), &cfg, &ScanOptions::default()).unwrap();
        assert!(r.findings.is_empty(), "{:?}", r.findings);
        // A second run creates nothing and never overwrites.
        let w = Workspace::load(dir.path(), &Default::default()).unwrap();
        assert!(plan(&w, &InitOptions::default()).created.is_empty());
    }

    #[test]
    fn default_config_parses() {
        let c = Config::from_toml(DEFAULT_CONFIG, std::path::Path::new(".mnemoa.toml")).unwrap();
        assert_eq!(c.rules.max_lines, 200);
    }
}
