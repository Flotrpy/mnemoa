//! MNA008 stale-command.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::{Check, Context, RULES, RuleInfo, finding_at};
use crate::diag::Finding;
use crate::doc::Doc;
use crate::repo::{RepoFacts, Runner};

/// Words that follow `make` in English prose ("make sure", "make sense"), never targets.
const MAKE_PROSE: &[&str] = &[
    "sure", "certain", "sense", "it", "this", "that", "them", "these", "those", "the", "a", "an",
    "any", "all", "your", "our", "use", "changes", "room", "way",
];

/// A script, target or recipe name that is a placeholder or a pattern, not a real name.
fn is_placeholder(name: &str) -> bool {
    name.contains(['<', '>', '*', '{', '}', '$', '[', ']', '|'])
        || name.eq_ignore_ascii_case("...")
        || name.chars().all(|c| c == '.' || c == '_')
        // `bun run X`
        || (name.len() == 1 && name.chars().all(|c| c.is_ascii_uppercase()))
}

/// Words that turn a mention into a "don't run this" (`**Never** use \`npm run build\``).
pub(crate) static NEGATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(never|not|don't|dont|avoid|instead\s+of|rather\s+than|no\s+longer|deprecated)\b",
    )
    .unwrap()
});

/// Evidence that a line creates a file: a download, a build, a copy, an output flag or a
/// redirect. A bare mention of a script's name is not enough.
static CREATES_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(wget|curl|cp|mv|install|touch|unzip|tar|cmake|make|build|go\s+build|cargo\s+build|gcc|clang|download|generate|git\s+clone|chmod)\b|\s-o\s|--output\b|>")
        .unwrap()
});

/// Directories named near a command: in a backticked path or after "from"/"in" ("from the
/// `build` folder", "VS Code extension (`apps/vscode`)").
static DIR_HINT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"`\.?/?([A-Za-z0-9_@][A-Za-z0-9_@./-]*)/?`|\b(?:from|in|inside|under|within)\s+(?:the\s+)?([A-Za-z0-9_@.-]+/[A-Za-z0-9_@./-]*)")
        .unwrap()
});

/// What a command's surroundings say about it.
struct Around<'a> {
    /// The command is a whole inline code span, not a line of a shell block.
    inline: bool,
    /// Directories with a task runner that the nearby text or headings name.
    hints: Vec<String>,
    /// The lines just before the command (a step there may download or build a script).
    prior: &'a str,
    /// The text before the command on its own line.
    line_before: &'a str,
    /// Simple entries of the root .gitignore (build outputs such as `/ollama`).
    ignored: &'a [String],
}

/// Simple root `.gitignore` entries: names and paths, plus `*.ext` suffixes.
fn gitignore_entries(cx: &Context<'_>) -> Vec<String> {
    cx.ws
        .read_file(".gitignore")
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .map(|l| l.trim_start_matches('/').trim_end_matches('/').to_string())
        .filter(|l| !l.contains(['[', '?']) && (!l.contains('*') || l.starts_with("*.")))
        .collect()
}

/// Whether `path` (repository-relative) or one of its parent directories is gitignored.
fn is_ignored(ignored: &[String], path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    let mut prefix = String::new();
    for comp in path.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(comp);
        if ignored
            .iter()
            .any(|e| *e == prefix || (!e.contains('/') && e == comp))
        {
            return true;
        }
    }
    ignored
        .iter()
        .any(|e| e.strip_prefix('*').is_some_and(|ext| base.ends_with(ext)))
}

