//! End-to-end integration tests: real Guile REPL + bundled deps driving the
//! diagnostic and completion pipelines the way the LSP backend does.

use mr_guile_lsp_server::bundle;
use mr_guile_lsp_server::diagnostics;
use mr_guile_lsp_server::guile::GuileRepl;
use mr_guile_lsp_server::parser::SExpr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tempfile::TempDir;
use tower_lsp::lsp_types::DiagnosticSeverity;

/// A single, shared, isolated temp tree for the bundled deps — created once via
/// `materialize_into` (non-forcing, so concurrent tests reuse rather than wipe
/// each other's extraction, and ~/.cache is never touched).
static DEPS_DIR: OnceLock<PathBuf> = OnceLock::new();

fn deps_dir() -> PathBuf {
    DEPS_DIR
        .get_or_init(|| {
            let tmp = TempDir::new().expect("temp dir");
            let dir = bundle::materialize_into(tmp.path()).expect("materialize deps");
            // Keep the temp dir alive for the whole test-binary lifetime (Guile
            // reads modules at spawn; we don't want them deleted mid-process).
            std::mem::forget(tmp);
            dir
        })
        .clone()
}

async fn spawn_repl() -> GuileRepl {
    GuileRepl::spawn(&deps_dir()).await.expect("spawn guile")
}

fn write_temp(name: &str, text: &str) -> PathBuf {
    let path = std::env::temp_dir().join(name);
    let mut f = std::fs::File::create(&path).expect("create temp");
    f.write_all(text.as_bytes()).expect("write temp");
    path
}

async fn check_syntax(repl: &mut GuileRepl, path: &Path) -> SExpr {
    // MSYS2 Guile treats `\` as an escape char in paths (`\U`, `\s` …); forward
    // slashes work on Windows too, so normalize before handing to the REPL.
    let guile_path = path.display().to_string().replace('\\', "/");
    let expr = format!("(lsp-check-syntax \"{}\")", guile_path.replace('"', "\\\""));
    repl.request(&expr).await.expect("lsp-check-syntax")
}

#[tokio::test]
async fn diagnostics_pipeline_detects_unbound_variable() {
    let mut repl = spawn_repl().await;
    let text = "(define (broken)\n  undefined-symbol-here)\n";
    let path = write_temp("mr-guile-lsp-it-unbound.scm", text);

    let sexpr = check_syntax(&mut repl, &path).await;
    let warnings = sexpr
        .alist_ref("warnings")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let error = sexpr.alist_ref("error").and_then(|v| v.as_str());
    let diags = diagnostics::build_diagnostics(text, warnings, error);

    assert_eq!(diags.len(), 1, "diags: {diags:?}");
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
    // Reported at line 2 col 2 (1-based) -> 0-based line 1.
    assert_eq!(diags[0].range.start.line, 1);
    assert!(diags[0].message.contains("unbound"));

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn diagnostics_pipeline_detects_unbalanced_parens() {
    let mut repl = spawn_repl().await;
    let text = "(define (f)\n  (display \"hi\")\n"; // missing closing paren
    let path = write_temp("mr-guile-lsp-it-unbalanced.scm", text);

    let sexpr = check_syntax(&mut repl, &path).await;
    let warnings = sexpr
        .alist_ref("warnings")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let error = sexpr.alist_ref("error").and_then(|v| v.as_str());
    let diags = diagnostics::build_diagnostics(text, warnings, error);

    assert!(!diags.is_empty(), "expected a syntax-error diagnostic");
    assert!(
        diags
            .iter()
            .any(|d| d.severity == Some(DiagnosticSeverity::ERROR)),
        "diags: {diags:?}"
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn diagnostics_pipeline_clean_file_has_no_diagnostics() {
    let mut repl = spawn_repl().await;
    let text = "(define (ok x)\n  (* x 2))\n";
    let path = write_temp("mr-guile-lsp-it-clean.scm", text);

    let sexpr = check_syntax(&mut repl, &path).await;
    let warnings = sexpr
        .alist_ref("warnings")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let error = sexpr.alist_ref("error").and_then(|v| v.as_str());
    let diags = diagnostics::build_diagnostics(text, warnings, error);

    assert!(
        diags.is_empty(),
        "clean file should have no diagnostics: {diags:?}"
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn completion_returns_matching_symbols() {
    let mut repl = spawn_repl().await;
    let result = repl
        .request(r#"(lsp-completions "display")"#)
        .await
        .expect("completion");
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
    assert!(labels.iter().any(|s| s == "display"), "labels: {labels:?}");
}

#[tokio::test]
async fn completion_finds_module_exported_symbol() {
    // A baseline Scheme LSP must complete identifiers defined inside a
    // `define-module` form, not only top-level defines in (guile-user).
    let mut repl = spawn_repl().await;
    let path = write_temp(
        "mr-guile-lsp-it-modcomp.scm",
        "(define-module (modcomp)\n  #:export (mod-greet))\n(define (mod-greet who)\n  (string-append \"hi \" who))\n",
    );
    let guile_path = path.display().to_string().replace('\\', "/");
    repl.request(&format!("(lsp-load-file \"{}\")", guile_path))
        .await
        .expect("load");

    let result = repl
        .request(r#"(lsp-completions "mod-greet")"#)
        .await
        .expect("completion");
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
    assert!(
        labels.iter().any(|s| s == "mod-greet"),
        "module-exported symbol must be completable: {labels:?}"
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn hover_returns_user_docstring() {
    let mut repl = spawn_repl().await;
    let path = write_temp(
        "mr-guile-lsp-it-hover.scm",
        "(define (greet name)\n  \"Say hi\"\n  (string-append \"hi \" name))\n",
    );
    let guile_path = path.display().to_string().replace('\\', "/");
    repl.request(&format!("(lsp-load-file \"{}\")", guile_path))
        .await
        .expect("load");

    let result = repl
        .request("(lsp-documentation (string->symbol \"greet\"))")
        .await
        .expect("documentation");
    match result {
        SExpr::Str(s) => assert!(s.contains("Say hi"), "docstring: {s}"),
        other => panic!("expected a docstring, got {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn goto_finds_user_definition_line() {
    let mut repl = spawn_repl().await;
    let path = write_temp(
        "mr-guile-lsp-it-goto.scm",
        "(define (alpha x) x)\n(define (beta y) y)\n",
    );
    let guile_path = path.display().to_string().replace('\\', "/");
    repl.request(&format!("(lsp-load-file \"{}\")", guile_path))
        .await
        .expect("load");

    let result = repl
        .request("(lsp-find-definition (string->symbol \"beta\"))")
        .await
        .expect("find-definition");
    // beta is defined on line 2 (1-based).
    let line = result.alist_ref("line");
    assert!(
        matches!(line, Some(SExpr::Number(n)) if (*n as u32) == 2),
        "got {result:?}"
    );
    let _ = std::fs::remove_file(&path);
}
