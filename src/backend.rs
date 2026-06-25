//! The `LanguageServer` backend: lifecycle handlers + document sync, plus a
//! shared Guile REPL that semantic handlers (diagnostics, completion, hover,
//! definition, signature) call into.

use crate::bundle;
use crate::capabilities::{server_capabilities, SERVER_NAME, SERVER_VERSION};
use crate::diagnostics::build_diagnostics;
use crate::documents::DocumentStore;
use crate::guile::GuileRepl;
use crate::parser::SExpr;
use crate::scheduler::DebouncedScheduler;
use crate::text::{
    find_definition_in_text, find_references_in_text, scan_defines, symbol_at, symbol_prefix_at,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

/// Quiet period after the last edit before a file is (re)compiled for diagnostics.
const DIAG_DEBOUNCE: Duration = Duration::from_millis(300);

/// The Guile REPL, lazily spawned during `initialized`.
pub type SharedRepl = Arc<Mutex<Option<GuileRepl>>>;

pub struct Backend {
    client: Client,
    documents: DocumentStore,
    repl: SharedRepl,
    diag: DebouncedScheduler<Url>,
    /// Workspace root, captured at initialize (used for goto-references).
    root: Mutex<Option<PathBuf>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: DocumentStore::new(),
            repl: Arc::new(Mutex::new(None)),
            diag: DebouncedScheduler::new(),
            root: Mutex::new(None),
        }
    }

    /// Start the Guile REPL using the *already-extracted* bundled deps. Does
    /// NOT extract — the deps must be released once after install via
    /// `mr-guile-lsp-server --extract-deps`. If missing, log a clear error and
    /// degrade (no semantic features) instead of crashing.
    async fn ensure_repl(&self) {
        let mut slot = self.repl.lock().await;
        if slot.is_some() {
            return;
        }
        let dir = match bundle::materialized_dir() {
            Some(d) => d,
            None => {
                tracing::error!(
                    "bundled Guile deps are not extracted; run \
                     `mr-guile-lsp-server --extract-deps` once after install, \
                     then restart the language server"
                );
                return;
            }
        };
        match GuileRepl::spawn(&dir).await {
            Ok(repl) => {
                tracing::info!("guile repl started");
                *slot = Some(repl);
            }
            Err(e) => tracing::error!("failed to spawn guile repl: {e}"),
        }
    }

    /// Schedule a debounced, non-blocking diagnose for `uri` (see
    /// [`crate::scheduler`]). Returns immediately so the LSP handler can serve
    /// other requests (completion/hover) without waiting on `compile-file`.
    fn schedule_diagnose(&self, uri: Url) {
        let client = self.client.clone();
        let documents = self.documents.clone();
        let repl = self.repl.clone();
        self.diag
            .schedule(uri, DIAG_DEBOUNCE, move |uri| async move {
                diagnose_uri(client, documents, repl, uri).await;
            });
    }
}

/// Run a request against the Guile REPL if it is up; else return None.
async fn repl_request(
    repl: &SharedRepl,
    expr: &str,
) -> Option<std::result::Result<SExpr, crate::guile::ReplError>> {
    let mut slot = repl.lock().await;
    match slot.as_mut() {
        Some(r) => Some(r.request(expr).await),
        None => None,
    }
}

