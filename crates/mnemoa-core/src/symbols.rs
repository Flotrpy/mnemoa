//! A lightweight symbol index built with regexes per language.
//!
//! `declared` holds names introduced by declarations (fn, struct, class, def ...). `mentioned`
//! holds every identifier that appears anywhere in source code, so a symbol from a dependency
//! that the code uses still counts as present. Swap in tree-sitter behind [`SymbolIndex::build`]
//! later if precision needs to go up.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::workspace::Workspace;

/// Source files larger than this are skipped.
const MAX_FILE_BYTES: usize = 1 << 20;

static IDENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").unwrap());

struct Lang {
    exts: &'static [&'static str],
    decl: &'static LazyLock<Regex>,
}

static RUST_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|const\s+|unsafe\s+|extern\s+(?:\x22[^\x22]*\x22\s+)?)*(?:fn|struct|enum|trait|type|const|static|mod|union)\s+([A-Za-z_][A-Za-z0-9_]*)|macro_rules!\s*([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});
static JS_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?(?:function\*?|class|const|let|var|interface|type|enum)\s+([A-Za-z_$][A-Za-z0-9_$]*)").unwrap()
});
static PY_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:async\s+)?(?:def|class)\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});
static GO_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(?:func\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)|type\s+([A-Za-z_][A-Za-z0-9_]*))").unwrap()
});
static GENERIC_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:public\s+|private\s+|protected\s+|static\s+|final\s+|abstract\s+)*(?:class|interface|enum|struct|def|fun|func|function)\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});

static LANGS: &[Lang] = &[
    Lang {
        exts: &["rs"],
        decl: &RUST_DECL,
    },
    Lang {
        exts: &["js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts"],
        decl: &JS_DECL,
    },
    Lang {
        exts: &["py", "pyi"],
        decl: &PY_DECL,
    },
    Lang {
        exts: &["go"],
        decl: &GO_DECL,
    },
    Lang {
        exts: &[
            "java", "kt", "kts", "swift", "rb", "cs", "scala", "php", "c", "h", "cc", "cpp", "hpp",
        ],
        decl: &GENERIC_DECL,
    },
];

fn lang_for(path: &str) -> Option<&'static Lang> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    LANGS.iter().find(|l| l.exts.contains(&ext.as_str()))
}

/// Whether a path is source code that the index reads.
pub fn is_source(path: &str) -> bool {
    lang_for(path).is_some()
}

