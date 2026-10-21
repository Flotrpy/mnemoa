//! Markdown document model with exact source offsets.
//!
//! [`Doc::parse`] extracts headings, rules (list items and imperative sentences), code spans,
//! fenced blocks, links, `@imports`, HTML comments, lists and front matter. Every element keeps
//! its byte range in the original text so diagnostics can point at it.

use std::ops::Range;
use std::sync::LazyLock;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use serde::Serialize;

use crate::frontmatter::{self, FrontMatter};

/// A byte range in the document text.
pub type Span = Range<usize>;

/// Maps byte offsets to 1-based line and column numbers.
#[derive(Debug, Clone, Default)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { starts }
    }

    /// 1-based line for a byte offset.
    pub fn line(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }

    /// 1-based (line, column) for a byte offset. Columns count characters.
    pub fn line_col(&self, text: &str, offset: usize) -> (usize, usize) {
        let line = self.line(offset);
        let start = self.starts[line - 1];
        let offset = offset.min(text.len());
        let col = text
            .get(start..offset)
            .map(|s| s.chars().count())
            .unwrap_or(0)
            + 1;
        (line, col)
    }

    /// Byte offset where a 1-based line starts.
    pub fn line_start(&self, line: usize) -> usize {
        self.starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(0)
    }

    /// Number of lines.
    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub span: Span,
}

/// Whether a rule tells the agent to do something or not to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Polarity {
    Do,
    Dont,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleKind {
    /// A list item.
    ListItem { ordered: bool, depth: usize },
    /// An imperative sentence in a paragraph.
    Sentence,
}

/// An instruction: a list item, or an imperative sentence in a paragraph.
#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    /// Plain text. Inline code keeps its backticks.
    pub text: String,
    pub span: Span,
    pub kind: RuleKind,
    /// Index into [`Doc::headings`] of the enclosing section.
    pub section: Option<usize>,
    /// Whether the text reads as an instruction (always, never, must, use ...).
    pub imperative: bool,
    pub polarity: Polarity,
}