/// Compile the document's current text via the REPL and publish diagnostics.
/// Reads the *latest* text/version at run time, so a burst of edits collapses to
/// diagnostics for the final buffer. Runs in a background task (non-blocking).
async fn diagnose_uri(client: Client, documents: DocumentStore, repl: SharedRepl, uri: Url) {
    let (text, version) = match documents.with_doc(&uri, |d| (d.text.clone(), d.version)) {
        Some(tv) => tv,
        None => return,
    };
    let path = temp_source_path(&uri);
    if let Err(e) = std::fs::write(&path, &text) {
        tracing::warn!(%uri, "failed to write temp source: {e}");
        return;
    }
    // Load into the REPL so Geiser can introspect user-defined symbols.
    let _ = repl_request(
        &repl,
        &format!("(lsp-load-file {})", scheme_string(&path.to_string_lossy())),
    )
    .await;
    let expr = format!(
        "(lsp-check-syntax {})",
        scheme_string(&path.to_string_lossy())
    );
    let diags = match repl_request(&repl, &expr).await {
        Some(Ok(sexpr)) => {
            let warnings = sexpr
                .alist_ref("warnings")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let error = sexpr.alist_ref("error").and_then(|v| v.as_str());
            build_diagnostics(&text, warnings, error)
        }
        Some(Err(e)) => {
            tracing::warn!(%uri, "repl lsp-check-syntax failed: {e}");
            vec![]
        }
        None => vec![],
    };
    // Version-tagged publish: the editor ignores diagnostics for stale buffers.
    client.publish_diagnostics(uri, diags, Some(version)).await;
}

/// Stable temp file path for a document (so recompilation reuses it).
fn temp_source_path(uri: &Url) -> PathBuf {
    let mut h = DefaultHasher::new();
    uri.hash(&mut h);
    std::env::temp_dir().join(format!("mr-guile-lsp-src-{:.0}.scm", h.finish()))
}

