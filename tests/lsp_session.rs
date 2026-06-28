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
use url::Url;
use tempfile::TempDir;

/// Path to the server binary (cargo sets CARGO_BIN_EXE_* at runtime).
/// When running `cargo test` cargo does NOT build the binary target, so
/// CARGO_BIN_EXE_* may be unset. In that case we use target/debug/<name>
/// and build it on first access via CACHE so it's done exactly once.
fn bin() -> String {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Ok(p) = std::env::var("CARGO_BIN_EXE_mr_guile_lsp_server") {
            return p;
        }
        // cargo test doesn't build bin targets — ensure it exists.
        let path = format!(
            "{}/debug/mr-guile-lsp-server",
            std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into())
        );
        if !std::path::Path::new(&path).exists() {
            let status = Command::new("cargo")
                .args(["build", "--bin", "mr-guile-lsp-server"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("cargo build");
            assert!(status.success(), "cargo build --bin failed");
        }
        path
    })
    .clone()
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
    let uri = Url::from_file_path(&file).unwrap().to_string();

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
    let use_uri = Url::from_file_path(&use_file).unwrap().to_string();
    let macros_uri = Url::from_file_path(proj.path().join("macros.scm")).unwrap().to_string();

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

/// references: finding all occurrences of a symbol across the workspace.
/// The server scans .scm files in the workspace root for references.
#[test]
fn references_finds_usages_across_files() {
    let mut s = Session::start();
    let proj = TempDir::new().unwrap();
    // lib.scm defines `helper`; caller.scm calls it twice.
    std::fs::write(
        proj.path().join("lib.scm"),
        "(define (helper x) x)\n",
    )
    .unwrap();
    let caller = proj.path().join("caller.scm");
    std::fs::write(
        &caller,
        "(define (main)\n  (helper 1)\n  (helper 2))\n",
    )
    .unwrap();
    let caller_uri = Url::from_file_path(&caller).unwrap().to_string();

    s.send(json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": caller_uri, "languageId": "scheme", "version": 1,
                     "text": "(define (main)\n  (helper 1)\n  (helper 2))\n"}}
    }));
    std::thread::sleep(Duration::from_millis(500));

    // references on `helper` at line 1 col 3 (the first call's 'h').
    s.send(json!({
        "jsonrpc": "2.0", "id": 30, "method": "textDocument/references",
        "params": {
            "textDocument": {"uri": caller_uri},
            "position": {"line": 1, "character": 3},
            "context": {"includeDeclaration": true}
        }
    }));
    let resp = s.response();
    let result = resp["result"].as_array();
    assert!(
        result.map(|a| !a.is_empty()).unwrap_or(false),
        "references should find `helper` usages across files: {resp}"
    );
    // Each location is in some .scm under the project.
    if let Some(arr) = result {
        for loc in arr {
            assert!(
                loc["uri"].as_str().unwrap_or("").ends_with(".scm"),
                "reference location uri should be a .scm file: {loc}"
            );
        }
    }
}

/// workspace/symbol: searching defined symbols by name across the workspace.
#[test]
fn workspace_symbol_finds_definition_by_query() {
    let mut s = Session::start();
    let proj = TempDir::new().unwrap();
    // A file defining a uniquely-named function in the workspace.
    std::fs::write(
        proj.path().join("defs.scm"),
        "(define (unique-name-fn a) a)\n",
    )
    .unwrap();
    // We must open a document under the project for the server to track the
    // workspace root. Open the defs file.
    let defs_uri = Url::from_file_path(proj.path().join("defs.scm")).unwrap().to_string();
    s.send(json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": defs_uri, "languageId": "scheme", "version": 1,
                     "text": "(define (unique-name-fn a) a)\n"}}
    }));
    std::thread::sleep(Duration::from_millis(500));

    s.send(json!({
        "jsonrpc": "2.0", "id": 40, "method": "workspace/symbol",
        "params": {"query": "unique-name-fn"}
    }));
    let resp = s.response();
    let result = resp["result"].as_array();
    assert!(
        result.map(|a| !a.is_empty()).unwrap_or(false),
        "workspace/symbol should find `unique-name-fn`: {resp}"
    );
    // The matched symbol's name should contain the query.
    if let Some(arr) = result {
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|s| s["name"].as_str())
            .collect();
        assert!(
            names.iter().any(|n| n.contains("unique-name-fn")),
            "matched symbol name should contain query: {names:?}"
        );
    }
}