impl Rule {
    pub fn is_list_item(&self) -> bool {
        matches!(self.kind, RuleKind::ListItem { .. })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeSpan {
    pub code: String,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeBlock {
    /// Info string language, lowercased; empty when absent or indented.
    pub lang: String,
    pub code: String,
    pub span: Span,
    pub fenced: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Link {
    pub dest: String,
    pub text: String,
    pub span: Span,
    pub image: bool,
}

/// A Claude Code style `@path` import.
#[derive(Debug, Clone, Serialize)]
pub struct Import {
    pub path: String,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct HtmlComment {
    /// Text between `<!--` and `-->`, trimmed.
    pub body: String,
    pub span: Span,
    /// The comment starts at the beginning of a line (a block-level comment).
    pub block: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListBlock {
    pub ordered: bool,
    pub depth: usize,
    pub span: Span,
    /// Spans of the direct items.
    pub items: Vec<Span>,
    pub section: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Paragraph {
    pub text: String,
    pub span: Span,
    pub section: Option<usize>,
}

/// A parsed Markdown document.
#[derive(Debug, Clone)]
pub struct Doc {
    pub path: String,
    pub text: String,
    pub front_matter: Option<FrontMatter>,
    pub lines: LineIndex,
    pub headings: Vec<Heading>,
    pub rules: Vec<Rule>,
    pub code_spans: Vec<CodeSpan>,
    pub code_blocks: Vec<CodeBlock>,
    pub links: Vec<Link>,
    pub imports: Vec<Import>,
    pub comments: Vec<HtmlComment>,
    pub lists: Vec<ListBlock>,
    pub paragraphs: Vec<Paragraph>,
}

static COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--(.*?)-->").unwrap());
static IMPORT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[\s(\[])@((?:\\ |[A-Za-z0-9_./~\-+])+)").unwrap());
static IMPERATIVE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)^(?:\*\*)?(?:you\s+|please\s+|agents?\s+|claude\s+|always\s+|never\s+|only\s+)*
        (?:always|never|must|should|shall|do|don['’]t|avoid|prefer|use|run|ensure|make\s+sure|keep|write|add|put|place|follow|only|stop|check|call|import|name|wrap|return|commit|update|test|document|remove|delete|create|install|start|ask|read|see|refer|consult|leave|treat|mark|let|instead)\b",
    )
    .unwrap()
});
static NEGATIVE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:never|don['’]t|do\s+not|must\s+not|mustn['’]t|should\s+not|shouldn['’]t|avoid|no\s+longer|not\s+allowed|forbidden|prohibited)\b").unwrap()
});

impl Doc {
    /// Parse `text` as the document at relative path `path`.
    pub fn parse(path: &str, text: &str) -> Doc {
        let front_matter = frontmatter::split(text);
        let body_start = front_matter.as_ref().map(|f| f.body_start).unwrap_or(0);
        let mut doc = Doc {
            path: path.to_string(),
            text: text.to_string(),
            front_matter,
            lines: LineIndex::new(text),
            headings: Vec::new(),
            rules: Vec::new(),
            code_spans: Vec::new(),
            code_blocks: Vec::new(),
            links: Vec::new(),
            imports: Vec::new(),
            comments: Vec::new(),
            lists: Vec::new(),
            paragraphs: Vec::new(),
        };
        doc.walk(body_start);
        doc.scan_comments_and_imports(body_start);
        doc
    }

    fn walk(&mut self, base: usize) {
        let source = self.text.clone();
        let body = &source[base..];
        let opts =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        let parser = Parser::new_ext(body, opts).into_offset_iter();

        // Stacks for nested structures.
        struct ItemCtx {
            start: usize,
            text: String,
            ordered: bool,
            depth: usize,
            nested: bool,
        }
        let mut list_stack: Vec<ListBlock> = Vec::new();
        let mut item_stack: Vec<ItemCtx> = Vec::new();
        let mut heading: Option<(u8, usize, String)> = None;
        let mut para: Option<(usize, String)> = None;
        let mut code_block: Option<(String, usize, String, bool)> = None;
        let mut link_stack: Vec<(String, usize, String, bool)> = Vec::new();
        let mut section: Option<usize> = None;
        let mut in_table = 0usize;

        let push_text =
            |s: &str,
             heading: &mut Option<(u8, usize, String)>,
             para: &mut Option<(usize, String)>,
             item_stack: &mut Vec<ItemCtx>,
             link_stack: &mut Vec<(String, usize, String, bool)>| {
                if let Some((_, _, t)) = heading.as_mut() {
                    t.push_str(s);
                }
                if let Some((_, t)) = para.as_mut() {
                    t.push_str(s);
                }
                if let Some(item) = item_stack.last_mut()
                    && !item.nested
                {
                    item.text.push_str(s);
                }
                if let Some((_, _, t, _)) = link_stack.last_mut() {
                    t.push_str(s);
                }
            };

        for (event, range) in parser {
            let span = (range.start + base)..(range.end + base);
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    heading = Some((level as u8, span.start, String::new()));
                }
                Event::End(TagEnd::Heading(_)) => {
                    if let Some((level, start, text)) = heading.take() {
                        self.headings.push(Heading {
                            level,
                            text: text.trim().to_string(),
                            span: start..span.end,
                        });
                        section = Some(self.headings.len() - 1);
                    }
                }
                Event::Start(Tag::Paragraph) => {
                    if in_table == 0 {
                        para = Some((span.start, String::new()));
                    }
                }
                Event::End(TagEnd::Paragraph) => {
                    if let Some((start, text)) = para.take() {
                        let in_item = !item_stack.is_empty();
                        let text = normalize_ws(&text);
                        if !in_item {
                            self.add_sentences(&text, start..span.end, section);
                        }
                        if !text.is_empty() {
                            self.paragraphs.push(Paragraph {
                                text,
                                span: start..span.end,
                                section,
                            });
                        }
                        if let Some(item) = item_stack.last_mut()
                            && !item.nested
                        {
                            item.text.push(' ');
                        }
                    }
                }
                Event::Start(Tag::List(first)) => {
                    if let Some(item) = item_stack.last_mut() {
                        item.nested = true;
                    }
                    list_stack.push(ListBlock {
                        ordered: first.is_some(),
                        depth: list_stack.len(),
                        span: span.clone(),
                        items: Vec::new(),
                        section,
                    });
                }
                Event::End(TagEnd::List(_)) => {
                    if let Some(mut l) = list_stack.pop() {
                        l.span = l.span.start..span.end;
                        self.lists.push(l);
                    }
                    if let Some(item) = item_stack.last_mut() {
                        item.nested = false;
                    }
                }
                Event::Start(Tag::Item) => {
                    let (ordered, depth) = list_stack
                        .last()
                        .map(|l| (l.ordered, l.depth))
                        .unwrap_or((false, 0));
                    item_stack.push(ItemCtx {
                        start: span.start,
                        text: String::new(),
                        ordered,
                        depth,
                        nested: false,
                    });
                }
                Event::End(TagEnd::Item) => {
                    if let Some(item) = item_stack.pop() {
                        // The item's own text ends where its first nested list starts.
                        let end = trim_end_offset(&source, item.start, span.end);
                        if let Some(l) = list_stack.last_mut() {
                            l.items.push(item.start..span.end);
                        }
                        let text = normalize_ws(&item.text);
                        if !text.is_empty() {
                            let polarity = polarity(&text);
                            self.rules.push(Rule {
                                imperative: is_imperative(&text),
                                polarity,
                                text,
                                span: item.start..end,
                                kind: RuleKind::ListItem {
                                    ordered: item.ordered,
                                    depth: item.depth,
                                },
                                section,
                            });
                        }
                    }
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    let (lang, fenced) = match kind {
                        CodeBlockKind::Fenced(info) => (
                            info.split_whitespace()
                                .next()
                                .unwrap_or("")
                                .trim_start_matches('{')
                                .trim_end_matches('}')
                                .to_ascii_lowercase(),
                            true,
                        ),
                        CodeBlockKind::Indented => (String::new(), false),
                    };
                    code_block = Some((lang, span.start, String::new(), fenced));
                }
                Event::End(TagEnd::CodeBlock) => {
                    if let Some((lang, start, code, fenced)) = code_block.take() {
                        self.code_blocks.push(CodeBlock {
                            lang,
                            code,
                            span: start..span.end,
                            fenced,
                        });
                    }
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    link_stack.push((dest_url.to_string(), span.start, String::new(), false));
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    link_stack.push((dest_url.to_string(), span.start, String::new(), true));
                }
                Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => {
                    if let Some((dest, start, text, image)) = link_stack.pop() {
                        self.links.push(Link {
                            dest,
                            text: normalize_ws(&text),
                            span: start..span.end,
                            image,
                        });
                        if let Some((_, _, t, _)) = link_stack.last_mut() {
                            t.push_str(&text);
                        }
                    }
                }
                Event::Start(Tag::Table(_)) => in_table += 1,
                Event::End(TagEnd::Table) => in_table = in_table.saturating_sub(1),
                Event::Text(t) => {
                    if let Some((_, _, code, _)) = code_block.as_mut() {
                        code.push_str(&t);
                    } else {
                        push_text(
                            &t,
                            &mut heading,
                            &mut para,
                            &mut item_stack,
                            &mut link_stack,
                        );
                    }
                }
                Event::Code(c) => {
                    self.code_spans.push(CodeSpan {
                        code: c.to_string(),
                        span: span.clone(),
                    });
                    let quoted = format!("`{c}`");
                    push_text(
                        &quoted,
                        &mut heading,
                        &mut para,
                        &mut item_stack,
                        &mut link_stack,
                    );
                }
                Event::SoftBreak | Event::HardBreak => {
                    push_text(
                        " ",
                        &mut heading,
                        &mut para,
                        &mut item_stack,
                        &mut link_stack,
                    );
                }
                _ => {}
            }
        }
    }

    fn add_sentences(&mut self, text: &str, span: Span, section: Option<usize>) {
        for sentence in split_sentences(text) {
            if is_imperative(sentence) {
                self.rules.push(Rule {
                    text: sentence.to_string(),
                    span: span.clone(),
                    kind: RuleKind::Sentence,
                    section,
                    imperative: true,
                    polarity: polarity(sentence),
                });
            }
        }
    }

    fn scan_comments_and_imports(&mut self, base: usize) {
        let mut code: Vec<Span> = self.code_blocks.iter().map(|b| b.span.clone()).collect();
        code.extend(self.code_spans.iter().map(|c| c.span.clone()));
        let in_code = |pos: usize| code.iter().any(|r| r.contains(&pos));
        let body = &self.text[base..];

        let mut comment_spans = Vec::new();
        for m in COMMENT_RE.captures_iter(body) {
            let whole = m.get(0).unwrap();
            let start = whole.start() + base;
            if in_code(start) {
                continue;
            }
            let line_start = self.text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let block = self.text[line_start..start].trim().is_empty();
            self.comments.push(HtmlComment {
                body: m[1].trim().to_string(),
                span: start..whole.end() + base,
                block,
            });
            comment_spans.push(start..whole.end() + base);
        }

        for m in IMPORT_RE.captures_iter(body) {
            let g = m.get(1).unwrap();
            let start = g.start() - 1 + base; // include the `@`
            if in_code(start) || comment_spans.iter().any(|r| r.contains(&start)) {
                continue;
            }
            let raw = g.as_str().trim_end_matches(['.', ',', ';', ':', ')']);
            if raw.is_empty()
                || !raw
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || "._~/".contains(c))
            {
                continue;
            }
            self.imports.push(Import {
                path: raw.replace("\\ ", " "),
                span: start..start + 1 + raw.len(),
            });
        }
    }

    /// (line, column) for a byte offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        self.lines.line_col(&self.text, offset)
    }

    /// 1-based line of a byte offset.
    pub fn line_of(&self, offset: usize) -> usize {
        self.lines.line(offset)
    }

    /// Number of lines in the document.
    pub fn line_count(&self) -> usize {
        let n = self.lines.len();
        if self.text.ends_with('\n') { n - 1 } else { n }
    }

    /// Whether a byte offset is inside a fenced or indented code block.
    pub fn in_code_block(&self, offset: usize) -> bool {
        self.code_blocks.iter().any(|b| b.span.contains(&offset))
    }

    /// The text a target actually sees: front matter removed, and block-level HTML comments
    /// removed when `strip_comments` is set.
    pub fn loaded_text(&self, strip_comments: bool) -> String {
        let start = self
            .front_matter
            .as_ref()
            .map(|f| f.body_start)
            .unwrap_or(0);
        if !strip_comments {
            return self.text[start..].to_string();
        }
        let mut out = String::with_capacity(self.text.len());
        let mut pos = start;
        for c in self.comments.iter().filter(|c| c.block) {
            if c.span.start >= pos {
                out.push_str(&self.text[pos..c.span.start]);
                pos = c.span.end;
            }
        }
        out.push_str(&self.text[pos..]);
        out
    }

    /// The heading text of a section index.
    pub fn section_title(&self, section: Option<usize>) -> Option<&str> {
        section
            .and_then(|i| self.headings.get(i))
            .map(|h| h.text.as_str())
    }

    /// The block-level HTML comment that ends on the line directly above `offset`, if any.
    pub fn comment_above(&self, offset: usize) -> Option<&HtmlComment> {
        let line = self.line_of(offset);
        if line < 2 {
            return None;
        }
        self.comments
            .iter()
            .find(|c| c.block && self.line_of(c.span.end.saturating_sub(1)) == line - 1)
    }
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trim_end_offset(text: &str, start: usize, end: usize) -> usize {
    let slice = &text[start..end];
    // Stop at the first line of a nested list item (an indented list marker).
    let mut offset = 0;
    for (i, line) in slice.split_inclusive('\n').enumerate() {
        if i > 0 {
            let t = line.trim_start();
            let indented = line.len() > t.len();
            let marker = t.starts_with("- ")
                || t.starts_with("* ")
                || t.starts_with("+ ")
                || t.split_once(". ")
                    .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
            if indented && marker {
                break;
            }
        }
        offset += line.len();
    }
    let s = &slice[..offset];
    start + s.trim_end().len()
}

/// Split a paragraph into sentences on `.`, `!` or `?` followed by whitespace,
/// ignoring dots inside inline code.
pub fn split_sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_code = false;
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'`' {
            in_code = !in_code;
        }
        if !in_code
            && matches!(b, b'.' | b'!' | b'?')
            && bytes.get(i + 1).is_some_and(|n| n.is_ascii_whitespace())
        {
            let s = text[start..=i].trim();
            if !s.is_empty() {
                out.push(s);
            }
            start = i + 1;
        }
    }
    let s = text[start..].trim();
    if !s.is_empty() {
        out.push(s);
    }
    out
}

