//! `forget`: one prune plan that shrinks always-loaded context.
//!
//! It combines scan findings, promotion and retirement:
//! - delete discoverable content (MNA009) and prose already promoted to checks (MNA012),
//! - move task procedures (MNA003) to `docs/agents/<topic>.md` behind a one-line pointer,
//! - promote enforceable rules to checks and delete their prose,
//! - retire rules whose retire-when condition is met,
//! - fix broken links when the file moved to a unique new place (MNA007),
//! - say when to read blind references, using the target's title (MNA006),
//! - make CLAUDE.md import AGENTS.md when they diverge (MNA010).
//!
//! The plan shows always-loaded tokens per target before and after. Nothing is written until
//! [`apply`].

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::Path;

use serde::Serialize;

use crate::budget::{self, BudgetReport};
use crate::config::Config;
use crate::diag::Finding;
use crate::discover;
use crate::doc::Doc;
use crate::promote::{self, Action};
use crate::retire;
use crate::rules::{self, Context};
use crate::tokens::Tokenizer;
use crate::workspace::{Workspace, resolve_relative};

/// What a forget action does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Delete,
    Move,
    Promote,
    Retire,
    FixReference,
    AddPointer,
    Import,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForgetAction {
    pub kind: Kind,
    pub path: String,
    pub line: usize,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetDelta {
    pub target: crate::Target,
    pub before: usize,
    pub after: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForgetPlan {
    pub actions: Vec<ForgetAction>,
    /// Always-loaded tokens per target, before and after.
    pub tokens: Vec<TargetDelta>,
    pub tokenizer: String,
    pub before: BudgetReport,
    pub after: BudgetReport,
    pub diff: String,
    #[serde(skip)]
    pub files: BTreeMap<String, String>,
    pub files_changed: Vec<String>,
}

struct Edit {
    range: Range<usize>,
    replacement: String,
    action: ForgetAction,
}

/// Byte range of whole lines covering `span`.
fn full_lines(doc: &Doc, span: &Range<usize>) -> Range<usize> {
    let start = doc.lines.line_start(doc.line_of(span.start));
    let last = span.end.saturating_sub(1).max(span.start);
    let end_line = doc.line_of(last);
    let end = if end_line < doc.lines.len() {
        doc.lines.line_start(end_line + 1)
    } else {
        doc.text.len()
    };
    start..end
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(48).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "procedure".into()
    } else {
        out
    }
}

fn title_of(text: &str) -> String {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    first
        .trim_start_matches('#')
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_string()
}

fn first_heading(doc: &Doc) -> Option<String> {
    doc.headings.first().map(|h| h.text.clone())
}

/// Build the forget plan.
pub fn plan(ws: &Workspace, config: &Config, today: &str, ast_grep: Option<&Path>) -> ForgetPlan {
    let tok = Tokenizer::best();
    let targets = config.targets.clone();
    let before = budget::budget(ws, &targets, tok);
    let cx = Context::new(ws, config, &targets);
    let only: Vec<String> = ["MNA003", "MNA006", "MNA007", "MNA009", "MNA010", "MNA012"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let findings: Vec<Finding> = rules::run_with(&cx, &rules::registry(), Some(&only));

    let mut edits: BTreeMap<String, Vec<Edit>> = BTreeMap::new();
    let mut new_files: BTreeMap<String, String> = BTreeMap::new();
    let mut whole_file: BTreeMap<String, (String, ForgetAction)> = BTreeMap::new();
    let mut add = |path: &str,
                   range: Range<usize>,
                   replacement: String,
                   kind: Kind,
                   line: usize,
                   summary: String| {
        edits.entry(path.to_string()).or_default().push(Edit {
            range,
            replacement,
            action: ForgetAction {
                kind,
                path: path.to_string(),
                line,
                summary,
            },
        });
    };

    for f in &findings {
        let Some(doc) = ws.doc(&f.path) else { continue };
        let span = f.span.clone();
        match f.rule.as_str() {
            "MNA009" => {
                let Some(span) = span else { continue };
                add(
                    &f.path,
                    full_lines(doc, &span),
                    String::new(),
                    Kind::Delete,
                    f.line,
                    "delete content the agent can discover".into(),
                );
            }
            "MNA012" => {
                let Some(span) = span else { continue };
                if let Some(rule) = doc.rules.iter().find(|r| r.span.start == span.start)
                    && let Some(r) = promote::removal_range(doc, rule)
                {
                    add(
                        &f.path,
                        r,
                        String::new(),
                        Kind::Delete,
                        f.line,
                        format!("delete promoted rule: {}", rule.text),
                    );
                }
            }
            "MNA003" => {
                let Some(span) = span else { continue };
                let range = full_lines(doc, &span);
                let body = doc.text[range.clone()].to_string();
                let title = title_of(&body);
                let mut target = format!("docs/agents/{}.md", slug(&title));
                let mut n = 2;
                while ws.discovery.has_file(&target) || new_files.contains_key(&target) {
                    target = format!("docs/agents/{}-{n}.md", slug(&title));
                    n += 1;
                }
                let content = if body.trim_start().starts_with('#') {
                    format!(
                        "# {title}\n{}",
                        body.trim_start()
                            .split_once('\n')
                            .map_or("", |(_, rest)| rest)
                    )
                } else {
                    format!("# {title}\n\n{body}")
                };
                // Keep a section heading in place and point from it; otherwise leave a pointer line.
                let heading_line = body
                    .trim_start()
                    .starts_with('#')
                    .then(|| body.trim_start().lines().next().unwrap_or("").to_string());
                let pointer = match &heading_line {
                    Some(h) => format!("{h}\n\n- Before starting, read `{target}`.\n"),
                    None => format!(
                        "- When the task involves {}, read `{target}`.\n",
                        title.to_lowercase()
                    ),
                };
                // Only move when the pointer is cheaper than what it replaces.
                if tok.count(&pointer) >= tok.count(&body) {
                    continue;
                }
                new_files.insert(target.clone(), content.trim_end().to_string() + "\n");
                add(
                    &f.path,
                    range,
                    pointer,
                    Kind::Move,
                    f.line,
                    format!("move \"{title}\" to {target}"),
                );
            }
            "MNA007" => {
                let Some(span) = span else { continue };
                let Some(link) = doc.links.iter().find(|l| l.span.start == span.start) else {
                    continue;
                };
                let dest = link.dest.split(['#', '?']).next().unwrap_or("");
                let base = dest.rsplit('/').next().unwrap_or(dest);
                if base.is_empty() {
                    continue;
                }
                let matches: Vec<&String> = ws
                    .discovery
                    .files
                    .iter()
                    .filter(|p| p.rsplit('/').next() == Some(base))
                    .collect();
                let [only] = matches.as_slice() else { continue };
                let rel = relative_path(&doc.path, only);
                let anchor = &link.dest[dest.len()..];
                let old = &doc.text[link.span.clone()];
                let new = old.replacen(&format!("({}", link.dest), &format!("({rel}{anchor}"), 1);
                if new != old {
                    add(
                        &f.path,
                        link.span.clone(),
                        new,
                        Kind::FixReference,
                        f.line,
                        format!("`{dest}` moved to `{only}`"),
                    );
                }
            }
            "MNA006" => {
                let Some(span) = span else { continue };
                let Some(rule) = doc
                    .rules
                    .iter()
                    .find(|r| r.span.start == span.start && r.is_list_item())
                else {
                    continue;
                };
                let Some(link) = doc.links.iter().find(|l| rule.span.contains(&l.span.start))
                else {
                    continue;
                };
                let dest = link.dest.split('#').next().unwrap_or("");
                let Some(target) = resolve_relative(&doc.path, dest) else {
                    continue;
                };
                let Some(heading) = ws
                    .read_file(&target)
                    .map(|t| Doc::parse(&target, &t))
                    .and_then(|d| first_heading(&d))
                else {
                    continue;
                };
                let line = format!(
                    "- Read [{}]({}) when working on {}.\n",
                    link.text,
                    link.dest,
                    heading.to_lowercase()
                );
                add(
                    &f.path,
                    full_lines(doc, &rule.span),
                    line,
                    Kind::AddPointer,
                    f.line,
                    format!("say when to read `{dest}`"),
                );
            }
            "MNA010" => {
                // CLAUDE.md becomes `@AGENTS.md` plus its own rules that AGENTS.md lacks.
                let claude = &f.path;
                let agents_path = f.snippet.strip_prefix("pair:").unwrap_or("AGENTS.md");
                let (Some(c), Some(a)) = (ws.doc(claude), ws.doc(agents_path)) else {
                    continue;
                };
                let agents_text = a.text.to_lowercase();
                let lossy = c.paragraphs.iter().any(|p| {
                    !agents_text.contains(&p.text.to_lowercase())
                        && !c
                            .rules
                            .iter()
                            .any(|r| r.span.start >= p.span.start && r.span.start < p.span.end)
                });
                if lossy {
                    continue;
                }
                let agents_rules: BTreeSet<String> =
                    a.rules.iter().map(|r| r.text.to_lowercase()).collect();
                let extra: Vec<&str> = c
                    .rules
                    .iter()
                    .filter(|r| !agents_rules.contains(&r.text.to_lowercase()))
                    .map(|r| r.text.as_str())
                    .collect();
                let import = relative_path(claude, agents_path);
                let mut content = format!("@{import}\n");
                if !extra.is_empty() {
                    content.push_str("\n## Claude Code\n\n");
                    for e in &extra {
                        content.push_str(&format!("- {e}\n"));
                    }
                }
                whole_file.insert(
                    claude.clone(),
                    (
                        content,
                        ForgetAction {
                            kind: Kind::Import,
                            path: claude.clone(),
                            line: 1,
                            summary: format!(
                                "import {agents_path} and keep {} Claude-only rule(s)",
                                extra.len()
                            ),
                        },
                    ),
                );
            }
            _ => {}
        }
    }

    // Promotions.
    let promo = promote::plan(ws, config);
    for p in &promo.promotions {
        let applied = p.artifacts.iter().all(|a| a.action != Action::Manual);
        if !(p.removes_prose && applied) {
            continue;
        }
        let Some(doc) = ws.doc(&p.source) else {
            continue;
        };
        let Some(rule) = doc
            .rules
            .iter()
            .find(|r| r.text == p.rule && doc.line_of(r.span.start) == p.line)
        else {
            continue;
        };
        if let Some(r) = promote::removal_range(doc, rule) {
            add(
                &p.source,
                r,
                String::new(),
                Kind::Promote,
                p.line,
                format!("promote to {} check `{}`", p.enforcer, p.id),
            );
        }
    }
    // The same rule may be copied into several files (AGENTS.md and CLAUDE.md); remove every copy.
    let promoted: BTreeMap<String, &promote::Promotion> = promo
        .promotions
        .iter()
        .filter(|p| p.removes_prose && p.artifacts.iter().all(|a| a.action != Action::Manual))
        .map(|p| (p.hash.clone(), p))
        .collect();
    for doc in cx.instruction_docs() {
        for rule in &doc.rules {
            let Some(p) = promoted.get(&crate::rationale::rule_hash(&rule.text)) else {
                continue;
            };
            let line = doc.line_of(rule.span.start);
            if doc.path == p.source && line == p.line {
                continue;
            }
            if let Some(r) = promote::removal_range(doc, rule) {
                add(
                    &doc.path,
                    r,
                    String::new(),
                    Kind::Promote,
                    line,
                    format!("remove copy of rule promoted to `{}`", p.id),
                );
            }
        }
    }
    let doc_paths: BTreeSet<&String> = ws.docs.keys().collect();
    for (path, content) in &promo.files {
        if !doc_paths.contains(path) {
            new_files.insert(path.clone(), content.clone());
        }
    }

    // Retirement.
    let ret = retire::plan(ws, config, today, ast_grep);
    for c in ret.retiring() {
        let Some(doc) = ws.doc(&c.source) else {
            continue;
        };
        let Some(rule) = doc
            .rules
            .iter()
            .find(|r| r.text == c.rule && doc.line_of(r.span.start) == c.line)
        else {
            continue;
        };
        if let Some(r) = promote::removal_range(doc, rule) {
            add(
                &c.source,
                r,
                String::new(),
                Kind::Retire,
                c.line,
                format!("retire ({})", c.reason),
            );
        }
    }
    for (path, content) in &ret.files {
        if !doc_paths.contains(path) {
            new_files.insert(path.clone(), content.clone());
        }
    }

    // Apply edits to docs, skipping overlaps; a whole-file rewrite wins only if nothing else edits the file.
    let mut files: BTreeMap<String, String> = new_files;
    let mut actions = Vec::new();
    for (path, (content, action)) in whole_file {
        if edits.contains_key(&path) {
            continue;
        }
        files.insert(path, content);
        actions.push(action);
    }
    for (path, mut list) in edits {
        let Some(doc) = ws.doc(&path) else { continue };
        list.sort_by_key(|e| (e.range.start, e.range.end));
        let mut out = String::new();
        let mut pos = 0;
        for e in list {
            if e.range.start < pos {
                continue;
            }
            out.push_str(&doc.text[pos..e.range.start]);
            out.push_str(&e.replacement);
            pos = e.range.end;
            actions.push(e.action);
        }
        out.push_str(&doc.text[pos..]);
        let mut out = drop_emptied_headings(doc, &out);
        // An empty CLAUDE.md would hide AGENTS.md from Claude Code; import it instead.
        let dir = path.rfind('/').map(|i| &path[..=i]).unwrap_or("");
        if path.ends_with("CLAUDE.md")
            && out.trim().is_empty()
            && ws.discovery.has_file(&format!("{dir}AGENTS.md"))
        {
            out = "@AGENTS.md\n".to_string();
        }
        files.insert(path.clone(), out);
    }
    actions
        .sort_by(|a, b| (a.path.as_str(), a.line, a.kind).cmp(&(b.path.as_str(), b.line, b.kind)));
    actions.dedup_by(|a, b| a.path == b.path && a.line == b.line && a.kind == b.kind);

    // Model the result to compute the after-budget.
    let mut all_files = ws.discovery.files.clone();
    all_files.extend(files.keys().cloned());
    let read = |p: &str| {
        files
            .get(p)
            .cloned()
            .or_else(|| ws.read_file(p))
            .unwrap_or_default()
    };
    let after = match discover::rebuild(&ws.root, all_files, &config.scan.include, &read) {
        Ok(d) => {
            let mut w2 =
                Workspace::from_discovery(d, |p| files.get(p).cloned().or_else(|| ws.read_file(p)));
            w2.mem = ws.mem.as_ref().map(|m| {
                let mut m = m.clone();
                m.extend(files.clone());
                m
            });
            budget::budget(&w2, &targets, tok)
        }
        Err(_) => before.clone(),
    };
    let tokens = before
        .targets
        .iter()
        .map(|b| TargetDelta {
            target: b.target,
            before: b.total_tokens,
            after: after
                .get(b.target)
                .map_or(b.total_tokens, |a| a.total_tokens),
        })
        .collect();

    let mut diff = String::new();
    for (path, after_text) in &files {
        let before_text = ws.read_file(path).unwrap_or_default();
        if &before_text == after_text {
            continue;
        }
        let a = if ws.discovery.has_file(path) {
            format!("a/{path}")
        } else {
            "/dev/null".to_string()
        };
        diff.push_str(&similar::udiff::unified_diff(
            similar::Algorithm::Myers,
            &before_text,
            after_text,
            3,
            Some((&a, &format!("b/{path}"))),
        ));
    }
    ForgetPlan {
        actions,
        tokens,
        tokenizer: before.tokenizer.clone(),
        before,
        after,
        diff,
        files_changed: files.keys().cloned().collect(),
        files,
    }
}

/// Remove level-2+ headings whose section had content before the edits and has none after.
fn drop_emptied_headings(before: &Doc, after: &str) -> String {
    let had_content: BTreeSet<String> = sections(before)
        .into_iter()
        .filter(|(_, body)| !body.trim().is_empty())
        .map(|(title, _)| title)
        .collect();
    let new_doc = Doc::parse(&before.path, after);
    let mut remove = Vec::new();
    for (i, h) in new_doc.headings.iter().enumerate() {
        if h.level < 2 {
            continue;
        }
        let end = new_doc.headings[i + 1..]
            .iter()
            .find(|n| n.level <= h.level)
            .map_or(after.len(), |n| n.span.start);
        let body = &after[h.span.end..end];
        let sub = new_doc.headings[i + 1..].iter().any(|n| n.span.start < end);
        if !sub && body.trim().is_empty() && had_content.contains(&h.text) {
            remove.push(h.span.start..end);
        }
    }
    if remove.is_empty() {
        return after.to_string();
    }
    let mut out = String::new();
    let mut pos = 0;
    for r in remove {
        out.push_str(&after[pos..r.start]);
        pos = r.end;
    }
    out.push_str(&after[pos..]);
    out
}

/// (heading text, section body) for every heading.
fn sections(doc: &Doc) -> Vec<(String, String)> {
    doc.headings
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let end = doc.headings[i + 1..]
                .iter()
                .find(|n| n.level <= h.level)
                .map_or(doc.text.len(), |n| n.span.start);
            (h.text.clone(), doc.text[h.span.end..end].to_string())
        })
        .collect()
}

