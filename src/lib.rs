//! mr-guile-lsp-server: a Guile 3.0.11 LSP server for the Helix editor.
//!
//! Architecture: a Rust shell built on `tower-lsp` handles the LSP protocol,
//! document synchronization and UTF-8 positions, while a Guile REPL subprocess
//! (running bundled Geiser modules) performs the semantic analysis.

pub mod backend;
pub mod bundle;
pub mod capabilities;
pub mod diagnostics;
pub mod documents;
pub mod guile;
pub mod parser;
pub mod position;
pub mod scheduler;
pub mod text;

pub use backend::Backend;
