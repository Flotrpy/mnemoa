//! MNA008 stale-command.

use super::{Check, Context, RULES, RuleInfo, finding_at};
use crate::diag::Finding;
use crate::doc::Doc;
use crate::repo::{RepoFacts, Runner};

const SHELL_LANGS: &[&str] = &[
    "",
    "sh",
    "bash",
    "shell",
    "console",
    "zsh",
    "shell-session",
    "fish",
    "terminal",
];

const CARGO_BUILTIN: &[&str] = &[
    "add",
    "bench",
    "build",
    "check",
    "clean",
    "clippy",
    "config",
    "doc",
    "fetch",
    "fix",
    "fmt",
    "generate-lockfile",
    "help",
    "info",
    "init",
    "install",
    "locate-project",
    "login",
    "logout",
    "metadata",
    "miri",
    "new",
    "owner",
    "package",
    "pkgid",
    "publish",
    "remove",
    "report",
    "run",
    "rustc",
    "rustdoc",
    "search",
    "test",
    "tree",
    "uninstall",
    "update",
    "vendor",
    "verify-project",
    "version",
    "yank",
    "b",
    "c",
    "d",
    "r",
    "t",
    // Common third-party subcommands.
    "nextest",
    "llvm-cov",
    "tarpaulin",
    "insta",
    "deny",
    "audit",
    "machete",
    "watch",
    "expand",
    "make",
    "release",
    "binstall",
    "udeps",
    "outdated",
    "edit",
    "hack",
    "dist",
    "zigbuild",
    "flamegraph",
    "bloat",
    "sort",
    "semver-checks",
    "mutants",
    "careful",
    "set-version",
    "upgrade",
    "criterion",
    "shear",
    "about",
    "auditable",
    "component",
    "cross",
    "leptos",
    "tauri",
    "wasm-pack",
];

const PY_TOOLS: &[&str] = &[
    "python",
    "python3",
    "pytest",
    "py.test",
    "ruff",
    "black",
    "mypy",
    "flake8",
    "pip",
    "uvicorn",
    "django-admin",
    "alembic",
    "coverage",
    "tox",
    "nox",
    "pre-commit",
    "sphinx-build",
    "mkdocs",
    "jupyter",
    "ipython",
    "isort",
    "pylint",
    "bandit",
    "pyright",
    "gunicorn",
    "celery",
];

/// MNA008.
pub struct StaleCommand;

impl Check for StaleCommand {
    fn info(&self) -> &'static RuleInfo {
        &RULES[7]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        for doc in cx.instruction_docs() {
            for (_, span, msg) in stale_commands(cx, doc) {
                out.push(
                    finding_at(self.info(), doc, span, msg)
                        .with_help("Update the command to one the repo defines, or remove it."),
                );
            }
        }
    }
}

/// Stale commands in one document: (command, span, reason).
pub(crate) fn stale_commands(
    cx: &Context<'_>,
    doc: &Doc,
) -> Vec<(String, std::ops::Range<usize>, String)> {
    let facts = cx.facts();
    let any_makefile = !facts.make.is_empty();
    let any_justfile = !facts.just.is_empty();
    commands(doc)
        .into_iter()
        .filter_map(|(cmd, span)| {
            let msg = stale(cx, facts, &cmd, any_makefile, any_justfile)?;
            Some((cmd, span, msg))
        })
        .collect()
}

/// Every command line in a document with its span: whole code spans and lines of shell blocks.
pub(crate) fn commands(doc: &Doc) -> Vec<(String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    for c in &doc.code_spans {
        out.push((c.code.trim().to_string(), c.span.clone()));
    }
    for b in doc
        .code_blocks
        .iter()
        .filter(|b| b.fenced && SHELL_LANGS.contains(&b.lang.as_str()))
    {
        let block_text = &doc.text[b.span.clone()];
        let mut search_from = 0;
        for raw in b.code.lines() {
            let line = raw.trim();
            let line = line
                .strip_prefix("$ ")
                .or_else(|| line.strip_prefix("> "))
                .unwrap_or(line);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let start = block_text[search_from..]
                .find(raw)
                .map(|i| b.span.start + search_from + i)
                .unwrap_or(b.span.start);
            search_from = (start - b.span.start + raw.len()).min(block_text.len());
            out.push((line.to_string(), start..start + raw.len()));
        }
    }
    out
}

