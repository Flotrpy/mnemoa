//! Facts about the repository that rules check prose against: task-runner commands,
//! configured enforcers, and declared dependencies.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::workspace::Workspace;

/// Commands a task runner file defines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Runner {
    pub names: BTreeSet<String>,
    /// The file includes or imports others, so the list may be incomplete.
    pub opaque: bool,
}

impl Runner {
    pub fn has(&self, name: &str) -> bool {
        self.names.contains(name)
    }
}

static MAKE_TARGET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9_.\-/$()% ]+?)\s*::?(?:[^=]|$)").unwrap());
static JUST_RECIPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^@?([A-Za-z_][A-Za-z0-9_\-]*)(?:\s+[^:]*)?\s*:(?:[^=]|$)").unwrap()
});
static JUST_ALIAS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^alias\s+([A-Za-z_][A-Za-z0-9_\-]*)\s*:=").unwrap());

/// Parse Makefile targets.
pub fn parse_makefile(text: &str) -> Runner {
    let mut r = Runner::default();
    for line in text.lines() {
        if line.starts_with('\t') || line.trim_start().starts_with('#') {
            continue;
        }
        let t = line.trim_end();
        if t.starts_with("include ") || t.starts_with("-include ") || t.starts_with("sinclude ") {
            r.opaque = true;
            continue;
        }
        if let Some(rest) = t.strip_prefix(".PHONY:") {
            r.names.extend(rest.split_whitespace().map(str::to_string));
            continue;
        }
        if let Some(c) = MAKE_TARGET_RE.captures(t) {
            for name in c[1].split_whitespace() {
                if name.contains(['$', '%', '(']) {
                    r.opaque = true;
                } else if !name.starts_with('.') {
                    r.names.insert(name.to_string());
                }
            }
        }
    }
    r
}

/// Parse justfile recipes and aliases.
pub fn parse_justfile(text: &str) -> Runner {
    let mut r = Runner::default();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) || line.trim_start().starts_with('#') {
            continue;
        }
        let t = line.trim_end();
        if t.starts_with("import ")
            || t.starts_with("mod ")
            || t.starts_with("import?")
            || t.starts_with("mod?")
        {
            r.opaque = true;
            continue;
        }
        if let Some(c) = JUST_ALIAS_RE.captures(t) {
            r.names.insert(c[1].to_string());
            continue;
        }
        if t.starts_with("set ") || t.starts_with("export ") || t.contains(":=") {
            continue;
        }
        if let Some(c) = JUST_RECIPE_RE.captures(t) {
            r.names.insert(c[1].to_string());
        }
    }
    r
}

/// Script names from package.json.
pub fn parse_package_scripts(text: &str) -> Option<Runner> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut r = Runner::default();
    if let Some(scripts) = v.get("scripts").and_then(|s| s.as_object()) {
        r.names.extend(scripts.keys().cloned());
    }
    Some(r)
}

/// Dependency names from package.json.
pub fn parse_package_deps(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        for key in ["dependencies", "devDependencies", "peerDependencies"] {
            if let Some(o) = v.get(key).and_then(|d| d.as_object()) {
                out.extend(o.keys().cloned());
            }
        }
    }
    out
}

fn toml_table<'a>(v: &'a toml::Value, path: &[&str]) -> Option<&'a toml::Table> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_table()
}

/// Cargo aliases from `.cargo/config.toml`.
pub fn parse_cargo_aliases(text: &str) -> Option<Runner> {
    let v: toml::Value = toml::from_str(text).ok()?;
    let t = toml_table(&v, &["alias"])?;
    Some(Runner {
        names: t.keys().cloned().collect(),
        opaque: false,
    })
}

/// Script tables declared in pyproject.toml, by tool.
#[derive(Debug, Clone, Default)]
pub struct PyScripts {
    pub pdm: Option<Runner>,
    pub hatch: Option<Runner>,
    pub poe: Option<Runner>,
    pub taskipy: Option<Runner>,
    pub project: BTreeSet<String>,
    pub deps: BTreeSet<String>,
}