/// Directories with a task runner that the text around a command (before it, and the rest of
/// its line), or its headings, name.
fn dir_hints(cx: &Context<'_>, doc: &Doc, span_start: usize, span_end: usize) -> Vec<String> {
    let facts = cx.facts();
    let section = doc
        .headings
        .iter()
        .rposition(|h| h.span.start <= span_start);
    let section_start = section.map_or(0, |i| doc.headings[i].span.start);
    let mut from = span_start.saturating_sub(600).max(section_start);
    while !doc.text.is_char_boundary(from) {
        from += 1;
    }
    let mut text = doc.section_path(section).join("\n");
    text.push('\n');
    text.push_str(&doc.text[from..span_start]);
    text.push('\n');
    let line_end = doc.text[span_end..]
        .find('\n')
        .map_or(doc.text.len(), |i| span_end + i);
    text.push_str(&doc.text[span_end..line_end]);
    let doc_dir = parent_of(&doc.path);
    let mut out: Vec<String> = Vec::new();
    for c in DIR_HINT_RE.captures_iter(&text) {
        let raw = c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str());
        let raw = raw.trim_end_matches(['/', '.', ',', ')']);
        for d in [join(doc_dir, raw), join("", raw)].into_iter().flatten() {
            let has_runner = facts.npm.contains_key(&d)
                || facts.make.contains_key(&d)
                || facts.just.contains_key(&d);
            if !d.is_empty() && has_runner && !out.contains(&d) {
                out.push(d);
            }
        }
    }
    out
}

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
    let doc_dir = parent_of(&doc.path);
    let ignored = gitignore_entries(cx);
    commands(doc)
        .into_iter()
        .filter_map(|(cmd, span)| {
            let line_start = doc.text[..span.start].rfind('\n').map_or(0, |i| i + 1);
            let prior_start = doc.text[..line_start]
                .match_indices('\n')
                .rev()
                .nth(6)
                .map_or(0, |(i, _)| i + 1);
            let around = Around {
                inline: doc.code_spans.iter().any(|c| c.span == span),
                hints: dir_hints(cx, doc, span.start, span.end),
                prior: &doc.text[prior_start..line_start],
                line_before: &doc.text[line_start..span.start],
                ignored: &ignored,
            };
            let msg = stale(
                cx,
                facts,
                doc_dir,
                &cmd,
                any_makefile,
                any_justfile,
                &around,
            )?;
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

/// The runner in `dir` itself (make and just do not look in parent directories).
fn runner_in<'a>(map: &'a BTreeMap<String, Runner>, dir: &str) -> Option<(&'a str, &'a Runner)> {
    map.get_key_value(dir).map(|(k, r)| (k.as_str(), r))
}

/// The runner in `dir` or its nearest ancestor (how npm, pnpm, yarn and bun find package.json).
fn runner_at_or_above<'a>(
    map: &'a BTreeMap<String, Runner>,
    dir: &str,
) -> Option<(&'a str, &'a Runner)> {
    let mut d = dir;
    loop {
        if let Some(found) = runner_in(map, d) {
            return Some(found);
        }
        if d.is_empty() {
            return None;
        }
        d = parent_of(d);
    }
}

/// Runners a command could mean. An instruction file's commands may be run from its own
/// directory or from the repository root, so without an explicit `cd` (or `-C`) both count.
fn candidates<'a>(
    map: &'a BTreeMap<String, Runner>,
    dir: &str,
    explicit_dir: bool,
    walk_up: bool,
    hints: &[String],
) -> Vec<(&'a str, &'a Runner)> {
    let find = |d: &str| {
        if walk_up {
            runner_at_or_above(map, d)
        } else {
            runner_in(map, d)
        }
    };
    let mut out: Vec<(&str, &Runner)> = find(dir).into_iter().collect();
    if !explicit_dir {
        // The root, and directories the surrounding text names ("from `apps/vscode`").
        for d in std::iter::once("").chain(hints.iter().map(String::as_str)) {
            if let Some(found) = find(d)
                && !out.iter().any(|(k, _)| *k == found.0)
            {
                out.push(found);
            }
        }
    }
    out
}

/// Extensions a module path may leave off (`./helpers/assertions` for `assertions.ts`).
const MODULE_EXTS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "svelte", "py", "rs", "go",
];

/// Whether `p` (relative to `dir`, or to the root when no `cd` was given) is missing while its
/// parent directory exists, so the path looks wrong rather than generated or external.
fn path_missing(cx: &Context<'_>, dir: &str, p: &str, explicit_dir: bool) -> Option<bool> {
    let mut bases = vec![dir];
    if !explicit_dir && !dir.is_empty() {
        bases.push("");
    }
    let mut parent_seen = false;
    for base in bases {
        let path = join(base, p)?;
        if cx.ws.discovery.exists(&path) {
            return Some(false);
        }
        // An extensionless path that names a source module is an import, not a missing program.
        let extensionless = !path.rsplit('/').next().unwrap_or("").contains('.');
        if extensionless
            && MODULE_EXTS
                .iter()
                .any(|e| cx.ws.discovery.exists(&format!("{path}.{e}")))
        {
            return Some(false);
        }
        parent_seen |= cx.ws.discovery.has_dir(parent_of(&path));
    }
    Some(parent_seen)
}

