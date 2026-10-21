//! Detect which formatters, linters and checkers a repository configures.
//!
//! Detection under-matches on purpose: a tool counts only with a config file, or when the
//! repo's automation (CI workflows, task runners, pre-commit) runs it.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::workspace::Workspace;

/// A configured enforcer and the evidence for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Enforcer {
    pub id: String,
    /// Human name, for example "Prettier".
    pub name: String,
    /// Files that show the tool is configured or run.
    pub evidence: Vec<String>,
    /// Concatenated configuration text, used to check that opt-in lints are enabled.
    #[serde(skip)]
    pub config_text: String,
}

/// Human name for an enforcer id.
pub fn display_name(id: &str) -> &str {
    match id {
        "editorconfig" => "EditorConfig",
        "prettier" => "Prettier",
        "eslint" => "ESLint",
        "biome" => "Biome",
        "rustfmt" => "rustfmt",
        "clippy" => "Clippy",
        "rustc-deny-warnings" => "rustc with -D warnings",
        "ruff" => "Ruff",
        "black" => "Black",
        "isort" => "isort",
        "mypy" => "mypy",
        "gofmt" => "gofmt",
        "go" => "the Go compiler",
        "golangci-lint" => "golangci-lint",
        "clang-format" => "clang-format",
        "rubocop" => "RuboCop",
        "stylelint" => "Stylelint",
        "markdownlint" => "markdownlint",
        "shellcheck" => "ShellCheck",
        "tsc" => "the TypeScript compiler",
        "pre-commit-whitespace" => "pre-commit whitespace hooks",
        other => other,
    }
}

/// Files whose text shows what automation runs.
fn is_automation(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    (path.starts_with(".github/workflows/") && (name.ends_with(".yml") || name.ends_with(".yaml")))
        || matches!(
            name,
            "Makefile"
                | "makefile"
                | "GNUmakefile"
                | "justfile"
                | "Justfile"
                | ".justfile"
                | ".gitlab-ci.yml"
                | ".pre-commit-config.yaml"
                | "lefthook.yml"
                | "lefthook.yaml"
                | "Taskfile.yml"
                | "Taskfile.yaml"
                | "azure-pipelines.yml"
                | "package.json"
                | "noxfile.py"
                | "tox.ini"
        )
        || path.starts_with(".husky/")
        || path == ".circleci/config.yml"
}

