//! Guile REPL subprocess client.
//!
//! Spawns a `guile` process with the bundled `deps/` on its load-path, which
//! runs `lsp-serve` (the sentinel request/response loop in lsp-helpers.scm).
//! Each `request` writes one S-expr line, reads the `write`-formatted response
//! up to the `%%LSP-DONE%%` sentinel, and parses it with [`crate::parser`].

use crate::parser::{parse, ParseError, SExpr};
use std::fmt;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// The end-of-response marker emitted by `lsp-serve`.
const SENTINEL: &str = "%%LSP-DONE%%";

/// Errors that can arise while talking to the Guile subprocess.
#[derive(Debug)]
pub enum ReplError {
    Io(std::io::Error),
    Eof,
    Parse(ParseError),
}

impl fmt::Display for ReplError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplError::Io(e) => write!(f, "io error: {e}"),
            ReplError::Eof => write!(f, "repl reached eof (subprocess exited)"),
            ReplError::Parse(e) => write!(f, "could not parse repl response: {e:?}"),
        }
    }
}

impl std::error::Error for ReplError {}

impl From<std::io::Error> for ReplError {
    fn from(e: std::io::Error) -> Self {
        ReplError::Io(e)
    }
}

impl From<ParseError> for ReplError {
    fn from(e: ParseError) -> Self {
        ReplError::Parse(e)
    }
}

pub struct GuileRepl {
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    _child: Child,
}

impl GuileRepl {
    /// Spawn a `guile` process with `deps_dir` on the load-path, running the
    /// sentinel loop. `deps_dir` must contain the `mr-guile-lsp/` tree.
    pub async fn spawn(deps_dir: &Path) -> std::io::Result<GuileRepl> {
        let mut cmd = Command::new("guile");
        cmd.arg("-L")
            .arg(deps_dir)
            .arg("--no-auto-compile")
            .arg("-c")
            .arg("(use-modules (mr-guile-lsp lsp-helpers)) (lsp-serve)")
            .env("GUILE_AUTO_COMPILE", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("guile stdin");
        let stdout = child.stdout.take().expect("guile stdout");
        Ok(GuileRepl {
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            _child: child,
        })
    }

    /// Send an S-expr request and return the parsed response.
    pub async fn request(&mut self, expr: &str) -> Result<SExpr, ReplError> {
        self.stdin.write_all(expr.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let mut response = String::new();
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).await?;
            if n == 0 {
                return Err(ReplError::Eof);
            }
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if trimmed == SENTINEL {
                break;
            }
            if !response.is_empty() {
                response.push('\n');
            }
            response.push_str(trimmed);
        }
        Ok(parse(&response)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn deps_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deps")
    }

    #[tokio::test]
    async fn completes_symbols_via_repl() {
        let mut repl = GuileRepl::spawn(&deps_dir()).await.expect("spawn guile");
        let result = repl
            .request(r#"(lsp-completions "display")"#)
            .await
            .expect("completion request");
        let labels: Vec<String> = match result {
            SExpr::List(items) => items
                .iter()
                .filter_map(|i| match i {
                    SExpr::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            other => panic!("expected a list, got {other:?}"),
        };
        assert!(labels.iter().any(|s| s == "display"), "got {labels:?}");
    }

    #[tokio::test]
    async fn fetches_documentation_as_string() {
        let mut repl = GuileRepl::spawn(&deps_dir()).await.expect("spawn guile");
        let result = repl
            .request("(lsp-documentation 'map)")
            .await
            .expect("documentation request");
        match result {
            SExpr::Str(s) => assert!(!s.is_empty(), "doc string should be non-empty"),
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn definition_query_is_graceful() {
        let mut repl = GuileRepl::spawn(&deps_dir()).await.expect("spawn guile");
        let result = repl
            .request("(lsp-find-definition 'display)")
            .await
            .expect("definition request");
        // `display` is re-exported across modules (guile, rnrs, ...), so Geiser's
        // symbol-location may resolve to a source file OR return #f depending on
        // apropos state. Either is valid; the contract is "no crash, well-formed".
        let graceful = result.is_false() || matches!(result, SExpr::List(_));
        assert!(graceful, "expected #f or a location, got {result:?}");
    }

    #[tokio::test]
    async fn handles_multiple_requests_on_one_repl() {
        let mut repl = GuileRepl::spawn(&deps_dir()).await.expect("spawn guile");
        let _ = repl.request(r#"(lsp-completions "list")"#).await.unwrap();
        let _ = repl.request("(lsp-documentation 'map)").await.unwrap();
        // A third request must still work on the same long-lived process.
        let third = repl
            .request(r#"(lsp-completions "display")"#)
            .await
            .unwrap();
        assert!(matches!(third, SExpr::List(_)));
    }
}
