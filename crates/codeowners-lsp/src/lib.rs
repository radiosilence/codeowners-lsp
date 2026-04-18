//! Shared library crate for codeowners-lsp and codeowners-cli.
//!
//! Exposes the modules used by both binaries, enabling external consumers
//! (benchmarks, integration tests) to import them.
//!
//! Pure parsing/matching lives in [`codeowners_parser`]; this crate houses
//! the LSP server, CLI commands, GitHub client, diagnostic logic, and
//! filesystem-bound helpers (file cache, fixer). For call-site ergonomics,
//! the parser modules are re-exported under the same paths they previously
//! occupied in this crate.

pub use codeowners_parser::{parser, pattern, validation};

pub mod blame;
pub mod diagnostics;
pub mod file_cache;
pub mod github;
pub mod handlers;
pub mod lookup;
pub mod ownership;
pub mod settings;
