//! mnemoa-core: keep a repository's agent memory small, true and enforced.
//!
//! This crate never touches the network.

pub mod budget;
pub mod catalog;
pub mod config;
pub mod diag;
pub mod discover;
pub mod doc;
pub mod drift;
pub mod enforcers;
pub mod frontmatter;
pub mod history;
pub mod plans;
pub mod rationale;
pub mod repo;
pub mod report;
pub mod rules;
pub mod scan;
pub mod symbols;
pub mod tokens;
pub mod workspace;

pub use config::Config;
pub use diag::{Finding, Severity, Summary};
pub use discover::{DiscoverOptions, Discovery, Loading, Surface, SurfaceKind, Target, discover};

/// Crate version, reported by the CLI and the API.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