/// Render `s` as a Guile string literal (escape `"` and `\`).
fn scheme_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Capture the workspace root for goto-references. Try workspace_folders,
        // then root_uri, then the deprecated root_path string.
        let root = params
            .workspace_folders
            .as_ref()
            .and_then(|fs| fs.first())
            .and_then(|f| f.uri.to_file_path().ok())
            .or_else(|| params.root_uri.as_ref().and_then(|u| u.to_file_path().ok()))
            .or_else(|| {
                #[allow(deprecated)]
                params.root_path.as_ref().map(PathBuf::from)
            });
        tracing::info!(
            root = ?root.as_ref().map(|r| r.display().to_string()),
            has_root_uri = params.root_uri.is_some(),
            has_workspace_folders = params.workspace_folders.is_some(),
            "initialize: workspace info received from client"
        );
        if let Some(r) = root {
            *self.root.lock().await = Some(r);
        }
        Ok(InitializeResult {
            capabilities: server_capabilities(),
            server_info: Some(ServerInfo {
                name: SERVER_NAME.to_string(),
                version: Some(SERVER_VERSION.to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        tracing::info!("mr-guile-lsp-server initialized");
        self.ensure_repl().await;
    }

    async fn shutdown(&self) -> Result<()> {
        tracing::info!("mr-guile-lsp-server shutting down");
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text;
        let version = params.text_document.version;
        tracing::debug!(%uri, version, "did_open");
        self.documents.open(uri.clone(), text, version);
        self.schedule_diagnose(uri.clone());
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        // FULL sync: the last change carries the entire new buffer.
        if let Some(change) = params.content_changes.into_iter().last() {
            self.documents.update(&uri, change.text, version);
        }
        // NOTE: live-on-type diagnostics need debouncing (Phase 5); for now we
        // re-diagnose on change without a debounce so edits still flow through.
        self.schedule_diagnose(uri.clone());
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        tracing::debug!(%uri, "did_close");
        self.documents.close(&uri);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        if let Some(text) = params.text {
            self.documents.update(&uri, text, 0);
        }
        self.schedule_diagnose(uri.clone());
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let text = match self.documents.get_text(uri) {
            Some(t) => t,
            None => return Ok(None),
        };
        let (prefix, _range) = match symbol_prefix_at(&text, &pos) {
            Some(p) => p,
            None => return Ok(None),
        };
        if prefix.is_empty() {
            return Ok(None);
        }
        let expr = format!("(lsp-completions {})", scheme_string(&prefix));
        let items = match repl_request(&self.repl, &expr).await {
            Some(Ok(SExpr::List(labels))) => labels
                .into_iter()
                .filter_map(|l| match l {
                    SExpr::Str(s) => Some(CompletionItem {
                        label: s,
                        ..Default::default()
                    }),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => return Ok(None),
        };
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let text = match self.documents.get_text(uri) {
            Some(t) => t,
            None => return Ok(None),
        };
        let (sym, range) = match symbol_at(&text, &pos) {
            Some(s) => s,
            None => return Ok(None),
        };
        // Try docstring first; if absent, fall back to the signature so hover
        // always shows something useful (many symbols have no docstring).
        let doc_expr = format!(
            "(lsp-documentation (string->symbol {}))",
            scheme_string(&sym)
        );
        let doc = match repl_request(&self.repl, &doc_expr).await {
            Some(Ok(SExpr::Str(s))) if !s.is_empty() => Some(s),
            _ => None,
        };
        let sig_expr = format!("(lsp-signature (string->symbol {}))", scheme_string(&sym));
        let sig = match repl_request(&self.repl, &sig_expr).await {
            Some(Ok(SExpr::Str(s))) if !s.is_empty() => Some(s),
            _ => None,
        };
        match (doc, sig) {
            (Some(d), _) => Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("`{sym}` — {d}"),
                }),
                range: Some(range),
            })),
            (None, Some(s)) => Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("`{s}`"),
                }),
                range: Some(range),
            })),
            (None, None) => Ok(None),
        }
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let text = match self.documents.get_text(uri) {
            Some(t) => t,
            None => return Ok(None),
        };
        // signatureHelp triggers on `(`, so the cursor is often not on the name.
        // Try the symbol at the cursor first, else scan back to the call's name.
        let sym = match symbol_at(&text, &pos) {
            Some((s, _)) => s,
            None => match crate::text::called_symbol_before(&text, &pos) {
                Some(s) => s,
                None => return Ok(None),
            },
        };
        let expr = format!("(lsp-signature (string->symbol {}))", scheme_string(&sym));
        if let Some(Ok(SExpr::Str(sig))) = repl_request(&self.repl, &expr).await {
            if !sig.is_empty() {
                return Ok(Some(SignatureHelp {
                    signatures: vec![SignatureInformation {
                        label: sig,
                        documentation: None,
                        parameters: None,
                        active_parameter: None,
                    }],
                    active_signature: Some(0),
                    active_parameter: None,
                }));
            }
        }
        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let pos = params.text_document_position_params.position;
        let text = match self.documents.get_text(&uri) {
            Some(t) => t,
            None => return Ok(None),
        };
        let (sym, _) = match symbol_at(&text, &pos) {
            Some(s) => s,
            None => return Ok(None),
        };

        // Tier 1: precise in-document definition (no REPL needed).
        if let Some(range) = find_definition_in_text(&text, &sym) {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range,
            })));
        }

        // Tier 1.5: cross-file definition in the workspace (structural). Handles
        // macros and functions defined in OTHER project files, where Geiser's
        // symbol-location returns a line but no source file. Falls back to the
        // document's directory if no workspace root was advertised.
        let root = self.root.lock().await.clone().or_else(|| {
            uri.to_file_path()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        });
        if let Some(loc) = root.and_then(|r| find_definition_in_workspace(&r, &sym)) {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }

        // Tier 2: ask Geiser for cross-module / library locations.
        let expr = format!(
            "(lsp-find-definition (string->symbol {}))",
            scheme_string(&sym)
        );
        if let Some(Ok(loc)) = repl_request(&self.repl, &expr).await {
            if !loc.is_false() {
                if let Some(loc) = geiser_location_to_lsp(&loc, &uri) {
                    return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                }
            }
        }
        Ok(None)
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let text = match self.documents.get_text(uri) {
            Some(t) => t,
            None => return Ok(None),
        };
        let symbols = scan_defines(&text)
            .into_iter()
            .map(|d| DocumentSymbol {
                name: d.name,
                detail: None,
                kind: d.kind,
                tags: None,
                #[allow(deprecated)]
                deprecated: None,
                range: d.range,
                selection_range: d.selection_range,
                children: None,
            })
            .collect::<Vec<_>>();
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let pos = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri.clone();
        let text = match self.documents.get_text(&uri) {
            Some(t) => t,
            None => return Ok(None),
        };
        let (sym, _) = match symbol_at(&text, &pos) {
            Some(s) => s,
            None => return Ok(None),
        };
        // Prefer the workspace root from initialize; fall back to the document's
        // own directory so references still work when the client sent no root.
        let root = self.root.lock().await.clone().or_else(|| {
            uri.to_file_path()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        });
        let locations = match root {
            Some(r) => find_references_in_workspace(&r, &sym),
            None => Vec::new(),
        };
        if locations.is_empty() {
            Ok(None)
        } else {
            Ok(Some(locations))
        }
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let query = params.query.to_lowercase();
        let root = match self.root.lock().await.clone() {
            Some(r) => r,
            None => return Ok(None),
        };
        tracing::info!(
            query = %params.query,
            "workspace/symbol request received"
        );
        let mut infos = Vec::new();
        for file in walk_scheme_files(&root) {
            let Ok(content) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Ok(uri) = Url::from_file_path(&file) else {
                continue;
            };
            for d in scan_defines(&content) {
                if !query.is_empty() && !d.name.to_lowercase().contains(&query) {
                    continue;
                }
                infos.push(SymbolInformation {
                    name: d.name,
                    kind: d.kind,
                    tags: None,
                    #[allow(deprecated)]
                    deprecated: None,
                    location: Location {
                        uri: uri.clone(),
                        range: d.selection_range,
                    },
                    container_name: None,
                });
            }
        }
        if infos.is_empty() {
            Ok(None)
        } else {
            Ok(Some(infos))
        }
    }
}

