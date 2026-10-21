//! Property tests: parsers never panic and keep their offsets valid on arbitrary input.

use mnemoa_core::doc::{Doc, LineIndex, split_sentences};
use mnemoa_core::{drafter, frontmatter, rationale};
use proptest::prelude::*;

/// Markdown-ish text: mostly the characters that matter to the parser.
fn markdownish() -> impl Strategy<Value = String> {
    let pieces = prop::sample::select(vec![
        "# ",
        "## ",
        "- ",
        "* ",
        "1. ",
        "```",
        "```sh\n",
        "`",
        "[",
        "]",
        "(",
        ")",
        "@",
        "<!--",
        "-->",
        "---\n",
        "\n",
        "\n\n",
        " ",
        "a",
        "src/",
        "docs/x.md",
        "Always ",
        "Never ",
        "use ",
        "é",
        "日本",
        "\t",
        "|",
        ":",
        ".",
        "\r\n",
    ]);
    prop::collection::vec(pieces, 0..120).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn doc_parse_never_panics_and_spans_are_valid(text in markdownish()) {
        let d = Doc::parse("AGENTS.md", &text);
        let check = |r: &std::ops::Range<usize>| {
            prop_assert!(r.start <= r.end && r.end <= text.len());
            prop_assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
            Ok(())
        };
        for h in &d.headings { check(&h.span)?; }
        for r in &d.rules { check(&r.span)?; }
        for c in &d.code_spans { check(&c.span)?; }
        for b in &d.code_blocks { check(&b.span)?; }
        for l in &d.links { check(&l.span)?; }
        for i in &d.imports { check(&i.span)?; }
        for c in &d.comments { check(&c.span)?; }
        let _ = d.loaded_text(true);
        let _ = d.line_count();
    }

    #[test]
    fn doc_parse_handles_any_unicode(text in "\\PC{0,300}") {
        let d = Doc::parse("x.md", &text);
        for r in &d.rules {
            prop_assert!(text.is_char_boundary(r.span.start) && text.is_char_boundary(r.span.end));
        }
    }

    #[test]
    fn line_col_is_monotonic(text in "[a-z\\n é]{0,200}") {
        let idx = LineIndex::new(&text);
        let mut last = (1, 0);
        for (off, _) in text.char_indices() {
            let lc = idx.line_col(&text, off);
            prop_assert!(lc >= last, "{lc:?} < {last:?}");
            last = lc;
        }
    }

    #[test]
    fn frontmatter_body_start_is_in_bounds(text in markdownish()) {
        if let Some(fm) = frontmatter::split(&text) {
            prop_assert!(fm.body_start <= text.len());
            prop_assert!(text.is_char_boundary(fm.body_start));
        }
    }

    #[test]
    fn sentences_are_substrings(text in "[A-Za-z .!?`]{0,200}") {
        for s in split_sentences(&text) {
            prop_assert!(text.contains(s));
        }
    }

    #[test]
    fn rationale_and_draft_parsers_never_panic(text in "\\PC{0,200}") {
        let _ = rationale::parse(&text);
        let _ = text.parse::<rationale::RetireWhen>();
        let _ = drafter::parse_draft(&text);
    }

    #[test]
    fn rule_hash_is_stable_under_whitespace(words in prop::collection::vec("[a-z]{1,8}", 1..10)) {
        let a = words.join(" ");
        let b = words.join("   ");
        prop_assert_eq!(rationale::rule_hash(&a), rationale::rule_hash(&b));
    }
}