/// Why `line` is stale, or `None` when it is fine or cannot be judged. Relative commands start
/// in `doc_dir`, the directory of the instruction file that mentions them.
fn stale(
    cx: &Context<'_>,
    facts: &RepoFacts,
    doc_dir: &str,
    line: &str,
    any_make: bool,
    any_just: bool,
    around: &Around<'_>,
) -> Option<String> {
    // "**Never** use `npm run build:production`" is not an instruction to run it.
    if NEGATION_RE.is_match(around.line_before) {
        return None;
    }
    // A span that is one path with spaces in it (`docs/architecture docs/x.md`) is a reference.
    let whole = line.trim().trim_start_matches("./");
    if whole.contains(' ')
        && [join(doc_dir, whole), join("", whole)]
            .into_iter()
            .flatten()
            .any(|p| cx.ws.discovery.exists(&p))
    {
        return None;
    }
    let mut cwd = doc_dir.to_string();
    let mut explicit_dir = false;
    let mut earlier: Vec<String> = Vec::new();
    for seg in split_segments(line) {
        let mut toks: Vec<&str> = seg.split_whitespace().collect();
        // A `# comment` ends the command.
        if let Some(i) = toks.iter().position(|t| t.starts_with('#')) {
            toks.truncate(i);
        }
        while toks.first().is_some_and(|t| {
            t.contains('=') && !t.starts_with('-') || *t == "sudo" || *t == "time" || *t == "env"
        }) {
            toks.remove(0);
        }
        let seg_text = seg.clone();
        // A script fetched or built by an earlier step on this line or the lines above, or a
        // gitignored build output (`./ollama` after `cmake --build`).
        let created_earlier = |name: &str| {
            let base = name.rsplit('/').next().unwrap_or(name);
            let rel = name.trim_start_matches("./");
            let creates = |l: &str| l.contains(base) && CREATES_RE.is_match(l);
            !base.is_empty()
                && (earlier.iter().any(|e| creates(e))
                    || around.prior.lines().any(creates)
                    || [join(&cwd, rel), join("", rel)]
                        .into_iter()
                        .flatten()
                        .any(|p| is_ignored(around.ignored, &p)))
        };
        let Some(&prog) = toks.first() else {
            earlier.push(seg_text);
            continue;
        };
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
                // `cd x` in a nested instruction file usually means `x` next to that file, but
                // some files give paths from the repository root. Use whichever exists.
                let here = join(&cwd, target)?;
                cwd = if cx.ws.discovery.has_dir(&here) {
                    here
                } else {
                    let from_root = join("", target)?;
                    if !cx.ws.discovery.has_dir(&from_root) {
                        return None;
                    }
                    from_root
                };
                explicit_dir = true;
            }
            "make" | "gmake" => {
                let mut dir = cwd.clone();
                let mut explicit_make_dir = explicit_dir;
                let mut targets = Vec::new();
                let mut i = 0;
                while i < args.len() {
                    let a = args[i];
                    if a == "-C" || a == "--directory" {
                        dir = join(&cwd, args.get(i + 1)?)?;
                        explicit_make_dir = true;
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
                    earlier.push(seg_text);
                    continue;
                }
                if targets.iter().any(|t| is_placeholder(t)) || MAKE_PROSE.contains(&targets[0]) {
                    return None;
                }
                let cands = candidates(&facts.make, &dir, explicit_make_dir, false, &around.hints);
                if cands.is_empty() {
                    if !any_make {
                        return Some(format!("`{seg}` runs make, but the repo has no Makefile."));
                    }
                    return None;
                }
                if let Some(msg) = missing(&cands, &targets, "target", "Makefile", &seg) {
                    return Some(msg);
                }
            }
            "just" => {
                let recipe = args.iter().find(|a| !a.starts_with('-'))?;
                if is_placeholder(recipe) {
                    return None;
                }
                if args
                    .iter()
                    .any(|a| matches!(*a, "-f" | "--justfile" | "-d" | "--working-directory"))
                {
                    return None;
                }
                let cands = candidates(&facts.just, &cwd, explicit_dir, false, &around.hints);
                if cands.is_empty() {
                    if !any_just {
                        return Some(format!("`{seg}` runs just, but the repo has no justfile."));
                    }
                    return None;
                }
                if let Some(msg) = missing(&cands, &[recipe], "recipe", "justfile", &seg) {
                    return Some(msg);
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
                    earlier.push(seg_text);
                    continue;
                }
                let script = args.get(1).filter(|s| !s.starts_with('-'))?;
                if is_placeholder(script) {
                    return None;
                }
                if prog == "bun" && (script.contains('.') || script.contains('/')) {
                    earlier.push(seg_text);
                    continue;
                }
                let cands = candidates(&facts.npm, &cwd, explicit_dir, true, &around.hints);
                if cands.is_empty() {
                    return None;
                }
                if let Some(msg) = missing(&cands, &[script], "script", "package.json", &seg) {
                    return Some(msg);
                }
            }
            "cargo" => {
                let sub = args
                    .iter()
                    .find(|a| !a.starts_with('-') && !a.starts_with('+'))?;
                let aliases = facts.cargo_aliases.as_ref()?;
                // Unknown subcommands may be installed plugins (`cargo xwin`). Only one that
                // looks like a renamed alias (`cargo xtask` with alias `xt`) is reported.
                let like_alias = aliases
                    .names
                    .iter()
                    .any(|a| a.len() >= 2 && (sub.starts_with(a.as_str()) || a.starts_with(*sub)));
                if !CARGO_BUILTIN.contains(sub) && !aliases.has(sub) && like_alias {
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
                if let Some(msg) = missing(&[("", r)], &[script], "script", "pyproject.toml", &seg)
                {
                    return Some(msg);
                }
            }
            "poe" => {
                let task = args.iter().find(|a| !a.starts_with('-'))?;
                let r = facts.py.poe.as_ref()?;
                if let Some(msg) = missing(&[("", r)], &[task], "task", "pyproject.toml", &seg) {
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
                if let Some(msg) = missing(&[("", r)], &[task], "task", file, &seg) {
                    return Some(msg);
                }
            }
            "bash" | "sh" | "zsh" | "python" | "python3" | "node" | "deno" | "ruby" | "perl" => {
                let script = args.iter().find(|a| !a.starts_with('-'))?;
                if script.contains('/')
                    && !script.contains(['*', '$', '<'])
                    && !script.starts_with('/')
                    && !created_earlier(script)
                    && path_missing(cx, &cwd, script, explicit_dir)?
                {
                    return Some(format!("`{seg}`: script `{script}` does not exist."));
                }
            }
            p if p.contains('/') && !p.starts_with('/') && !p.contains(['*', '~', '<']) => {
                // A bare path with no arguments is only a command if it looks runnable: a shell
                // script, or `./name` with no extension or a script extension. Anything else
                // (`packages/core`, `tools/emit.py`, `./foo.ts`) is a file reference, which is
                // MNA007's job.
                let ext = p
                    .rsplit('/')
                    .next()
                    .and_then(|f| f.rsplit_once('.'))
                    .map(|(_, e)| e);
                let shell_script = matches!(ext, Some("sh" | "bash" | "zsh"));
                let dot_exec = p.starts_with("./")
                    && matches!(ext, None | Some("sh" | "bash" | "zsh" | "py" | "rb" | "pl"));
                // In prose, `./pg` or `./helpers` is an import or export path; only a shell
                // script is clearly a command there. A shell block line can run `./tool`.
                let runnable = if around.inline {
                    shell_script
                } else {
                    shell_script || dot_exec
                };
                // Directory listings (`entities/   entity classes`), tool output
                // (`extensions/copilot: 329 packages`) and lists of paths are not commands.
                let not_a_program = p.ends_with(['/', ':', ',', ';'])
                    || args.iter().any(|a| a.contains('/') && !a.starts_with('-'));
                if not_a_program || (args.is_empty() && !runnable) {
                    earlier.push(seg_text);
                    continue;
                }
                if !created_earlier(p) && path_missing(cx, &cwd, p, explicit_dir)? {
                    return Some(format!("`{seg}`: `{p}` does not exist."));
                }
            }
            _ => {}
        }
        earlier.push(seg_text);
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

/// Names that none of the candidate runners (`(dir, runner)`) defines, as a message. Silent
/// when any candidate is opaque, since its list may be incomplete.
fn missing(
    cands: &[(&str, &Runner)],
    names: &[&str],
    what: &str,
    file_name: &str,
    seg: &str,
) -> Option<String> {
    let (first_dir, first) = *cands.first()?;
    if cands.iter().any(|(_, r)| r.opaque) {
        return None;
    }
    let gone: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !cands.iter().any(|(_, r)| r.has(n)))
        .collect();
    if gone.is_empty() {
        return None;
    }
    let files: Vec<String> = cands.iter().map(|(d, _)| file_in(d, file_name)).collect();
    let mut known: Vec<&str> = first.names.iter().map(String::as_str).collect();
    known.truncate(8);
    let known = if known.is_empty() {
        "none".to_string()
    } else {
        known.join(", ")
    };
    let gone = gone.join("`, `");
    Some(if files.len() == 1 {
        format!(
            "`{seg}`: no {what} `{gone}` in {} (has: {known}).",
            files[0]
        )
    } else {
        format!(
            "`{seg}`: no {what} `{gone}` in {} ({} has: {known}).",
            files.join(" or "),
            file_in(first_dir, file_name)
        )
    })
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
    fn bare_source_file_paths_are_not_commands() {
        let f = check(
            &StaleCommand,
            &[
                ("src/a.rs", ""),
                ("scripts/other.sh", ""),
                (
                    "AGENTS.md",
                    "1. Add the route in `src/routes.ts`.\n2. Run `scripts/gen.sh`.\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("scripts/gen.sh"));
    }

    #[test]
    fn directory_with_trailing_slash_is_not_a_command() {
        let f = check(
            &StaleCommand,
            &[
                ("src/a.rs", ""),
                (
                    "AGENTS.md",
                    "- Files under `fixtures/` are test data.\n- `src/gone/` too.\n",
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

    // Near misses below come from findings labeled wrong in notebooks/results/labels.csv.

    #[test]
    fn nested_instruction_file_uses_its_own_package_json() {
        // appwrite apps/console/AGENTS.md; modelcontextprotocol src/everything/AGENTS.md.
        let files = [
            (
                "package.json",
                r#"{"scripts":{"format:compose":"x","lint:compose":"y"}}"#,
            ),
            (
                "apps/console/package.json",
                r#"{"scripts":{"generate:cover-manifest":"x","lint":"y"}}"#,
            ),
            (
                "apps/console/AGENTS.md",
                "- `bun run generate:cover-manifest`\n- `bun run lint`\n- `bun run format:compose`\n",
            ),
        ];
        let f = check(&StaleCommand, &files);
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn nested_file_still_flags_a_script_no_package_defines() {
        let f = check(
            &StaleCommand,
            &[
                ("package.json", r#"{"scripts":{"build":"x"}}"#),
                ("apps/web/package.json", r#"{"scripts":{"dev":"x"}}"#),
                ("apps/web/AGENTS.md", "- `npm run typecheck`\n"),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(
            f[0].message
                .contains("apps/web/package.json or package.json"),
            "{}",
            f[0].message
        );
    }

    #[test]
    fn explicit_cd_uses_only_that_package() {
        // payload AGENTS.md: a real stale command, labeled correct.
        let f = check(
            &StaleCommand,
            &[
                ("package.json", r#"{"scripts":{"translateNewKeys":"x"}}"#),
                (
                    "packages/richtext-lexical/package.json",
                    r#"{"scripts":{"build":"x"}}"#,
                ),
                (
                    "AGENTS.md",
                    "```bash\ncd packages/richtext-lexical && pnpm run translateNewKeys\n```\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn placeholders_and_globs_are_not_script_names() {
        // electron CLAUDE.md, cline .clinerules, appwrite, bun CLAUDE.md.
        let f = check(
            &StaleCommand,
            &[
                (
                    "package.json",
                    r#"{"scripts":{"build":"x","build:debug":"y"}}"#,
                ),
                ("Makefile", "lint:\n\tx\n"),
                (
                    "AGENTS.md",
                    "- `yarn run <tool>`\n- `npm run <script>`\n- `bun run i18n:audit:<lang>`\n- `bun run build*`\n- `make <target>`\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn file_and_directory_references_are_not_commands() {
        // PostHog scout_harness and desktop AGENTS.md, cline hooks README, TypeScript
        // copilot-instructions, storybook AGENTS.md.
        let f = check(
            &StaleCommand,
            &[
                ("src/a.ts", ""),
                ("tools/x.py", ""),
                (
                    "AGENTS.md",
                    "- See `tools/emit.py`.\n- `packages/core` holds logic.\n- `.clinerules/hooks/TaskResume`\n- `tsc/testdata/baselines/local`\n- Use explicit extensions on relative TS imports (`./foo.ts`).\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn paths_resolve_from_the_instruction_files_directory() {
        let f = check(
            &StaleCommand,
            &[
                ("tools/run.sh", ""),
                ("svc/tools/emit.sh", ""),
                (
                    "svc/AGENTS.md",
                    "- `bash tools/emit.sh`\n- `./tools/run.sh`\n- `bash tools/gone.sh`\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("gone.sh"));
    }

    #[test]
    fn tree_entries_with_comments_are_not_commands() {
        // n8n packages/@n8n/agents/AGENTS.md.
        let f = check(
            &StaleCommand,
            &[
                ("src/a.ts", ""),
                (
                    "AGENTS.md",
                    "```\nutils/              # JSON typing helpers\n```\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn script_downloaded_earlier_in_the_line_is_not_stale() {
        // microsoft/autogen copilot-instructions.
        let f = check(
            &StaleCommand,
            &[
                ("src/a.cs", ""),
                (
                    "AGENTS.md",
                    "```bash\nwget https://dot.net/v1/dotnet-install.sh && chmod +x ./dotnet-install.sh && ./dotnet-install.sh --channel 9.0\n./setup.sh --x\n```\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("setup.sh"));
    }

    #[test]
    fn cd_falls_back_to_a_root_relative_directory() {
        // PostHog proto/AGENTS.md: `cd nodejs` means the repository's nodejs/, not proto/nodejs/.
        let files = [
            ("package.json", r#"{"scripts":{"build":"x"}}"#),
            (
                "nodejs/package.json",
                r#"{"scripts":{"generate:proto":"x"}}"#,
            ),
            ("proto/x.proto", ""),
            (
                "proto/AGENTS.md",
                "```bash\ncd nodejs && pnpm run generate:proto\ncd nodejs && pnpm run gone\ncd missing && pnpm run whatever\n```\n",
            ),
        ];
        let f = check(&StaleCommand, &files);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("gone"));
    }

    #[test]
    fn module_import_paths_are_not_missing_programs() {
        // grafana e2e-playwright/dashboard-new-layouts/AGENTS.md.
        let f = check(
            &StaleCommand,
            &[
                ("e2e/helpers/assertions.ts", ""),
                (
                    "e2e/AGENTS.md",
                    "- Import from `./helpers/assertions`.\n\n```bash\n./helpers/assertions\n./helpers/setup\n```\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("setup"));
    }

    // Round two: findings labeled wrong after the first fixes, re-scanning 38 repositories.

    #[test]
    fn directories_named_in_headings_or_text_count() {
        // cline AGENTS.md "VS Code extension (`apps/vscode`)"; vscode "from the `build`
        // folder"; sst/opencode "from `packages/client`".
        let files = [
            ("package.json", r#"{"scripts":{"build":"x"}}"#),
            (
                "apps/vscode/package.json",
                r#"{"scripts":{"protos":"x","package":"y"}}"#,
            ),
            ("build/package.json", r#"{"scripts":{"typecheck":"x"}}"#),
            (
                "AGENTS.md",
                "## VS Code extension (`apps/vscode`)\n\n- `bun run protos` (from `apps/vscode`)\n- `bun run package`\n\n## Build scripts\n\nRun `npm run typecheck` from the `build` folder.\n\n## Other\n\n- `bun run protos`\n",
            ),
        ];
        let f = check(&StaleCommand, &files);
        // Only the last one: nothing near it names apps/vscode, and the root lacks `protos`.
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].line, 12);
    }

    #[test]
    fn negated_commands_are_not_instructions() {
        // dotnet/aspnetcore src/Components/AGENTS.md.
        let f = check(
            &StaleCommand,
            &[
                ("package.json", r#"{"scripts":{"build":"x"}}"#),
                (
                    "AGENTS.md",
                    "- **Never** use `npm run build:production` here.\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn prose_paths_and_listings_are_not_commands() {
        // medusa exports subpaths, trpc test helper imports, n8n layout block, vscode tool
        // output, prisma link text for a folder with a space in its name.
        let f = check(
            &StaleCommand,
            &[
                ("packages/deps/src/index.ts", ""),
                ("docs/architecture docs/patterns/x.md", ""),
                ("extensions/a.ts", ""),
                (
                    "packages/deps/AGENTS.md",
                    "Subpaths: `./pg`, `./zod`, `./awilix`.\nImport `./__testHelpers`.\n\n```\nentities/        entity classes\n```\n\n- `extensions/copilot: 329 packages`\n",
                ),
                (
                    "AGENTS.md",
                    "See [`docs/architecture docs/patterns/x.md`](x).\n",
                ),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn scripts_built_or_downloaded_just_before_are_not_stale() {
        // ollama AGENTS.md (gitignored /ollama), autogen (wget on the line before).
        let f = check(
            &StaleCommand,
            &[
                (".gitignore", "build\n/ollama\n"),
                ("src/main.go", ""),
                (
                    "AGENTS.md",
                    "```sh\ncmake -B build .\ncmake --build build --parallel 8\n./ollama serve\nwget https://dot.net/v1/dotnet-install.sh\n./dotnet-install.sh --channel 8.0\n./missing.sh\n```\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("missing.sh"));
    }

    #[test]
    fn mentioning_a_script_is_not_creating_it() {
        // Codex review on this change: a prose mention must not hide a missing script.
        let f = check(
            &StaleCommand,
            &[
                ("src/main.go", ""),
                (
                    "AGENTS.md",
                    "Run setup.sh to configure the tools:\n\n```sh\n./setup.sh\n```\n\n```sh\ncurl -o setup2.sh https://x/setup2.sh\n./setup2.sh\n```\n",
                ),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("`./setup.sh`"), "{f:?}");
    }

    #[test]
    fn placeholder_letter_and_cargo_plugins() {
        // cline `bun run X`; uv `cargo xwin clippy` with only a `dev` alias.
        let f = check(
            &StaleCommand,
            &[
                ("package.json", r#"{"scripts":{"build":"x"}}"#),
                (".cargo/config.toml", "[alias]\ndev = \"run -p dev\"\n"),
                (
                    "AGENTS.md",
                    "- `bun run X`\n- `cargo xwin clippy`\n- `cargo devtool`\n",
                ),
            ],
        );
        // `cargo devtool` looks like the renamed `dev` alias.
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("devtool"));
    }

    #[test]
    fn missing_script_with_arguments_is_still_flagged() {
        // zed .rules: `script/crash-to-prompt <issue-id>` no longer exists; labeled correct.
        let f = check(
            &StaleCommand,
            &[
                ("script/sentry-fetch", ""),
                ("AGENTS.md", "- Run `script/crash-to-prompt <issue-id>`.\n"),
            ],
        );
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn make_in_english_prose_is_not_a_command() {
        // mastra docs/styleguides/STYLEGUIDE.md: "Use `Ensure`, not `make sure`".
        let f = check(
            &StaleCommand,
            &[(
                "AGENTS.md",
                "- Use `Ensure`, not `make sure`.\n- `make sense`\n",
            )],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn root_makefile_counts_for_a_file_in_a_subdirectory() {
        // huggingface/transformers .github/copilot-instructions.md: labeled correct.
        let files = [
            ("Makefile", "style:\n\tx\n"),
            (
                ".github/copilot-instructions.md",
                "- `make fixup`\n- `make style`\n",
            ),
        ];
        let f = check(&StaleCommand, &files);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("fixup"));
    }

    #[test]
    fn segments_split_on_operators_but_not_quotes() {
        assert_eq!(split_segments("a && b | c; d"), vec!["a", "b", "c", "d"]);
        assert_eq!(split_segments("echo 'a && b'"), vec!["echo 'a && b'"]);
    }
}
