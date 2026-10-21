//! YAML front matter at the top of Markdown files (Cursor `.mdc`, Claude rules,
//! Kiro steering, Copilot instructions).

use serde_yaml_ng::Value;

/// Front matter split from the start of a document.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontMatter {
    /// The raw YAML between the `---` fences.
    pub raw: String,
    /// Parsed YAML, or `None` when the YAML is invalid.
    pub value: Option<Value>,
    /// Byte offset where the body starts (just after the closing fence line).
    pub body_start: usize,
}

impl FrontMatter {
    /// Look up a top-level key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.value.as_ref()?.as_mapping()?.get(key)
    }

    /// A top-level key as a string.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    /// A top-level key as a bool. Accepts YAML booleans and the strings "true"/"false".
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            Value::Bool(b) => Some(*b),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    /// A top-level key as a list of strings. A single string becomes a one-item list,
    /// and a comma-separated string (Cursor `globs` style) is split.
    pub fn get_list(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Some(Value::Sequence(items)) => items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            Some(Value::String(s)) => s
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Whether the key is present at all.
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
}

/// Split front matter from `text`. Front matter must start on the first line with `---`
/// and end with a line that is exactly `---` (or `...`).
pub fn split(text: &str) -> Option<FrontMatter> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let open_len = text.len() - rest.len();
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" || trimmed == "..." {
            let raw = rest[..offset].to_string();
            let value = if raw.trim().is_empty() {
                Some(Value::Mapping(Default::default()))
            } else {
                serde_yaml_ng::from_str::<Value>(&raw).ok()
            };
            return Some(FrontMatter {
                raw,
                value,
                body_start: open_len + offset + line.len(),
            });
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list_and_bool() {
        let fm =
            split("---\npaths:\n  - \"src/**/*.ts\"\nalwaysApply: true\n---\n# Body\n").unwrap();
        assert_eq!(fm.get_list("paths"), vec!["src/**/*.ts"]);
        assert_eq!(fm.get_bool("alwaysApply"), Some(true));
        assert_eq!(
            &"---\npaths:\n  - \"src/**/*.ts\"\nalwaysApply: true\n---\n# Body\n"[fm.body_start..],
            "# Body\n"
        );
    }

    #[test]
    fn comma_separated_globs() {
        let fm = split("---\nglobs: src/*.rs, tests/*.rs\n---\n").unwrap();
        assert_eq!(fm.get_list("globs"), vec!["src/*.rs", "tests/*.rs"]);
    }

    #[test]
    fn no_front_matter_when_not_first_line() {
        assert!(split("# Title\n---\na: 1\n---\n").is_none());
    }

    #[test]
    fn unclosed_front_matter_is_none() {
        assert!(split("---\na: 1\n").is_none());
    }

    #[test]
    fn invalid_yaml_keeps_raw() {
        let fm = split("---\n: : :\n  - [\n---\nbody").unwrap();
        assert!(fm.value.is_none());
        assert!(fm.raw.contains(": : :"));
    }
}
