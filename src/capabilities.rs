//! Server capability advertisement.
//!
//! We declare FULL text sync, the providers we implement, and the UTF-8
//! position encoding so Helix sends byte-accurate positions (correct for
//! Guile source with non-ASCII content).

use tower_lsp::lsp_types::*;

pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        // FULL sync: Helix resends the whole buffer on every change.
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec!["(".to_string(), " ".to_string()]),
            resolve_provider: Some(false),
            ..Default::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec![" ".to_string(), "(".to_string()]),
            retrigger_characters: None,
            work_done_progress_options: WorkDoneProgressOptions {
                work_done_progress: Some(false),
            },
        }),
        // Advertise UTF-8 positions so byte offsets line up with Guile source.
        position_encoding: Some(PositionEncodingKind::UTF8),
        ..Default::default()
    }
}

pub const SERVER_NAME: &str = "mr-guile-lsp-server";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
