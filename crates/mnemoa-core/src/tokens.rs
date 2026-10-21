//! Token estimates.
//!
//! Counts use OpenAI's `o200k_base` encoding when the `tiktoken` feature is on (the default).
//! Anthropic's tokenizer is not public, so every count is an estimate and is labeled as one.

use serde::{Deserialize, Serialize};

/// How tokens are counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Tokenizer {
    /// `o200k_base` via tiktoken-rs (needs the `tiktoken` feature).
    #[default]
    O200k,
    /// Bytes divided by four, rounded up. Fast and dependency-free.
    Heuristic,
}

impl Tokenizer {
    /// The best tokenizer compiled into this build.
    pub fn best() -> Self {
        if cfg!(feature = "tiktoken") {
            Tokenizer::O200k
        } else {
            Tokenizer::Heuristic
        }
    }

    /// Label shown next to counts.
    pub fn label(self) -> &'static str {
        match self.effective() {
            Tokenizer::O200k => "o200k_base estimate",
            Tokenizer::Heuristic => "bytes/4 estimate",
        }
    }

    fn effective(self) -> Self {
        if cfg!(feature = "tiktoken") {
            self
        } else {
            Tokenizer::Heuristic
        }
    }

    /// Estimated tokens in `text`.
    pub fn count(self, text: &str) -> usize {
        match self.effective() {
            #[cfg(feature = "tiktoken")]
            Tokenizer::O200k => tiktoken_rs::o200k_base_singleton()
                .encode_ordinary(text)
                .len(),
            _ => heuristic(text),
        }
    }
}

/// Bytes divided by four, rounded up.
pub fn heuristic(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Estimated tokens with the best available tokenizer.
pub fn estimate(text: &str) -> usize {
    Tokenizer::best().count(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_rounds_up() {
        assert_eq!(heuristic(""), 0);
        assert_eq!(heuristic("abc"), 1);
        assert_eq!(heuristic("abcde"), 2);
        assert_eq!(Tokenizer::Heuristic.count("abcdefgh"), 2);
    }

    #[test]
    fn o200k_counts_are_plausible() {
        let text = "Always run `make lint` before committing.";
        let n = Tokenizer::O200k.count(text);
        assert!(n > 0 && n < text.len());
        assert!(Tokenizer::best().label().contains("estimate"));
    }
}
