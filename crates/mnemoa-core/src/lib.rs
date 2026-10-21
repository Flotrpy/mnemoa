//! mnemoa-core: keep a repository's agent memory small, true and enforced.
//!
//! This crate never touches the network.

/// Crate version, reported by the CLI and the API.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