/// Whether a sentence or list item reads as an instruction.
pub fn is_imperative(text: &str) -> bool {
    let t = text
        .trim_start_matches(|c: char| !c.is_alphanumeric() && c != '`' && c != '*')
        .trim_start_matches("[ ] ")
        .trim_start_matches("[x] ");
    // Strip a leading bold label such as "**Tests:** ".
    let t = match t.strip_prefix("**").and_then(|r| r.split_once("**")) {
        Some((label, rest)) if label.len() < 40 => rest.trim_start_matches([':', ' ']),
        _ => t,
    };
    IMPERATIVE_RE.is_match(t) || NEGATIVE_RE.is_match(t) || t.contains(" must ")
}

/// Whether an instruction is phrased as a prohibition.
pub fn polarity(text: &str) -> Polarity {
    if NEGATIVE_RE.is_match(text) {
        Polarity::Dont
    } else {
        Polarity::Do
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\npaths:\n  - src/**\n---\n# Title\n\nAlways run `make lint` before committing. The project is old.\n\n## Rules\n\n<!-- why: CI broke | retire-when: manual -->\n- Never call `.unwrap()` in `src/`.\n- See [the API docs](docs/api.md) when changing handlers.\n  - nested detail\n\n1. First\n2. Second\n\n```sh\nmake test @not/an/import.md\n```\n\nImport @docs/extra.md here, mail me at a@b.com.\n";

    #[test]
    fn front_matter_is_split_and_offsets_are_absolute() {
        let d = Doc::parse("x.md", SAMPLE);
        assert!(d.front_matter.is_some());
        let h = &d.headings[0];
        assert_eq!(h.text, "Title");
        assert_eq!(&SAMPLE[h.span.clone()].trim(), &"# Title");
        assert_eq!(d.line_of(h.span.start), 5);
    }

    #[test]
    fn rules_from_lists_and_sentences() {
        let d = Doc::parse("x.md", SAMPLE);
        let texts: Vec<&str> = d.rules.iter().map(|r| r.text.as_str()).collect();
        assert!(texts.contains(&"Always run `make lint` before committing."));
        assert!(!texts.iter().any(|t| t.contains("The project is old")));
        let never = d
            .rules
            .iter()
            .find(|r| r.text.starts_with("Never"))
            .unwrap();
        assert_eq!(never.polarity, Polarity::Dont);
        assert_eq!(d.section_title(never.section), Some("Rules"));
        assert!(never.is_list_item());
        // The parent item text does not include the nested item.
        let see = d.rules.iter().find(|r| r.text.starts_with("See")).unwrap();
        assert!(!see.text.contains("nested"));
        assert!(!SAMPLE[see.span.clone()].contains("nested"));
    }

    #[test]
    fn code_spans_blocks_links() {
        let d = Doc::parse("x.md", SAMPLE);
        assert!(d.code_spans.iter().any(|c| c.code == ".unwrap()"));
        let b = &d.code_blocks[0];
        assert_eq!(b.lang, "sh");
        assert!(b.code.contains("make test"));
        assert_eq!(d.links[0].dest, "docs/api.md");
        assert_eq!(d.links[0].text, "the API docs");
        assert_eq!(
            &SAMPLE[d.links[0].span.clone()],
            "[the API docs](docs/api.md)"
        );
    }

    #[test]
    fn imports_skip_code_and_emails() {
        let d = Doc::parse("x.md", SAMPLE);
        let paths: Vec<&str> = d.imports.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, vec!["docs/extra.md"]);
        assert_eq!(&SAMPLE[d.imports[0].span.clone()], "@docs/extra.md");
    }

    #[test]
    fn comments_and_rationale_lookup() {
        let d = Doc::parse("x.md", SAMPLE);
        assert_eq!(d.comments.len(), 1);
        assert!(d.comments[0].block);
        let never = d
            .rules
            .iter()
            .find(|r| r.text.starts_with("Never"))
            .unwrap();
        let c = d.comment_above(never.span.start).unwrap();
        assert!(c.body.starts_with("why:"));
        let see = d.rules.iter().find(|r| r.text.starts_with("See")).unwrap();
        assert!(d.comment_above(see.span.start).is_none());
    }

    #[test]
    fn loaded_text_strips_comments_and_front_matter() {
        let d = Doc::parse("x.md", SAMPLE);
        let t = d.loaded_text(true);
        assert!(!t.contains("why: CI broke"));
        assert!(!t.contains("paths:"));
        assert!(d.loaded_text(false).contains("why: CI broke"));
    }

    #[test]
    fn lists_are_recorded() {
        let d = Doc::parse("x.md", SAMPLE);
        let ordered = d.lists.iter().find(|l| l.ordered).unwrap();
        assert_eq!(ordered.items.len(), 2);
        assert!(d.lists.iter().any(|l| l.depth == 1));
    }

    #[test]
    fn line_col_counts_characters() {
        let text = "é\nab";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_col(text, 0), (1, 1));
        assert_eq!(idx.line_col(text, 4), (2, 2));
    }

    #[test]
    fn sentence_split_ignores_code_dots() {
        let s = split_sentences("Run `a. b` now. Then stop.");
        assert_eq!(s, vec!["Run `a. b` now.", "Then stop."]);
    }

    #[test]
    fn imperative_detection() {
        assert!(is_imperative("Use pnpm, not npm."));
        assert!(is_imperative("**Tests:** run `cargo test` first"));
        assert!(is_imperative("You must not commit secrets"));
        assert!(!is_imperative("This crate parses Markdown."));
        assert!(!is_imperative("`src/` holds the code"));
    }
}