pub fn parse_pyproject(text: &str) -> PyScripts {
    let mut out = PyScripts::default();
    let Ok(v) = toml::from_str::<toml::Value>(text) else {
        return out;
    };
    let names = |t: &toml::Table| Runner {
        names: t.keys().cloned().collect(),
        opaque: false,
    };
    out.pdm = toml_table(&v, &["tool", "pdm", "scripts"]).map(names);
    out.poe = toml_table(&v, &["tool", "poe", "tasks"]).map(names);
    out.taskipy = toml_table(&v, &["tool", "taskipy", "tasks"]).map(names);
    if let Some(envs) = toml_table(&v, &["tool", "hatch", "envs"]) {
        let mut r = Runner::default();
        for (env, cfg) in envs {
            if let Some(s) = cfg.get("scripts").and_then(|s| s.as_table()) {
                for k in s.keys() {
                    r.names.insert(k.clone());
                    r.names.insert(format!("{env}:{k}"));
                }
            }
        }
        out.hatch = Some(r);
    }
    for path in [
        &["project", "scripts"][..],
        &["tool", "poetry", "scripts"][..],
    ] {
        if let Some(t) = toml_table(&v, path) {
            out.project.extend(t.keys().cloned());
        }
    }
    let dep_name = |s: &str| {
        s.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.'))
            .next()
            .unwrap_or("")
            .to_lowercase()
    };
    if let Some(deps) = v
        .get("project")
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_array())
    {
        out.deps
            .extend(deps.iter().filter_map(|d| d.as_str()).map(dep_name));
    }
    if let Some(groups) = v
        .get("project")
        .and_then(|p| p.get("optional-dependencies"))
        .and_then(|d| d.as_table())
    {
        for list in groups.values().filter_map(|l| l.as_array()) {
            out.deps
                .extend(list.iter().filter_map(|d| d.as_str()).map(dep_name));
        }
    }
    if let Some(t) = toml_table(&v, &["tool", "poetry", "dependencies"]) {
        out.deps.extend(t.keys().map(|k| k.to_lowercase()));
    }
    out
}

/// Task names from a go-task `Taskfile.yml`.
pub fn parse_taskfile(text: &str) -> Option<Runner> {
    let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(text).ok()?;
    let tasks = v.get("tasks")?.as_mapping()?;
    let mut r = Runner::default();
    r.names
        .extend(tasks.keys().filter_map(|k| k.as_str().map(str::to_string)));
    r.opaque = v.get("includes").is_some();
    Some(r)
}

/// Dependency names from Cargo.toml (all dependency tables, including workspace ones).
pub fn parse_cargo_deps(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(v) = toml::from_str::<toml::Value>(text) else {
        return out;
    };
    let mut add = |t: Option<&toml::Table>| {
        if let Some(t) = t {
            out.extend(t.keys().cloned());
        }
    };
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        add(toml_table(&v, &[key]));
        add(toml_table(&v, &["workspace", key]));
    }
    out
}

/// Collected facts, computed once per scan.
#[derive(Debug, Clone, Default)]
pub struct RepoFacts {
    /// Makefile targets by directory ("" for the root).
    pub make: BTreeMap<String, Runner>,
    pub just: BTreeMap<String, Runner>,
    /// package.json scripts by directory.
    pub npm: BTreeMap<String, Runner>,
    pub cargo_aliases: Option<Runner>,
    pub py: PyScripts,
    pub taskfile: Option<Runner>,
    /// Declared dependency names across manifests, lowercased.
    pub deps: BTreeSet<String>,
    /// Lockfiles present at the root.
    pub lockfiles: BTreeSet<String>,
}

