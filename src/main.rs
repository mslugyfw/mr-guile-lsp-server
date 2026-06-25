//! Entry point.
//!
//! By default runs the LSP server over stdio. `--extract-deps` pre-releases the
//! bundled Scheme deps to the cache (useful for packaging / warming the cache
//! before the first editor session). Logs go to stderr so they never corrupt
//! the stdio LSP transport.

use mr_guile_lsp_server::{bundle, Backend};
use std::process::ExitCode;
use tower_lsp::{LspService, Server};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mr_guile_lsp_server=info".into()),
        )
        .init();

    match std::env::args().nth(1).as_deref() {
        None | Some("--serve") => run_server().await,
        Some("--extract-deps") | Some("--extract") => extract_deps(),
        Some("-h") | Some("--help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown argument: {other}\n");
            print_help();
            ExitCode::from(2)
        }
    }
}

/// Option B (default): the bundled Scheme deps are released on the first LSP
/// session inside `initialized` (see `backend.rs`). This just runs the server.
async fn run_server() -> ExitCode {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
    ExitCode::SUCCESS
}

/// Option A: pre-release the bundled Scheme deps to the persistent cache now,
/// then exit. Lets packagers / users warm the cache before the first session.
fn extract_deps() -> ExitCode {
    match bundle::materialize() {
        Ok(dir) => {
            eprintln!("extracted bundled deps to: {}", dir.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("failed to extract bundled deps: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    eprintln!(
        "mr-guile-lsp-server {VERSION}\n\
         \n\
         A Guile 3.0.11 LSP server for Helix (Rust + tower-lsp).\n\
         \n\
         USAGE:\n    \
         mr-guile-lsp-server [OPTIONS]\n\
         \n\
         OPTIONS:\n    \
         (no args)          Run as the LSP server over stdio (default)\n    \
         --extract-deps     Release the bundled Scheme deps to the cache and exit\n    \
         -h, --help         Print this help\n"
    );
}
