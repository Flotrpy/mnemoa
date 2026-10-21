//! MNA006 blind-reference and MNA007 broken-reference.

use std::sync::LazyLock;

use regex::Regex;

use super::{Check, Context, RULES, RuleInfo, finding_at};
use crate::diag::{Finding, Severity};
use crate::doc::{Doc, RuleKind};
use crate::workspace::resolve_relative;

/// Words that say nothing about when or why to follow a reference.
const FILLER: &[&str] = &[
    "see",
    "read",
    "also",
    "more",
    "info",
    "information",
    "details",
    "detail",
    "docs",
    "doc",
    "documentation",
    "the",
    "a",
    "an",
    "for",
    "and",
    "or",
    "here",
    "link",
    "links",
    "reference",
    "references",
    "check",
    "out",
    "refer",
    "to",
    "at",
    "in",
    "file",
    "files",
    "our",
    "this",
    "that",
    "please",
    "full",
    "further",
    "other",
    "related",
    "useful",
    "resources",
    "guide",
    "guides",
    "notes",
    "readme",
    "of",
    "on",
    "is",
    "are",
    "available",
];

/// Words that signal a condition or purpose.
const CONTEXT_WORDS: &[&str] = &[
    "when", "before", "after", "if", "while", "whenever", "during", "for",
];

static WORD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z][A-Za-z0-9'’-]*").unwrap());
static PATHLIKE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\./|\.\./)*[A-Za-z0-9_@.\-]+(?:/[A-Za-z0-9_.@\-]+)+/?$").unwrap()
});
static SCHEME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.\-]*:").unwrap());

fn content_words(text: &str) -> Vec<String> {
    WORD_RE
        .find_iter(text)
        .map(|m| m.as_str().to_lowercase())
        .filter(|w| !FILLER.contains(&w.as_str()))
        .collect()
}

/// Whether a code span looks like a repository path (not a glob, URL or placeholder).
pub(crate) fn looks_like_path(code: &str) -> bool {
    !code.contains([
        '*', '?', '{', '}', '<', '>', '$', ' ', '[', ']', '|', '\\', '=',
    ]) && !code.starts_with('-')
        && !code.starts_with("//")
        && !code.contains("::")
        && !SCHEME_RE.is_match(code)
        && PATHLIKE_RE.is_match(code)
}

/// Whether the last path segment has a file extension or the path ends with `/`.
pub(crate) fn has_extension_or_slash(path: &str) -> bool {
    if path.ends_with('/') {
        return true;
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    match last.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty() && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// MNA006.
pub struct BlindReference;

impl Check for BlindReference {
    fn info(&self) -> &'static RuleInfo {
        &RULES[5]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        for doc in cx.instruction_docs() {
            check_blind(self.info(), doc, out);
        }
    }
}

fn check_blind(info: &RuleInfo, doc: &Doc, out: &mut Vec<Finding>) {
    // Candidate blocks: list items, and paragraphs outside lists.
    let mut blocks: Vec<(std::ops::Range<usize>, String, Option<usize>)> = doc
        .rules
        .iter()
        .filter(|r| matches!(r.kind, RuleKind::ListItem { .. }))
        .map(|r| (r.span.clone(), r.text.clone(), r.section))
        .collect();
    for p in &doc.paragraphs {
        let in_list = doc.lists.iter().any(|l| l.span.contains(&p.span.start));
        if !in_list {
            blocks.push((p.span.clone(), p.text.clone(), p.section));
        }
    }

    for (span, text, section) in blocks {
        let links: Vec<_> = doc
            .links
            .iter()
            .filter(|l| !l.image && span.contains(&l.span.start))
            .collect();
        let paths: Vec<_> = doc
            .code_spans
            .iter()
            .filter(|c| span.contains(&c.span.start) && looks_like_path(&c.code))
            .collect();
        if links.is_empty() && paths.is_empty() {
            continue;
        }
        // Remove link texts and path spans, then see what is left.
        let mut rest = text.clone();
        let mut link_words = 0;
        for l in &links {
            link_words += content_words(&l.text).len();
            rest = rest.replacen(&l.text, " ", 1);
        }
        for p in &paths {
            rest = rest.replacen(&format!("`{}`", p.code), " ", 1);
        }
        let rest_words = content_words(&rest);
        if !rest_words.is_empty() || link_words >= 3 {
            continue;
        }
        let heading = doc.section_title(section).unwrap_or("").to_lowercase();
        if heading
            .split_whitespace()
            .any(|w| CONTEXT_WORDS.contains(&w))
        {
            continue;
        }
        let target = links
            .first()
            .map(|l| l.dest.clone())
            .or_else(|| paths.first().map(|p| p.code.clone()))
            .unwrap_or_default();
        out.push(
            finding_at(
                info,
                doc,
                span.clone(),
                format!("Reference to `{target}` says nothing about when or why to read it."),
            )
            .with_help(format!(
                "Say when it matters, for example: \"Read `{target}` before changing ...\"."
            )),
        );
    }
}

/// MNA007.
pub struct BrokenReference;

impl Check for BrokenReference {
    fn info(&self) -> &'static RuleInfo {
        &RULES[6]
    }

    fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        for doc in cx.instruction_docs() {
            check_broken(self.info(), cx, doc, out);
        }
    }
}