impl RepoFacts {
    pub fn collect(ws: &Workspace) -> RepoFacts {
        let mut f = RepoFacts::default();
        for path in &ws.discovery.files {
            let (dir, name) = match path.rfind('/') {
                Some(i) => (&path[..i], &path[i + 1..]),
                None => ("", path.as_str()),
            };
            match name {
                "Makefile" | "makefile" | "GNUmakefile" => {
                    if let Some(t) = ws.read_file(path) {
                        f.make.insert(dir.to_string(), parse_makefile(&t));
                    }
                }
                "justfile" | "Justfile" | ".justfile" => {
                    if let Some(t) = ws.read_file(path) {
                        f.just.insert(dir.to_string(), parse_justfile(&t));
                    }
                }
                "package.json" => {
                    if let Some(t) = ws.read_file(path) {
                        if let Some(r) = parse_package_scripts(&t) {
                            f.npm.insert(dir.to_string(), r);
                        }
                        f.deps
                            .extend(parse_package_deps(&t).into_iter().map(|d| d.to_lowercase()));
                    }
                }
                "Cargo.toml" => {
                    if let Some(t) = ws.read_file(path) {
                        f.deps
                            .extend(parse_cargo_deps(&t).into_iter().map(|d| d.to_lowercase()));
                    }
                }
                "pyproject.toml" if dir.is_empty() => {
                    if let Some(t) = ws.read_file(path) {
                        f.py = parse_pyproject(&t);
                        f.deps.extend(f.py.deps.iter().cloned());
                    }
                }
                "Taskfile.yml" | "Taskfile.yaml" if dir.is_empty() => {
                    f.taskfile = ws.read_file(path).and_then(|t| parse_taskfile(&t));
                }
                "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "bun.lockb" | "bun.lock"
                | "uv.lock" | "poetry.lock" | "pdm.lock" | "Pipfile.lock"
                    if dir.is_empty() =>
                {
                    f.lockfiles.insert(name.to_string());
                }
                _ => {}
            }
        }
        for p in [".cargo/config.toml", ".cargo/config"] {
            if let Some(t) = ws.read_file(p)
                && let Some(r) = parse_cargo_aliases(&t)
            {
                f.cargo_aliases = Some(r);
                break;
            }
        }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makefile_targets() {
        let r = parse_makefile(
            ".PHONY: fmt lint\nfmt:\n\tcargo fmt\nlint test: fmt\n\tx\nVAR := 1\nX = 2\n.SUFFIXES:\n",
        );
        assert!(r.has("fmt") && r.has("lint") && r.has("test"));
        assert!(!r.has("VAR") && !r.has("X") && !r.has(".SUFFIXES"));
        assert!(!r.opaque);
        assert!(parse_makefile("include common.mk\n").opaque);
        assert!(parse_makefile("%.o: %.c\n\tcc\n").opaque);
    }

    #[test]
    fn justfile_recipes() {
        let r = parse_justfile(
            "set shell := [\"bash\"]\nversion := \"1\"\nalias t := test\n# c\ntest *args:\n    cargo test\n@lint:\n    x\nbuild target='x': lint\n    y\n",
        );
        assert!(r.has("test") && r.has("t") && r.has("lint") && r.has("build"));
        assert!(!r.has("version") && !r.has("set"));
        assert!(parse_justfile("import 'x.just'\n").opaque);
    }

    #[test]
    fn package_and_cargo_and_pyproject() {
        let p = parse_package_scripts(
            r#"{"scripts":{"build":"x","test:unit":"y"},"devDependencies":{"vitest":"1"}}"#,
        )
        .unwrap();
        assert!(p.has("build") && p.has("test:unit"));
        assert!(parse_package_deps(r#"{"devDependencies":{"vitest":"1"}}"#).contains("vitest"));
        assert!(parse_package_scripts("not json").is_none());
        let c = parse_cargo_aliases("[alias]\nxtask = \"run -p xtask --\"\n").unwrap();
        assert!(c.has("xtask"));
        assert!(parse_cargo_aliases("[build]\njobs = 2\n").is_none());
        let py = parse_pyproject(
            "[project]\ndependencies = [\"httpx>=0.27\"]\n[project.scripts]\nmycli = \"a:b\"\n[tool.pdm.scripts]\nlint = \"ruff check\"\n[tool.hatch.envs.default.scripts]\ncov = \"x\"\n",
        );
        assert!(py.pdm.unwrap().has("lint"));
        let hatch = py.hatch.unwrap();
        assert!(hatch.has("cov") && hatch.has("default:cov"));
        assert!(py.project.contains("mycli"));
        assert!(py.deps.contains("httpx"));
        let deps = parse_cargo_deps(
            "[dependencies]\nserde = \"1\"\n[workspace.dependencies]\ntokio = \"1\"\n",
        );
        assert!(deps.contains("serde") && deps.contains("tokio"));
        let tf = parse_taskfile("version: '3'\ntasks:\n  build:\n    cmds: [x]\n").unwrap();
        assert!(tf.has("build"));
    }

    #[test]
    fn collect_from_workspace() {
        let w = crate::workspace::tests::ws(&[
            ("Makefile", "lint:\n\tx\n"),
            ("web/package.json", r#"{"scripts":{"dev":"vite"}}"#),
            ("pnpm-lock.yaml", ""),
            ("AGENTS.md", "x"),
        ]);
        let f = RepoFacts::collect(&w);
        assert!(f.make[""].has("lint"));
        assert!(f.npm["web"].has("dev"));
        assert!(f.lockfiles.contains("pnpm-lock.yaml"));
    }
}