/// Detect configured enforcers.
pub fn detect(ws: &Workspace) -> BTreeMap<String, Enforcer> {
    let files = &ws.discovery.files;
    let mut automation = String::new();
    let mut automation_files = Vec::new();
    for f in files.iter().filter(|f| is_automation(f)) {
        if let Some(t) = ws.read_file(f) {
            automation.push_str(&t);
            automation.push('\n');
            automation_files.push(f.clone());
        }
    }
    let auto_lower = automation.to_lowercase();
    let read = |p: &str| ws.read_file(p).unwrap_or_default();
    let root_files: Vec<&String> = files.iter().filter(|f| !f.contains('/')).collect();
    let root_match = |pred: &dyn Fn(&str) -> bool| -> Vec<String> {
        root_files
            .iter()
            .filter(|f| pred(f))
            .map(|f| f.to_string())
            .collect()
    };
    let pyproject = read("pyproject.toml");
    let package = read("package.json");
    let cargo = read("Cargo.toml");
    let has_file = |p: &str| ws.discovery.has_file(p);

    let mut out: BTreeMap<String, Enforcer> = BTreeMap::new();
    let mut add = |id: &str, evidence: Vec<String>, config_text: String| {
        if evidence.is_empty() {
            return;
        }
        let e = out.entry(id.to_string()).or_insert_with(|| Enforcer {
            id: id.to_string(),
            name: display_name(id).to_string(),
            evidence: Vec::new(),
            config_text: String::new(),
        });
        for ev in evidence {
            if !e.evidence.contains(&ev) {
                e.evidence.push(ev);
            }
        }
        e.config_text.push_str(&config_text);
    };
    let automation_evidence = |needles: &[&str]| -> Vec<String> {
        if needles.iter().any(|n| auto_lower.contains(n)) {
            automation_files
                .iter()
                .filter(|f| {
                    let t = ws.read_file(f).unwrap_or_default().to_lowercase();
                    needles.iter().any(|n| t.contains(n))
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    };
    let concat = |paths: &[String]| paths.iter().map(|p| read(p)).collect::<Vec<_>>().join("\n");

    // EditorConfig
    if has_file(".editorconfig") {
        add(
            "editorconfig",
            vec![".editorconfig".into()],
            read(".editorconfig"),
        );
    }
    // Prettier
    let mut ev = root_match(&|f| f.starts_with(".prettierrc") || f.starts_with("prettier.config."));
    let package_json: serde_json::Value = serde_json::from_str(&package).unwrap_or_default();
    if package_json.get("prettier").is_some() {
        ev.push("package.json".into());
    }
    ev.extend(automation_evidence(&[
        "prettier --check",
        "prettier --write",
        "prettier -c",
        "mirrors-prettier",
    ]));
    let text = concat(&ev);
    add("prettier", ev, text);
    // ESLint
    let mut ev = root_match(&|f| f.starts_with("eslint.config.") || f.starts_with(".eslintrc"));
    if package_json.get("eslintConfig").is_some() {
        ev.push("package.json".into());
    }
    let text = concat(&ev);
    add("eslint", ev, text);
    // Biome
    let ev = root_match(&|f| f == "biome.json" || f == "biome.jsonc");
    let text = concat(&ev);
    add("biome", ev, text);
    // TypeScript compiler
    if has_file("tsconfig.json") {
        add("tsc", vec!["tsconfig.json".into()], read("tsconfig.json"));
    }
    // rustfmt
    let mut ev = root_match(&|f| f == "rustfmt.toml" || f == ".rustfmt.toml");
    ev.extend(automation_evidence(&["cargo fmt", "rustfmt"]));
    let text = concat(&ev);
    add("rustfmt", ev, text);
    // Clippy
    let mut ev = root_match(&|f| f == "clippy.toml" || f == ".clippy.toml");
    if cargo.contains("[lints.clippy]") || cargo.contains("[workspace.lints.clippy]") {
        ev.push("Cargo.toml".into());
    }
    ev.extend(automation_evidence(&["cargo clippy"]));
    let mut text = concat(&ev);
    text.push_str(&automation);
    add("clippy", ev, text);
    // rustc warnings as errors
    if has_file("Cargo.toml") {
        let mut ev = automation_evidence(&["-d warnings", "-dwarnings", "deny(warnings)"]);
        if cargo.contains("warnings = \"deny\"") || cargo.contains("warnings = { level = \"deny\"")
        {
            ev.push("Cargo.toml".into());
        }
        if read(".cargo/config.toml").contains("-D warnings") {
            ev.push(".cargo/config.toml".into());
        }
        add("rustc-deny-warnings", ev, String::new());
    }
    // Ruff
    let mut ev = root_match(&|f| f == "ruff.toml" || f == ".ruff.toml");
    if pyproject.contains("[tool.ruff") {
        ev.push("pyproject.toml".into());
    }
    ev.extend(automation_evidence(&[
        "ruff check",
        "ruff format",
        "ruff-pre-commit",
    ]));
    let text = concat(&ev);
    add("ruff", ev, text);
    // Black
    let mut ev = Vec::new();
    if pyproject.contains("[tool.black]") {
        ev.push("pyproject.toml".into());
    }
    ev.extend(automation_evidence(&[
        "psf/black",
        "black --check",
        "black .",
    ]));
    add("black", ev, String::new());
    // isort
    let mut ev = root_match(&|f| f == ".isort.cfg");
    if pyproject.contains("[tool.isort]") {
        ev.push("pyproject.toml".into());
    }
    add("isort", ev, String::new());
    // mypy
    let mut ev = root_match(&|f| f == "mypy.ini" || f == ".mypy.ini");
    if pyproject.contains("[tool.mypy]") {
        ev.push("pyproject.toml".into());
    }
    let mut text = concat(&ev);
    if pyproject.contains("[tool.mypy]") {
        text.push_str(&pyproject);
    }
    add("mypy", ev, text);
    // Go
    if has_file("go.mod") {
        add("go", vec!["go.mod".into()], String::new());
        let ev = automation_evidence(&["gofmt", "goimports", "go fmt"]);
        add("gofmt", ev, String::new());
    }
    let ev = root_match(&|f| f.starts_with(".golangci."));
    let text = concat(&ev);
    add("golangci-lint", ev, text);
    // Others with a config file.
    for (id, names) in [
        ("clang-format", &[".clang-format", "_clang-format"][..]),
        ("rubocop", &[".rubocop.yml"][..]),
        ("shellcheck", &[".shellcheckrc"][..]),
    ] {
        let ev = root_match(&|f| names.contains(&f));
        add(id, ev, String::new());
    }
    let ev = root_match(&|f| f.starts_with(".stylelintrc") || f.starts_with("stylelint.config."));
    add("stylelint", ev, String::new());
    let ev = root_match(&|f| f.starts_with(".markdownlint"));
    add("markdownlint", ev, String::new());
    // pre-commit whitespace hooks
    let pc = read(".pre-commit-config.yaml");
    if pc.contains("trailing-whitespace")
        || pc.contains("end-of-file-fixer")
        || pc.contains("mixed-line-ending")
    {
        add(
            "pre-commit-whitespace",
            vec![".pre-commit-config.yaml".into()],
            pc,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::ws;

    #[test]
    fn detects_config_files() {
        let w = ws(&[
            (".editorconfig", "[*]\nindent_style = space\n"),
            (".prettierrc", "{}"),
            (
                "eslint.config.js",
                "export default [{ rules: { 'no-console': 'error' } }]",
            ),
            ("ruff.toml", "[lint]\nselect = [\"E\", \"I\"]\n"),
            ("AGENTS.md", "x"),
        ]);
        let e = detect(&w);
        assert!(
            e.contains_key("editorconfig")
                && e.contains_key("prettier")
                && e.contains_key("eslint")
                && e.contains_key("ruff")
        );
        assert!(e["eslint"].config_text.contains("no-console"));
        assert!(!e.contains_key("rustfmt"));
    }

    #[test]
    fn detects_tools_run_by_automation() {
        let w = ws(&[
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
            (
                ".github/workflows/ci.yml",
                "run: cargo fmt --all --check\nrun: cargo clippy -- -D warnings\n",
            ),
        ]);
        let e = detect(&w);
        assert_eq!(e["rustfmt"].evidence, vec![".github/workflows/ci.yml"]);
        assert!(e.contains_key("clippy"));
        assert!(e.contains_key("rustc-deny-warnings"));
    }

    #[test]
    fn manifest_alone_is_not_enforcement() {
        let w = ws(&[
            ("Cargo.toml", "[package]\n"),
            ("package.json", "{\"devDependencies\":{\"prettier\":\"3\"}}"),
        ]);
        let e = detect(&w);
        assert!(
            !e.contains_key("rustfmt") && !e.contains_key("prettier") && !e.contains_key("clippy")
        );
    }
}