const SPECULATIVE: &[&str] = &[
    "create",
    "creates",
    "new ",
    "generate",
    "generated",
    "e.g.",
    "for example",
    "such as",
    "will be",
    "would",
    "output",
    "build/",
    "named like",
    "placeholder",
];

fn check_broken(info: &RuleInfo, cx: &Context<'_>, doc: &Doc, out: &mut Vec<Finding>) {
    let disc = &cx.ws.discovery;
    let exists_from = |base: &str, target: &str| -> Option<bool> {
        let resolved = if let Some(abs) = target.strip_prefix('/') {
            Some(abs.trim_end_matches('/').to_string())
        } else {
            resolve_relative(base, target.trim_end_matches('/'))
        }?;
        Some(disc.exists(&resolved))
    };

    // Links and images.
    for l in &doc.links {
        let dest = l.dest.trim();
        if dest.is_empty()
            || dest.starts_with('#')
            || SCHEME_RE.is_match(dest)
            || dest.contains(['<', '>', '{', '}', '$', '*'])
            || dest.starts_with("//")
        {
            continue;
        }
        let path = dest.split(['#', '?']).next().unwrap_or("");
        if path.is_empty() {
            continue;
        }
        let path = percent_decode(path);
        if exists_from(&doc.path, &path) == Some(false) {
            out.push(
                finding_at(
                    info,
                    doc,
                    l.span.clone(),
                    format!("Link target `{path}` does not exist."),
                )
                .with_help("Fix the path or remove the link."),
            );
        }
    }

    // Claude Code @imports.
    for imp in &doc.imports {
        if imp.path.starts_with('~') || imp.path.starts_with('/') {
            let mut f = finding_at(
                info,
                doc,
                imp.span.clone(),
                format!(
                    "Import `@{}` points outside the repository, so it only works on machines that have that file.",
                    imp.path
                ),
            );
            f.severity = Severity::Info;
            out.push(f);
            continue;
        }
        if !has_extension_or_slash(&imp.path) {
            continue;
        }
        if exists_from(&doc.path, &imp.path) == Some(false) {
            out.push(
                finding_at(
                    info,
                    doc,
                    imp.span.clone(),
                    format!("Import `@{}` does not resolve.", imp.path),
                )
                .with_help("Claude Code resolves imports relative to the importing file."),
            );
        }
    }

    // Inline-code paths whose top-level directory exists (so the path probably moved).
    for c in &doc.code_spans {
        let code = c.code.trim();
        if !looks_like_path(code) || !has_extension_or_slash(code) {
            continue;
        }
        let line_start = doc.lines.line_start(doc.line_of(c.span.start));
        let line_end = doc.text[c.span.start..]
            .find('\n')
            .map_or(doc.text.len(), |i| c.span.start + i);
        let line = doc.text[line_start..line_end].to_lowercase();
        if SPECULATIVE.iter().any(|w| line.contains(w)) {
            continue;
        }
        let stripped = code.trim_start_matches("./");
        let first = stripped.split('/').next().unwrap_or("");
        let doc_dir = doc.path.rfind('/').map(|i| &doc.path[..i]).unwrap_or("");
        let first_in_doc_dir = if doc_dir.is_empty() {
            first.to_string()
        } else {
            format!("{doc_dir}/{first}")
        };
        if first.is_empty()
            || first == ".."
            || !(disc.has_dir(first) || disc.has_dir(&first_in_doc_dir))
        {
            continue;
        }
        let at_root = disc.exists(stripped.trim_end_matches('/'));
        let near_doc = exists_from(&doc.path, code) == Some(true);
        if !at_root && !near_doc {
            out.push(
                finding_at(
                    info,
                    doc,
                    c.span.clone(),
                    format!("Path `{code}` does not exist, though `{first}/` does."),
                )
                .with_help("The file may have moved. Update the path or remove the line."),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::testutil::check;

    // ---- MNA006 ----

    #[test]
    fn blind_bare_link_is_flagged() {
        let f = check(
            &BlindReference,
            &[
                ("AGENTS.md", "# Docs\n\n- [API docs](docs/api.md)\n"),
                ("docs/api.md", "x"),
            ],
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "MNA006");
        assert_eq!(f[0].line, 3);
    }

    #[test]
    fn blind_bare_path_paragraph_is_flagged() {
        let f = check(
            &BlindReference,
            &[
                ("AGENTS.md", "See `docs/arch.md`.\n"),
                ("docs/arch.md", "x"),
            ],
        );
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn link_with_when_clause_is_not_blind() {
        let f = check(
            &BlindReference,
            &[
                (
                    "AGENTS.md",
                    "- Read [API docs](docs/api.md) before changing handlers.\n",
                ),
                ("docs/api.md", "x"),
            ],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn descriptive_link_text_is_not_blind() {
        let f = check(
            &BlindReference,
            &[
                (
                    "AGENTS.md",
                    "- [How we version the public API](docs/v.md)\n",
                ),
                ("docs/v.md", "x"),
            ],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn heading_with_context_is_not_blind() {
        let f = check(
            &BlindReference,
            &[
                (
                    "AGENTS.md",
                    "## Read before releasing\n\n- [Checklist](docs/release.md)\n",
                ),
                ("docs/release.md", "x"),
            ],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn links_in_code_blocks_are_ignored() {
        let f = check(
            &BlindReference,
            &[("AGENTS.md", "```md\n- [API](docs/api.md)\n```\n")],
        );
        assert!(f.is_empty());
    }

    // ---- MNA007 ----

    #[test]
    fn broken_link_is_an_error() {
        let f = check(
            &BrokenReference,
            &[
                ("AGENTS.md", "Read [the guide](docs/missing.md) before X.\n"),
                ("docs/a.md", ""),
            ],
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error);
        assert!(f[0].message.contains("docs/missing.md"));
    }

    #[test]
    fn existing_link_with_anchor_and_urls_pass() {
        let f = check(
            &BrokenReference,
            &[
                (
                    "AGENTS.md",
                    "See [x](docs/a.md#setup), [web](https://example.com), [top](#top) and [mail](mailto:a@b.c).\n",
                ),
                ("docs/a.md", ""),
            ],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn link_relative_to_nested_doc() {
        let f = check(
            &BrokenReference,
            &[("pkg/AGENTS.md", "[up](../README.md)\n"), ("README.md", "")],
        );
        assert!(f.is_empty());
    }

    #[test]
    fn broken_import_is_flagged_but_mentions_are_not() {
        let f = check(
            &BrokenReference,
            &[
                (
                    "CLAUDE.md",
                    "@AGENTS.md\n@docs/gone.md\nPing @alice or use @types/node.\n",
                ),
                ("AGENTS.md", ""),
            ],
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("docs/gone.md"));
    }

    #[test]
    fn home_import_is_info() {
        let f = check(&BrokenReference, &[("CLAUDE.md", "@~/.claude/mine.md\n")]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Info);
    }

    #[test]
    fn moved_code_path_is_flagged() {
        let f = check(
            &BrokenReference,
            &[
                ("AGENTS.md", "- Handlers live in `src/api/handlers.rs`.\n"),
                ("src/api/routes.rs", ""),
            ],
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("src/api/handlers.rs"));
    }

    #[test]
    fn code_paths_near_misses_are_silent() {
        let f = check(
            &BrokenReference,
            &[
                (
                    "AGENTS.md",
                    "- Globs like `src/**/*.rs` are fine.\n- `target/debug/app` is build output.\n- Create `src/new_mod.rs` for new modules.\n- `src/lib.rs` exists.\n- Use `crate::x` paths.\n",
                ),
                ("src/lib.rs", ""),
            ],
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn helpers() {
        assert!(looks_like_path("src/main.rs"));
        assert!(looks_like_path("./scripts/x.sh"));
        assert!(!looks_like_path("src/**/*.rs"));
        assert!(!looks_like_path("https://x.y/z"));
        assert!(!looks_like_path("main.rs"));
        assert!(has_extension_or_slash("a/b.md") && has_extension_or_slash("a/b/"));
        assert!(!has_extension_or_slash("a/b"));
        assert_eq!(percent_decode("Design%20Docs/a.md"), "Design Docs/a.md");
    }
}
