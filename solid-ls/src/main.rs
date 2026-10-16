//! solid-ls — Language Server for SOLID (C-043/044/045, spec/lsp.md).
//!
//! JSON-RPC over stdio via `lsp-server` (sync main loop). Document
//! state lives here; all analysis lives in `analysis`.

use std::collections::HashMap;
use std::error::Error;

use lsp_server::{Connection, ErrorCode, Message, Request, Response};
use lsp_types::{
    HoverProviderCapability, OneOf, RenameOptions, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensServerCapabilities, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, WorkDoneProgressOptions,
};
use solid_ls::analysis::{self, Doc};

fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: None,
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: analysis::token_legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: None,
                work_done_progress_options: Default::default(),
            },
        )),
        ..Default::default()
    }
}

/// Text lookup for cross-file reference ranges: open buffers first,
/// then disk (saved state). Anything unreadable degrades gracefully.
fn lookup_text(open: &HashMap<String, Doc>, path: &str) -> Option<String> {
    analysis::lookup_text(open, path)
}

fn serve(connection: Connection) -> Result<(), Box<dyn Error + Sync + Send>> {
    let server_capabilities = serde_json::to_value(capabilities())?;
    let _params = connection.initialize(server_capabilities)?;
    let mut open: HashMap<String, Doc> = HashMap::new();
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                if let Some(response) = handle_request(&mut open, req) {
                    connection.sender.send(Message::Response(response))?;
                }
            }
            Message::Notification(note) => handle_notification(&mut open, note),
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn parse_request<R>(req: &Request) -> Result<R::Params, Response>
where
    R: lsp_types::request::Request,
{
    serde_json::from_value::<R::Params>(req.params.clone()).map_err(|e| {
        Response::new_err(
            req.id.clone(),
            ErrorCode::InvalidParams as i32,
            e.to_string(),
        )
    })
}

fn handle_request(open: &mut HashMap<String, Doc>, req: Request) -> Option<Response> {
    let lookup = |path: &str| lookup_text(open, path);
    match req.method.as_str() {
        "textDocument/hover" => {
            let params: lsp_types::HoverParams =
                match parse_request::<lsp_types::request::HoverRequest>(&req) {
                    Ok(params) => params,
                    Err(response) => return Some(response),
                };
            let result = doc_at(
                open,
                &params.text_document_position_params.text_document.uri,
            )
            .and_then(|doc| {
                analysis::hover_at(
                    doc,
                    params.text_document_position_params.position.line,
                    params.text_document_position_params.position.character,
                )
            });
            Some(Response::new_ok(req.id, result))
        }
        "textDocument/definition" => {
            let params: lsp_types::GotoDefinitionParams =
                match parse_request::<lsp_types::request::GotoDefinition>(&req) {
                    Ok(params) => params,
                    Err(response) => return Some(response),
                };
            let result = doc_at(
                open,
                &params.text_document_position_params.text_document.uri,
            )
            .map(|doc| {
                analysis::definition_at(
                    doc,
                    &lookup,
                    params.text_document_position_params.position.line,
                    params.text_document_position_params.position.character,
                )
            });
            Some(Response::new_ok(req.id, result))
        }
        "textDocument/references" => {
            let params: lsp_types::ReferenceParams =
                match parse_request::<lsp_types::request::References>(&req) {
                    Ok(params) => params,
                    Err(response) => return Some(response),
                };
            let result =
                doc_at(open, &params.text_document_position.text_document.uri).map(|doc| {
                    analysis::references_at(
                        doc,
                        &lookup,
                        params.text_document_position.position.line,
                        params.text_document_position.position.character,
                        params.context.include_declaration,
                    )
                });
            Some(Response::new_ok(req.id, result))
        }
        "textDocument/rename" => {
            let params: lsp_types::RenameParams =
                match parse_request::<lsp_types::request::Rename>(&req) {
                    Ok(params) => params,
                    Err(response) => return Some(response),
                };
            let doc = match doc_at(open, &params.text_document_position.text_document.uri) {
                Some(doc) => doc,
                None => {
                    return Some(Response::new_err(
                        req.id,
                        ErrorCode::InvalidParams as i32,
                        "no open document for rename".to_string(),
                    ));
                }
            };
            match analysis::rename_at(
                doc,
                &lookup,
                params.text_document_position.position.line,
                params.text_document_position.position.character,
                &params.new_name,
            ) {
                Ok(edit) => Some(Response::new_ok(req.id, edit)),
                Err(message) => Some(Response::new_err(
                    req.id,
                    ErrorCode::RequestFailed as i32,
                    message,
                )),
            }
        }
        "textDocument/semanticTokens/full" => {
            let params: lsp_types::SemanticTokensParams =
                match parse_request::<lsp_types::request::SemanticTokensFullRequest>(&req) {
                    Ok(params) => params,
                    Err(response) => return Some(response),
                };
            let result = doc_at(open, &params.text_document.uri).map(|doc| {
                let absolute = analysis::semantic_tokens_absolute(&doc.text);
                lsp_types::SemanticTokens {
                    result_id: None,
                    data: analysis::encode_semantic_tokens(&absolute),
                }
            });
            Some(Response::new_ok(req.id, result))
        }
        _ => Some(Response::new_err(
            // Requests always get a response: an unknown method is
            // MethodNotFound, never silence (a hanging client is
            // worse than a refusal). Notifications have no id and
            // never reach this dispatcher.
            req.id.clone(),
            ErrorCode::MethodNotFound as i32,
            format!("unsupported method: {}", req.method),
        )),
    }
}

fn doc_at<'a>(open: &'a HashMap<String, Doc>, uri: &lsp_types::Url) -> Option<&'a Doc> {
    open.get(&uri.to_string())
}

fn handle_notification(open: &mut HashMap<String, Doc>, note: lsp_server::Notification) {
    match note.method.as_str() {
        "textDocument/didOpen" => {
            if let Ok(params) =
                serde_json::from_value::<lsp_types::DidOpenTextDocumentParams>(note.params)
            {
                let doc = analysis::open_doc(
                    params.text_document.uri.clone(),
                    params.text_document.text,
                    params.text_document.version,
                );
                open.insert(params.text_document.uri.to_string(), doc);
            }
        }
        "textDocument/didChange" => {
            if let Ok(params) =
                serde_json::from_value::<lsp_types::DidChangeTextDocumentParams>(note.params)
            {
                if let Some(change) = params.content_changes.into_iter().last() {
                    let doc = analysis::open_doc(
                        params.text_document.uri.clone(),
                        change.text,
                        params.text_document.version,
                    );
                    open.insert(params.text_document.uri.to_string(), doc);
                }
            }
        }
        "textDocument/didClose" => {
            if let Ok(params) =
                serde_json::from_value::<lsp_types::DidCloseTextDocumentParams>(note.params)
            {
                open.remove(&params.text_document.uri.to_string());
            }
        }
        _ => {}
    }
}

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    let (connection, io_threads) = Connection::stdio();
    serve(connection)?;
    io_threads.join()?;
    Ok(())
}