/// Path of `to` relative to the directory of `from` (both repo-relative).
pub fn relative_path(from: &str, to: &str) -> String {
    let from_dir: Vec<&str> = match from.rfind('/') {
        Some(i) => from[..i].split('/').collect(),
        None => Vec::new(),
    };
    let to_parts: Vec<&str> = to.split('/').collect();
    let common = from_dir
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<&str> = std::iter::repeat_n("..", from_dir.len() - common).collect();
    parts.extend(&to_parts[common..]);
    parts.join("/")
}

/// Write the plan.
pub fn apply(ws: &Workspace, plan: &ForgetPlan) -> std::io::Result<Vec<String>> {
    for (path, content) in &plan.files {
        std::fs::write(ws.writable_path(path)?, content)?;
    }
    Ok(plan.files.keys().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::ws;

    fn run(files: &[(&str, &str)]) -> ForgetPlan {
        plan(&ws(files), &Config::default(), "2026-10-21", None)
    }

    const AGENTS: &str = "# Project\n\n- Use 2 spaces for indentation.\n- Read [API](docs/api.md)\n\n```\n.\n├── src/\n│   └── lib.rs\n└── Cargo.toml\n```\n\n## Adding a new endpoint\n\n1. Add the route.\n2. Add the handler.\n3. Add a test.\n\n<!-- why: freeze | retire-when: date:2026-01-01 -->\n- Keep PRs under 100 lines.\n- Never commit secrets.\n";

    #[test]
    fn combined_plan_shrinks_context() {
        let p = run(&[
            ("AGENTS.md", AGENTS),
            ("docs/api.md", "# API reference\n"),
            ("src/lib.rs", ""),
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ]);
        let kinds: BTreeSet<Kind> = p.actions.iter().map(|a| a.kind).collect();
        for k in [
            Kind::Delete,
            Kind::Move,
            Kind::Promote,
            Kind::Retire,
            Kind::AddPointer,
        ] {
            assert!(kinds.contains(&k), "missing {k:?}: {:?}", p.actions);
        }
        let agents = &p.files["AGENTS.md"];
        assert!(agents.contains("Never commit secrets"));
        assert!(!agents.contains("├──"));
        assert!(!agents.contains("2 spaces"));
        assert!(!agents.contains("PRs under 100"));
        assert!(agents.contains("## Adding a new endpoint\n\n- Before starting, read `docs/agents/adding-a-new-endpoint.md`."));
        assert!(agents.contains("- Read [API](docs/api.md) when working on api reference."));
        assert!(
            p.files["docs/agents/adding-a-new-endpoint.md"]
                .starts_with("# Adding a new endpoint\n")
        );
        assert!(p.files.contains_key(".editorconfig"));
        let claude = p
            .tokens
            .iter()
            .find(|t| t.target == crate::Target::ClaudeCode)
            .unwrap();
        assert!(claude.after < claude.before, "{claude:?}");
    }

    #[test]
    fn divergent_claude_md_imports_agents() {
        let p = run(&[
            ("AGENTS.md", "- Use pnpm.\n- Run tests.\n"),
            ("CLAUDE.md", "- Use pnpm.\n- Prefer plan mode.\n"),
        ]);
        assert_eq!(
            p.files["CLAUDE.md"],
            "@AGENTS.md\n\n## Claude Code\n\n- Prefer plan mode.\n"
        );
        assert!(p.actions.iter().any(|a| a.kind == Kind::Import));
    }

    #[test]
    fn moved_link_is_fixed() {
        let p = run(&[
            (
                "AGENTS.md",
                "- Read [guide](docs/guide.md#setup) before deploying.\n",
            ),
            ("docs/old/guide.md", "# G\n"),
        ]);
        assert_eq!(
            p.files["AGENTS.md"],
            "- Read [guide](docs/old/guide.md#setup) before deploying.\n"
        );
    }

    #[test]
    fn clean_repo_has_empty_plan() {
        let p = run(&[
            (
                "AGENTS.md",
                "- Read [x](docs/x.md) before changing the API.\n",
            ),
            ("docs/x.md", "# X\n"),
        ]);
        assert!(p.actions.is_empty() && p.files.is_empty() && p.diff.is_empty());
    }

    #[test]
    fn emptied_sections_lose_their_heading_and_copies_are_removed() {
        let p = run(&[
            (
                "AGENTS.md",
                "# P\n\n- Use 2 spaces for indentation.\n\n## Stack\n\n- TypeScript\n- React\n- Vitest\n\n## Rules\n\n- Keep PRs small.\n",
            ),
            (
                "CLAUDE.md",
                "@AGENTS.md\n\n- Use 2 spaces for indentation.\n- Prefer plan mode.\n",
            ),
            (
                "package.json",
                r#"{"devDependencies":{"typescript":"5","react":"18","vitest":"1"}}"#,
            ),
            ("tsconfig.json", "{}"),
        ]);
        let a = &p.files["AGENTS.md"];
        assert!(!a.contains("## Stack"), "{a}");
        assert!(a.contains("## Rules") && a.contains("Keep PRs small"));
        assert_eq!(p.files["CLAUDE.md"], "@AGENTS.md\n\n- Prefer plan mode.\n");
    }

    #[test]
    fn tiny_procedures_are_not_moved() {
        let p = run(&[("AGENTS.md", "## Releasing\n\n1. a\n2. b\n3. c\n")]);
        assert!(p.actions.is_empty(), "{:?}", p.actions);
    }

    #[test]
    fn relative_paths() {
        assert_eq!(relative_path("AGENTS.md", "docs/a.md"), "docs/a.md");
        assert_eq!(relative_path("pkg/CLAUDE.md", "pkg/AGENTS.md"), "AGENTS.md");
        assert_eq!(relative_path("a/b/X.md", "a/c/Y.md"), "../c/Y.md");
    }

    #[test]
    fn apply_writes_new_and_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "## Releasing\n\n1. Bump the version in Cargo.toml and update the changelog with every user-facing change.\n2. Tag the release commit with the new version and push the tag to origin.\n3. Wait for the release workflow, then check the published artifacts and the docs site.\n",
        )
        .unwrap();
        let w = Workspace::load(dir.path(), &Default::default()).unwrap();
        let p = plan(&w, &Config::default(), "2026-10-21", None);
        apply(&w, &p).unwrap();
        assert!(dir.path().join("docs/agents/releasing.md").is_file());
        let a = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert_eq!(
            a,
            "## Releasing\n\n- Before starting, read `docs/agents/releasing.md`.\n"
        );
    }
}
