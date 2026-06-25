//! Full LSP-session integration test: drives the REAL server binary over stdio
//! with Content-Length framing and asserts the responses of the request
//! handlers (completion / hover / definition / document-symbol). This covers
//! the `tower-lsp` handler WIRING and gives an automated full-session
//! regression test — the pure-Rust logic is already unit-tested in src/.
//!
//! Each session uses an isolated cache dir (`MR_GUILE_LSP_CACHE_DIR`) so it
//! never touches `~/.cache` and needs no `--extract-deps` outside the test.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;
use tempfile::TempDir;

/// Path to the server binary (cargo sets CARGO_BIN_EXE_* at runtime).
fn bin() -> String {
    std::env::var("CARGO_BIN_EXE_mr_guile_lsp_server")
        .unwrap_or_else(|_| "mr-guile-lsp-server".to_string())
}

/// Shared isolated cache dir for the test binary (extracted once, reused).
static CACHE: OnceLock<(TempDir, PathBuf)> = OnceLock::new();

fn cache_dir() -> PathBuf {
    CACHE
        .get_or_init(|| {
            let tmp = TempDir::new().expect("temp dir");
            let path = tmp.path().to_path_buf();
            let status = Command::new(bin())
                .arg("--extract-deps")
                .env("MR_GUILE_LSP_CACHE_DIR", &path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("run --extract-deps");
            assert!(status.success(), "--extract-deps failed");
            (tmp, path)
        })
        .1
        .clone()
}

struct Session {
    child: std::process::Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Session {
    fn start() -> Self {
        let mut child = Command::new(bin())
            .env("MR_GUILE_LSP_CACHE_DIR", cache_dir())
            .env("GUILE_AUTO_COMPILE", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut s = Session {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        // Handshake: send initialize, READ its response, THEN send initialized
        // (tower-lsp drops `initialized` if sent before the response is read).
        s.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"capabilities": {}, "processId": 1, "rootUri": null}
        }));
        let resp = s.recv();
        assert!(
            resp["result"]["capabilities"].is_object(),
            "init resp: {resp}"
        );
        s.send(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
        s
    }

    fn send(&mut self, msg: Value) {
        let body = serde_json::to_string(&msg).unwrap();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        self.stdin.flush().unwrap();
    }

    /// Read the next message that has an `id` (a response); skip notifications
    /// like publishDiagnostics / logMessage.
    fn response(&mut self) -> Value {
        loop {
            let msg = self.recv();
            if msg.get("id").is_some() {
                return msg;
            }
        }
    }

    fn recv(&mut self) -> Value {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("read header");
            assert!(n > 0, "server closed stdout unexpectedly");
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some(v) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(v.trim().parse().expect("Content-Length num"));
            }
        }
        let len = content_length.expect("no Content-Length header");
        let mut body = vec![0u8; len];
        self.stdout.read_exact(&mut body).expect("read body");
        serde_json::from_slice(&body).expect("parse JSON")
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Best-effort teardown so the child never lingers between tests.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn full_lsp_session_drives_all_handlers() {
    let mut s = Session::start();

    // A tiny Guile "project": one file with two top-level defines.
    let text = "(define (greet name)\n  \"Say hi\"\n  (string-append \"hi \" name))\n(define (caller) (greet \"world\"))\n";
    //  line 0: (define (greet name)        -> greet at cols 9..13 (g@9, r@10, ...)
    //  line 3: (define (caller) (greet "world")) -> greet at cols 18..22 (g@18, r@19, e@20, ...)
    let proj = TempDir::new().unwrap();
    let file = proj.path().join("main.scm");
    std::fs::write(&file, text).unwrap();
    let uri = format!("file://{}", file.display());

    s.send(json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "scheme", "version": 1, "text": text}}
    }));
    // Let the debounced diagnose run (it loads the file into the REPL so user
    // symbols like `greet` become introspectable) and let Guile warm up.
    std::thread::sleep(Duration::from_millis(1500));

    // --- completion: cursor inside the `greet` call (line 3, col 20 -> prefix "gr") ---
    s.send(json!({
        "jsonrpc": "2.0", "id": 10, "method": "textDocument/completion",
        "params": {"textDocument": {"uri": uri}, "position": {"line": 3, "character": 20}}
    }));
    let resp = s.response();
    let items = resp["result"]
        .get("items")
        .and_then(|v| v.as_array())
        .or_else(|| resp["result"].as_array())
        .cloned()
        .unwrap_or_default();
    assert!(!items.is_empty(), "completion should return items: {resp}");

    // --- hover on the `greet` definition (col 10, on 'r') ---
    s.send(json!({
        "jsonrpc": "2.0", "id": 11, "method": "textDocument/hover",
        "params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 10}}
    }));
    let resp = s.response();
    assert!(
        resp["result"]["contents"].is_object(),
        "hover should show contents: {resp}"
    );

    // --- documentSymbol: exactly two top-level defines ---
    s.send(json!({
        "jsonrpc": "2.0", "id": 12, "method": "textDocument/documentSymbol",
        "params": {"textDocument": {"uri": uri}}
    }));
    let resp = s.response();
    let syms = resp["result"]
        .as_array()
        .expect("documentSymbol returns an array");
    assert_eq!(syms.len(), 2, "expected two defines: {resp}");

    // --- goto definition: from the `greet` call (line 3, col 18, on 'g') -> Tier 1
    //     finds `greet` in THIS file, so the location stays in-file. ---
    s.send(json!({
        "jsonrpc": "2.0", "id": 13, "method": "textDocument/definition",
        "params": {"textDocument": {"uri": uri}, "position": {"line": 3, "character": 18}}
    }));
    let resp = s.response();
    assert_eq!(
        resp["result"]["uri"], uri,
        "in-file definition should resolve to this file: {resp}"
    );
}

#[test]
fn goto_definition_finds_macro_in_another_file() {
    let mut s = Session::start();
    let proj = TempDir::new().unwrap();
    // macros.scm defines a macro; use.scm uses it.
    std::fs::write(
        proj.path().join("macros.scm"),
        "(define-syntax my-when\n  (syntax-rules () ((_ body) body)))\n",
    )
    .unwrap();
    let use_file = proj.path().join("use.scm");
    std::fs::write(&use_file, "(my-when 42)\n").unwrap();
    let use_uri = format!("file://{}", use_file.display());
    let macros_uri = format!("file://{}/macros.scm", proj.path().display());

    s.send(json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": use_uri, "languageId": "scheme", "version": 1, "text": "(my-when 42)\n"}}
    }));
    std::thread::sleep(Duration::from_millis(800));

    // goto on `my-when` in use.scm -> Tier 1.5 finds the macro in macros.scm.
    s.send(json!({
        "jsonrpc": "2.0", "id": 20, "method": "textDocument/definition",
        "params": {"textDocument": {"uri": use_uri}, "position": {"line": 0, "character": 2}}
    }));
    let resp = s.response();
    assert_eq!(
        resp["result"]["uri"], macros_uri,
        "cross-file macro should jump to its defining file: {resp}"
    );
}
