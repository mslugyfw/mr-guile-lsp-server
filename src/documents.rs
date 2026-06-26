//! In-memory document store with FULL text synchronization.
//!
//! Helix sends the full document text on every change when we advertise
//! `TextDocumentSyncKind::FULL` (a KISS choice that avoids incremental-merge
//! bugs; Scheme files are typically small). Each `Document` keeps the text and
//! its version so handlers can query the current buffer without touching disk.

use dashmap::DashMap;
use std::sync::Arc;
use tower_lsp::lsp_types::Url;

/// Thread-safe map from document URI to its current content. Cheaply clonable
/// (shares the underlying map) so spawned tasks can read documents.
#[derive(Clone)]
pub struct DocumentStore(Arc<DashMap<Url, Document>>);

#[derive(Clone)]
pub struct Document {
    pub text: String,
    pub version: i32,
}

impl DocumentStore {
    pub fn new() -> Self {
        Self(Arc::new(DashMap::new()))
    }

    /// Register a freshly opened document.
    pub fn open(&self, uri: Url, text: String, version: i32) {
        self.0.insert(uri, Document { text, version });
    }

    /// Replace the full text of a document (FULL sync).
    pub fn update(&self, uri: &Url, text: String, version: i32) {
        if let Some(mut doc) = self.0.get_mut(uri) {
            doc.text = text;
            doc.version = version;
        } else {
            // Defensive: tolerate a did_change before did_open.
            self.0.insert(uri.clone(), Document { text, version });
        }
    }

    pub fn close(&self, uri: &Url) {
        self.0.remove(uri);
    }

    /// Snapshot of the current text for a document, if open.
    pub fn get_text(&self, uri: &Url) -> Option<String> {
        self.0.get(uri).map(|d| d.text.clone())
    }

    /// Borrow a document for read-only access within a closure.
    pub fn with_doc<R>(&self, uri: &Url, f: impl FnOnce(&Document) -> R) -> Option<R> {
        self.0.get(uri).map(|d| f(&d))
    }

    /// Return any one open document's URI, if at least one is open. Used to
    /// infer a workspace root (via the URI's parent dir) when the client sent
    /// no rootUri.
    pub fn any_uri(&self) -> Option<Url> {
        self.0.iter().next().map(|e| e.key().clone())
    }
}

impl Default for DocumentStore {
    fn default() -> Self {
        Self::new()
    }
}