/// Collect `.scm`/`.ss`/`.sld` file paths under `root`, skipping VCS/build
/// dirs. Capped to avoid pathological trees.
fn walk_scheme_files(root: &Path) -> Vec<PathBuf> {
    const MAX_FILES: usize = 2000;
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with('.')
                        || matches!(
                            name,
                            "target" | "node_modules" | "_build" | "dist" | "build"
                        )
                    {
                        continue;
                    }
                }
                stack.push(path);
            } else if files.len() < MAX_FILES
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e, "scm" | "ss" | "sld"))
            {
                files.push(path);
            }
        }
    }
    files
}

/// Find the definition of `symbol` in some workspace Scheme file (other than
/// the obvious in-file case), by structural scan. Returns the first match —
/// used for cross-file goto on user macros / functions.
fn find_definition_in_workspace(root: &Path, symbol: &str) -> Option<Location> {
    for file in walk_scheme_files(root) {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        if let Some(range) = find_definition_in_text(&content, symbol) {
            let Ok(uri) = Url::from_file_path(&file) else {
                continue;
            };
            return Some(Location { uri, range });
        }
    }
    None
}

/// Collect every reference to `symbol` across the workspace's Scheme files.
fn find_references_in_workspace(root: &Path, symbol: &str) -> Vec<Location> {
    let mut out = Vec::new();
    for file in walk_scheme_files(root) {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(uri) = Url::from_file_path(&file) else {
            continue;
        };
        for range in find_references_in_text(&content, symbol) {
            out.push(Location {
                uri: uri.clone(),
                range,
            });
        }
    }
    out
}
/// LSP Location. Falls back to `current_uri` when the file is unknown.
fn geiser_location_to_lsp(loc: &SExpr, current_uri: &Url) -> Option<Location> {
    let line = loc.alist_ref("line")?;
    let line = match line {
        SExpr::Number(n) => *n as u32,
        _ => return None,
    };
    let uri = match loc.alist_ref("file").and_then(|v| v.as_str()) {
        Some(path) if !path.is_empty() => Url::from_file_path(path).ok()?,
        _ => current_uri.clone(),
    };
    // Geiser line is 1-based; LSP is 0-based.
    let start_line = line.saturating_sub(1);
    Some(Location {
        uri,
        range: Range {
            start: Position {
                line: start_line,
                character: 0,
            },
            end: Position {
                line: start_line,
                character: 0,
            },
        },
    })
}