/// Declarations found in one file.
pub fn declarations(path: &str, text: &str) -> Vec<String> {
    let Some(lang) = lang_for(path) else {
        return Vec::new();
    };
    lang.decl
        .captures_iter(text)
        .filter_map(|c| {
            c.iter()
                .skip(1)
                .flatten()
                .next()
                .map(|m| m.as_str().to_string())
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct SymbolIndex {
    /// Declared name → files declaring it.
    pub declared: BTreeMap<String, Vec<String>>,
    /// Every identifier seen in source.
    pub mentioned: HashSet<String>,
    /// Number of source files read.
    pub files: usize,
}

impl SymbolIndex {
    /// Build the index from every source file in the workspace.
    pub fn build(ws: &Workspace) -> SymbolIndex {
        let mut idx = SymbolIndex::default();
        for path in ws.discovery.files.iter().filter(|p| is_source(p)) {
            let Some(text) = ws.read_file(path) else {
                continue;
            };
            if text.len() > MAX_FILE_BYTES {
                continue;
            }
            idx.add(path, &text);
        }
        idx
    }

    pub fn add(&mut self, path: &str, text: &str) {
        self.files += 1;
        for name in declarations(path, text) {
            self.declared
                .entry(name)
                .or_default()
                .push(path.to_string());
        }
        for m in IDENT_RE.find_iter(text) {
            if !self.mentioned.contains(m.as_str()) {
                self.mentioned.insert(m.as_str().to_string());
            }
        }
    }

    /// Whether `name` appears in source at all.
    pub fn knows(&self, name: &str) -> bool {
        self.mentioned.contains(name) || self.declared.contains_key(name)
    }
}

/// Common names that are never repo-specific symbols.
const COMMON: &[&str] = &[
    "String",
    "Vec",
    "Option",
    "Result",
    "None",
    "Some",
    "Ok",
    "Err",
    "Box",
    "HashMap",
    "BTreeMap",
    "HashSet",
    "Self",
    "TODO",
    "FIXME",
    "README",
    "JSON",
    "YAML",
    "TOML",
    "HTTP",
    "HTTPS",
    "URL",
    "API",
    "CLI",
    "SDK",
    "CI",
    "PR",
    "ID",
    "UUID",
    "UTF",
    "OK",
    "GitHub",
    "GitLab",
    "TypeScript",
    "JavaScript",
    "Python",
    "Rust",
    "Markdown",
    "Promise",
    "Array",
    "Object",
    "Map",
    "Set",
    "Error",
    "Exception",
    "NULL",
    "EOF",
    "PATH",
    "HOME",
    "Dockerfile",
    "Makefile",
    "SQL",
    "CSS",
    "HTML",
];

/// If a code span names a symbol (CamelCase, snake_case, `call()`, `a::b`, `a.b()`), the name to
/// look up. Plain lowercase words and file paths are not symbols.
pub fn symbol_in_span(code: &str) -> Option<&str> {
    let code = code.trim();
    if code.is_empty()
        || code.contains([
            ' ', '/', '"', '\'', '=', '<', '>', '{', '}', '[', ']', '$', '@', '*', '#', ',',
        ])
    {
        return None;
    }
    let call = code.ends_with("()");
    let base = code.trim_end_matches("()");
    let last = base
        .rsplit("::")
        .next()
        .unwrap_or(base)
        .rsplit('.')
        .next()
        .unwrap_or(base);
    let qualified = base.contains("::") || (base.contains('.') && call);
    if last.is_empty()
        || !last
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        return None;
    }
    if !last.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    if base.contains('.') && !call && !base.contains("::") {
        // `foo.md`, `a.b` without call: file names and config keys, not symbols.
        return None;
    }
    let has_upper = last.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = last.chars().any(|c| c.is_ascii_lowercase());
    let camel = has_upper && has_lower && last.chars().skip(1).any(|c| c.is_ascii_uppercase());
    let pascal =
        last.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && has_lower && last.len() > 3;
    let snake = last.contains('_')
        && last.len() > 3
        && !last.starts_with('_')
        && !last
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit());
    if !(call || qualified || camel || pascal || snake) {
        return None;
    }
    if COMMON.contains(&last) || last.len() < 3 {
        return None;
    }
    Some(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_per_language() {
        assert_eq!(
            declarations(
                "a.rs",
                "pub fn scan() {}\nstruct Doc;\npub(crate) async fn run() {}\nmacro_rules! m { () => {} }\n"
            ),
            vec!["scan", "Doc", "run", "m"]
        );
        assert_eq!(
            declarations(
                "a.ts",
                "export function load() {}\nexport class Store {}\nconst x = 1;\n"
            ),
            vec!["load", "Store", "x"]
        );
        assert_eq!(
            declarations("a.py", "def main():\n  pass\nclass Thing:\n  pass\n"),
            vec!["main", "Thing"]
        );
        assert_eq!(
            declarations("a.go", "func (s *S) Do() {}\ntype T struct{}\n"),
            vec!["Do", "T"]
        );
        assert!(declarations("a.md", "fn x").is_empty());
    }

    #[test]
    fn symbol_detection() {
        assert_eq!(symbol_in_span("ScanReport"), Some("ScanReport"));
        assert_eq!(symbol_in_span("load_config()"), Some("load_config"));
        assert_eq!(symbol_in_span("Workspace::load"), Some("load"));
        assert_eq!(symbol_in_span("client.scan()"), Some("scan"));
        assert_eq!(symbol_in_span("render_sarif"), Some("render_sarif"));
        assert_eq!(symbol_in_span("make"), None);
        assert_eq!(symbol_in_span("src/main.rs"), None);
        assert_eq!(symbol_in_span("config.toml"), None);
        assert_eq!(symbol_in_span("cargo test"), None);
        assert_eq!(symbol_in_span("String"), None);
        assert_eq!(symbol_in_span("MAX_LINES"), None);
    }

    #[test]
    fn index_knows_mentions_and_declarations() {
        let mut idx = SymbolIndex::default();
        idx.add(
            "src/lib.rs",
            "use std::collections::HashMap;\npub fn scan_repo() { useThing(); }\n",
        );
        assert!(idx.knows("scan_repo"));
        assert!(idx.knows("useThing"));
        assert!(!idx.knows("render_sarif"));
        assert_eq!(idx.declared["scan_repo"], vec!["src/lib.rs"]);
    }
}
