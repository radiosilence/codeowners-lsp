//! Shared library crate for codeowners-lsp and codeowners-cli.
//!
//! Exposes the core modules used by both binaries, enabling
//! external consumers (benchmarks, integration tests) to import them.

pub mod blame;
pub mod diagnostics;
pub mod file_cache;
pub mod github;
pub mod handlers;
pub mod lookup;
pub mod ownership;
pub mod parser;
pub mod pattern;
pub mod settings;
pub mod validation;