fn split_segments(line: &str) -> Vec<String> {
    let mut segs = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                cur.push(c);
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                cur.push(c);
            }
            (None, '&') if chars.peek() == Some(&'&') => {
                chars.next();
                segs.push(std::mem::take(&mut cur));
            }
            (None, '|') => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                segs.push(std::mem::take(&mut cur));
            }
            (None, ';') => segs.push(std::mem::take(&mut cur)),
            (None, c) => cur.push(c),
        }
    }
    segs.push(cur);
    segs.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn join(dir: &str, sub: &str) -> Option<String> {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for seg in sub.trim_end_matches('/').split('/') {
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

/// Why `line` is stale, or `None` when it is fine or cannot be judged.
fn stale(
    cx: &Context<'_>,
    facts: &RepoFacts,
    line: &str,
    any_make: bool,
    any_just: bool,
) -> Option<String> {
    let mut cwd = String::new();
    for seg in split_segments(line) {
        let mut toks: Vec<&str> = seg.split_whitespace().collect();
        while toks.first().is_some_and(|t| {
            t.contains('=') && !t.starts_with('-') || *t == "sudo" || *t == "time" || *t == "env"
        }) {
            toks.remove(0);
        }
        let Some(&prog) = toks.first() else { continue };
        if prog.contains(['$', '<', '>', '{', '`']) {
            return None;
        }
        let args = &toks[1..];
        match prog {
            "cd" => {
                let target = args.first()?;
                if target.contains(['$', '~', '<']) || target.starts_with('/') {
                    return None;
                }
                cwd = join(&cwd, target)?;
            }
            "make" | "gmake" => {
                let mut dir = cwd.clone();
                let mut targets = Vec::new();
                let mut i = 0;
                while i < args.len() {
                    let a = args[i];
                    if a == "-C" || a == "--directory" {
                        dir = join(&cwd, args.get(i + 1)?)?;
                        i += 2;
                        continue;
                    }
                    if a == "-f" || a == "--file" || a.starts_with("--file=") {
                        return None;
                    }
                    if a.starts_with('-') || a.contains('=') {
                        i += 1;
                        continue;
                    }
                    targets.push(a);
                    i += 1;
                }
                if targets.is_empty() {
                    continue;
                }
                match facts.make.get(&dir) {
                    Some(r) => {
                        if let Some(msg) =
                            missing(r, &targets, "target", &file_in(&dir, "Makefile"), &seg)
                        {
                            return Some(msg);
                        }
                    }
                    None if !any_make => {
                        return Some(format!("`{seg}` runs make, but the repo has no Makefile."));
                    }
                    None => return None,
                }
            }
            "just" => {
                let recipe = args.iter().find(|a| !a.starts_with('-'))?;
                if args
                    .iter()
                    .any(|a| matches!(*a, "-f" | "--justfile" | "-d" | "--working-directory"))
                {
                    return None;
                }
                match facts.just.get(&cwd) {
                    Some(r) => {
                        if let Some(msg) =
                            missing(r, &[recipe], "recipe", &file_in(&cwd, "justfile"), &seg)
                        {
                            return Some(msg);
                        }
                    }
                    None if !any_just => {
                        return Some(format!("`{seg}` runs just, but the repo has no justfile."));
                    }
                    None => return None,
                }
            }
            "npm" | "pnpm" | "yarn" | "bun" => {
                if args.iter().any(|a| {
                    matches!(
                        *a,
                        "-w" | "--workspace"
                            | "--filter"
                            | "-F"
                            | "--prefix"
                            | "-C"
                            | "--dir"
                            | "--cwd"
                            | "-r"
                            | "--recursive"
                    ) || a.starts_with("--workspace=")
                        || a.starts_with("--filter=")
                        || a.starts_with("--prefix=")
                }) {
                    return None;
                }
                let sub = args.first()?;
                if *sub != "run" && *sub != "run-script" {
                    continue;
                }
                let script = args.get(1).filter(|s| !s.starts_with('-'))?;
                if prog == "bun" && (script.contains('.') || script.contains('/')) {
                    continue;
                }
                let r = facts.npm.get(&cwd)?;
                if let Some(msg) =
                    missing(r, &[script], "script", &file_in(&cwd, "package.json"), &seg)
                {
                    return Some(msg);
                }
            }
            "cargo" => {
                let sub = args
                    .iter()
                    .find(|a| !a.starts_with('-') && !a.starts_with('+'))?;
                let aliases = facts.cargo_aliases.as_ref()?;
                if !CARGO_BUILTIN.contains(sub) && !aliases.has(sub) {
                    return Some(format!(
                        "`{seg}`: `{sub}` is not a cargo command or an alias in .cargo/config.toml."
                    ));
                }
            }
            "pdm" | "hatch" => {
                if args.first() != Some(&"run") {
                    continue;
                }
                let script = args.get(1)?;
                if PY_TOOLS.contains(script)
                    || facts.py.project.contains(*script)
                    || script.contains(['/', '.'])
                {
                    continue;
                }
                let r = if prog == "pdm" {
                    facts.py.pdm.as_ref()
                } else {
                    facts.py.hatch.as_ref()
                }?;
                if let Some(msg) = missing(r, &[script], "script", "pyproject.toml", &seg) {
                    return Some(msg);
                }
            }
            "poe" => {
                let task = args.iter().find(|a| !a.starts_with('-'))?;
                let r = facts.py.poe.as_ref()?;
                if let Some(msg) = missing(r, &[task], "task", "pyproject.toml", &seg) {
                    return Some(msg);
                }
            }
            "task" => {
                let task = args.iter().find(|a| !a.starts_with('-'))?;
                let (r, file) = match (&facts.taskfile, &facts.py.taskipy) {
                    (Some(r), _) => (r, "Taskfile.yml"),
                    (None, Some(r)) => (r, "pyproject.toml"),
                    _ => return None,
                };
                if let Some(msg) = missing(r, &[task], "task", file, &seg) {
                    return Some(msg);
                }
            }
            "bash" | "sh" | "zsh" | "python" | "python3" | "node" | "deno" | "ruby" | "perl" => {
                let script = args.iter().find(|a| !a.starts_with('-'))?;
                if script.contains('/') && !script.contains(['*', '$']) && !script.starts_with('/')
                {
                    let p = join(&cwd, script)?;
                    if !cx.ws.discovery.exists(&p) && cx.ws.discovery.has_dir(parent_of(&p)) {
                        return Some(format!("`{seg}`: script `{script}` does not exist."));
                    }
                }
            }
            p if p.contains('/') && !p.starts_with('/') && !p.contains(['*', '~']) => {
                let path = join(&cwd, p)?;
                let parent = parent_of(&path);
                if !cx.ws.discovery.exists(&path) && cx.ws.discovery.has_dir(parent) {
                    return Some(format!("`{seg}`: `{p}` does not exist."));
                }
            }
            _ => {}
        }
    }
    None
}

fn parent_of(p: &str) -> &str {
    p.rfind('/').map(|i| &p[..i]).unwrap_or("")
}

fn file_in(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn missing(r: &Runner, names: &[&str], what: &str, file: &str, seg: &str) -> Option<String> {
    if r.opaque {
        return None;
    }
    let gone: Vec<&str> = names.iter().copied().filter(|n| !r.has(n)).collect();
    if gone.is_empty() {
        return None;
    }
    let mut known: Vec<&str> = r.names.iter().map(String::as_str).collect();
    known.truncate(8);
    Some(format!(
        "`{seg}`: no {what} `{}` in {file} (has: {}).",
        gone.join("`, `"),
        if known.is_empty() {
            "none".to_string()
        } else {
            known.join(", ")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::testutil::check;

    const MAKEFILE: &str = ".PHONY: lint test\nlint:\n\tx\ntest:\n\ty\n";

    #[test]
    fn missing_make_target_is_flagged() {
        let f = check(
            &StaleCommand,
            &[
                ("Makefile", MAKEFILE),
                ("AGENTS.md", "- Run `make check` before committing.\n"),
            ],
        );
        assert_eq!(f.len(), 1);
        assert!(
            f[0].message.contains("no target `check`"),
            "{}",
            f[0].message
        );
    }

    #[test]
    fn existing_make_targets_and_variables_pass() {
        let f = check(
            &StaleCommand,
            &[
                ("Makefile", MAKEFILE),
                (
                    "AGENTS.md",
                    "- Run `make lint test`.\n- Run `make -j4 test V=1`.\n- `make`\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn include_makes_makefile_opaque() {
        let f = check(
            &StaleCommand,
            &[
                ("Makefile", "include ci.mk\nlint:\n\tx\n"),
                ("AGENTS.md", "Run `make release`.\n"),
            ],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn make_without_any_makefile_is_flagged() {
        let f = check(&StaleCommand, &[("AGENTS.md", "Run `make lint`.\n")]);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn cd_into_subdir_uses_its_makefile() {
        let f = check(
            &StaleCommand,
            &[
                ("Makefile", MAKEFILE),
                ("web/Makefile", "build:\n\tx\n"),
                (
                    "AGENTS.md",
                    "```sh\ncd web && make build\n$ make -C web build\n```\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn npm_run_missing_script_in_code_block() {
        let f = check(
            &StaleCommand,
            &[
                (
                    "package.json",
                    r#"{"scripts":{"build":"vite build","test":"vitest"}}"#,
                ),
                (
                    "AGENTS.md",
                    "```bash\nnpm run build\npnpm run typecheck\n```\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("typecheck"));
        assert_eq!(f[0].line, 3);
    }

    #[test]
    fn npm_near_misses() {
        let f = check(
            &StaleCommand,
            &[
                ("package.json", r#"{"scripts":{"build":"x"}}"#),
                (
                    "AGENTS.md",
                    "- `npm install`\n- `pnpm --filter web run dev`\n- `bun run src/index.ts`\n- `npm test`\n```python\nnpm run nope\n```\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn just_recipes() {
        let files = [
            ("justfile", "test:\n    cargo test\n"),
            ("AGENTS.md", "- `just test`\n- `just deploy prod`\n"),
        ];
        let f = check(&StaleCommand, &files);
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("deploy"));
    }

    #[test]
    fn cargo_alias_only_checked_when_aliases_exist() {
        let without = check(&StaleCommand, &[("AGENTS.md", "- `cargo xtask dist`\n")]);
        assert!(without.is_empty());
        let with = check(
            &StaleCommand,
            &[
                (".cargo/config.toml", "[alias]\nxt = \"run -p xtask\"\n"),
                (
                    "AGENTS.md",
                    "- `cargo xtask dist`\n- `cargo xt`\n- `cargo nextest run`\n",
                ),
            ],
        );
        assert_eq!(with.len(), 1);
        assert!(with[0].message.contains("xtask"));
    }

    #[test]
    fn script_paths() {
        let f = check(
            &StaleCommand,
            &[
                ("scripts/setup.sh", ""),
                (
                    "AGENTS.md",
                    "- `./scripts/setup.sh`\n- `bash scripts/gone.sh`\n- `./scripts/old.sh --x`\n- `./bin/tool` (built later)\n",
                ),
            ],
        );
        assert_eq!(f.len(), 2, "{f:?}");
    }

    #[test]
    fn directory_paths_in_code_spans_are_not_commands() {
        let f = check(
            &StaleCommand,
            &[
                ("crates/core/src/lib.rs", ""),
                (
                    "AGENTS.md",
                    "- `crates/core`: the library.\n- `crates/core/` too.\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn pyproject_scripts() {
        let f = check(
            &StaleCommand,
            &[
                (
                    "pyproject.toml",
                    "[tool.pdm.scripts]\nlint = \"ruff check\"\n",
                ),
                (
                    "AGENTS.md",
                    "- `pdm run lint`\n- `pdm run pytest -q`\n- `pdm run typecheck`\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("typecheck"));
    }

    #[test]
    fn segments_split_on_operators_but_not_quotes() {
        assert_eq!(split_segments("a && b | c; d"), vec!["a", "b", "c", "d"]);
        assert_eq!(split_segments("echo 'a && b'"), vec!["echo 'a && b'"]);
    }
}
