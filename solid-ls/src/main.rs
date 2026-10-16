//! solid-ls — Language Server for SOLID (C-043/044/045, spec/lsp.md).
//!
//! JSON-RPC over stdio via `lsp-server` (sync main loop). Document
//! state lives here; all analysis lives in `analysis`.

use std::collections::HashMap;
use std::error::Error;

use lsp_server::{Connection, ErrorCode, Message, Request, Response};
use lsp_types::{
    CodeActionProviderCapability, CompletionOptions, HoverProviderCapability, OneOf, RenameOptions,
    SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensServerCapabilities,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, WorkDoneProgressOptions,
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
        completion_provider: Some(CompletionOptions {
            trigger_characters: None,
            ..Default::default()
        }),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
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
    // URIs ever published per open document, for stale-group
    // withdrawal and full close-clears (spec/lsp.md §4).
    let mut published: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
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
            Message::Notification(note) => {
                for outgoing in handle_notification(&mut open, &mut published, note) {
                    connection.sender.send(outgoing)?;
                }
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn parse_request<R>(req: &Request) -> Result<R::Params, Box<Response>>
where
    R: lsp_types::request::Request,
{
    serde_json::from_value::<R::Params>(req.params.clone()).map_err(|e| {
        Box::new(Response::new_err(
            req.id.clone(),
            ErrorCode::InvalidParams as i32,
            e.to_string(),
        ))
    })
}

fn handle_request(open: &mut HashMap<String, Doc>, req: Request) -> Option<Response> {
    let lookup = |path: &str| lookup_text(open, path);
    match req.method.as_str() {
        "textDocument/hover" => {
            let params: lsp_types::HoverParams =
                match parse_request::<lsp_types::request::HoverRequest>(&req) {
                    Ok(params) => params,
                    Err(response) => return Some(*response),
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
                    Err(response) => return Some(*response),
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
                    Err(response) => return Some(*response),
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
                    Err(response) => return Some(*response),
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
                    Err(response) => return Some(*response),
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
        "textDocument/completion" => {
            let params: lsp_types::CompletionParams =
                match parse_request::<lsp_types::request::Completion>(&req) {
                    Ok(params) => params,
                    Err(response) => return Some(*response),
                };
            let result =
                doc_at(open, &params.text_document_position.text_document.uri).map(|doc| {
                    analysis::completion_at(
                        doc,
                        params.text_document_position.position.line,
                        params.text_document_position.position.character,
                    )
                });
            Some(Response::new_ok(req.id, result))
        }
        "textDocument/inlayHint" => {
            let params: lsp_types::InlayHintParams =
                match parse_request::<lsp_types::request::InlayHintRequest>(&req) {
                    Ok(params) => params,
                    Err(response) => return Some(*response),
                };
            let result = doc_at(open, &params.text_document.uri)
                .map(|doc| analysis::inlay_hints(doc, Some(params.range)));
            Some(Response::new_ok(req.id, result))
        }
        "textDocument/codeAction" => {
            let params: lsp_types::CodeActionParams =
                match parse_request::<lsp_types::request::CodeActionRequest>(&req) {
                    Ok(params) => params,
                    Err(response) => return Some(*response),
                };
            let result = doc_at(open, &params.text_document.uri)
                .map(|doc| analysis::code_actions(doc, params.range));
            Some(Response::new_ok(req.id, result))
        }
        // Dependency-graph queries for agents and dashboards
        // (spec/graph.md §5): custom methods, no capability needed.
        "solid/graph" => {
            let Some(uri) = text_doc_uri(&req.params) else {
                return Some(Response::new_err(
                    req.id,
                    ErrorCode::InvalidParams as i32,
                    "solid/graph needs {textDocument: {uri}}".to_string(),
                ));
            };
            let result = doc_at(open, &uri).map(|doc| {
                let (mermaid, dot, json) = analysis::graph_snapshot(doc);
                serde_json::json!({"mermaid": mermaid, "dot": dot, "json": json})
            });
            Some(Response::new_ok(req.id, result))
        }
        "solid/impact" => {
            let Some((uri, line, character)) = text_doc_position(&req.params) else {
                return Some(Response::new_err(
                    req.id,
                    ErrorCode::InvalidParams as i32,
                    "solid/impact needs {textDocument, position}".to_string(),
                ));
            };
            let result = doc_at(open, &uri).and_then(|doc| {
                analysis::impact_at(doc, line, character)
                    .map(|(key, paths)| serde_json::json!({"key": key, "paths": paths}))
            });
            Some(Response::new_ok(req.id, result))
        }
        "solid/why" => {
            let Some((uri, line, character)) = text_doc_position(&req.params) else {
                return Some(Response::new_err(
                    req.id,
                    ErrorCode::InvalidParams as i32,
                    "solid/why needs {textDocument, position}".to_string(),
                ));
            };
            let result = doc_at(open, &uri).and_then(|doc| {
                analysis::why_at(doc, line, character)
                    .map(|(key, paths)| serde_json::json!({"key": key, "paths": paths}))
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

/// Client URI from custom-method params (`{textDocument: {uri}}`).
fn text_doc_uri(params: &serde_json::Value) -> Option<lsp_types::Url> {
    params
        .get("textDocument")?
        .get("uri")?
        .as_str()
        .and_then(|uri| uri.parse().ok())
}

/// Client URI plus 0-based position from custom-method params.
fn text_doc_position(params: &serde_json::Value) -> Option<(lsp_types::Url, u32, u32)> {
    let uri = text_doc_uri(params)?;
    let position = params.get("position")?;
    let line = position.get("line")?.as_u64()? as u32;
    let character = position.get("character")?.as_u64()? as u32;
    Some((uri, line, character))
}

/// Publish notification for a document's current diagnostics.
/// `version` is omitted (None) for close-clears: some clients
/// discard clears older than the last document version.
fn publish(
    uri: &lsp_types::Url,
    version: Option<i32>,
    diagnostics: Vec<lsp_types::Diagnostic>,
) -> Message {
    let params = serde_json::to_value(lsp_types::PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version,
    })
    .unwrap_or(serde_json::Value::Null);
    Message::Notification(lsp_server::Notification {
        method: "textDocument/publishDiagnostics".to_string(),
        params,
    })
}

/// Diagnostics for every file with findings: the open document plus
/// included files (spec/lsp.md §4). Cross-file text resolves from
/// open buffers first, then disk. Versions are per-URI: the open
/// document's version for its own URI, the included buffer's version
/// when that file is also open, and none otherwise (a parent's
/// version is meaningless for another file and may cause valid
/// diagnostics to be discarded).
fn publish_all(
    open: &HashMap<String, Doc>,
    published: &mut HashMap<String, std::collections::HashSet<String>>,
    key: &str,
    doc: &Doc,
    version: Option<i32>,
) -> Vec<Message> {
    let lookup = |path: &str| analysis::lookup_text(open, path);
    let mut outgoing = Vec::new();
    let mut current = std::collections::HashSet::new();
    for (uri, diagnostics) in analysis::diagnostics_by_file(doc, &lookup) {
        let uri_string = uri.to_string();
        current.insert(uri_string.clone());
        // Per-URI versions (spec §4): the open buffer's own version
        // under canonical-tolerant matching, else none — never the
        // parent document's version for another file.
        let file_version = if uri_string == key {
            version
        } else {
            analysis::open_version(open, &uri)
        };
        outgoing.push(publish(&uri, file_version, diagnostics));
    }
    // Withdraw groups that vanished (removed includes), preserving
    // URIs another open document still owns (see `withdraw`).
    if let Some(previous) = published.insert(key.to_string(), current.clone()) {
        outgoing.extend(withdraw(
            open,
            published,
            key,
            previous.difference(&current).cloned().collect(),
        ));
    }
    outgoing
}

/// Clear-or-republish URIs formerly owned by `closing_key`: any URI
/// still published by another open document (including a still-open
/// included file itself) is republished from that owner's current
/// state instead of cleared; the rest clear versionless.
fn withdraw(
    open: &HashMap<String, Doc>,
    published: &HashMap<String, std::collections::HashSet<String>>,
    closing_key: &str,
    uris: std::collections::HashSet<String>,
) -> Vec<Message> {
    let mut uris: Vec<String> = uris.into_iter().collect();
    uris.sort();
    let lookup = |path: &str| analysis::lookup_text(open, path);
    let mut outgoing = Vec::new();
    for stale in uris {
        // Still owned by another open document? Republish from a
        // remaining owner instead of clearing. Owners sort by URI:
        // co-owners should agree (same file content), but the choice
        // is deterministic either way (HashMap order is random).
        // Ownership compares by file identity, not URI spelling:
        // client URIs and canonical include stamps for the same file
        // can differ textually (Windows 8.3 short-names).
        let mut owners: Vec<(&String, &Doc)> = open
            .iter()
            .filter(|(other_key, _)| *other_key != closing_key)
            .filter(|(other_key, _)| {
                published
                    .get(*other_key)
                    .is_some_and(|set| set.iter().any(|u| analysis::same_uri(u, &stale)))
            })
            .collect();
        owners.sort_by(|a, b| a.0.cmp(b.0));
        let mut republished = false;
        for (_, other_doc) in owners {
            if let Some((uri, diagnostics)) = analysis::diagnostics_by_file(other_doc, &lookup)
                .into_iter()
                .find(|(uri, _)| analysis::same_uri(uri.as_ref(), &stale))
            {
                outgoing.push(publish(
                    &uri,
                    analysis::open_version(open, &uri),
                    diagnostics,
                ));
                republished = true;
                break;
            }
        }
        if !republished {
            if let Ok(uri) = stale.parse() {
                let uri: lsp_types::Url = uri;
                outgoing.push(publish(&uri, None, Vec::new()));
            }
        }
    }
    outgoing
}

fn handle_notification(
    open: &mut HashMap<String, Doc>,
    published: &mut HashMap<String, std::collections::HashSet<String>>,
    note: lsp_server::Notification,
) -> Vec<Message> {
    match note.method.as_str() {
        "textDocument/didOpen" => {
            if let Ok(params) =
                serde_json::from_value::<lsp_types::DidOpenTextDocumentParams>(note.params)
            {
                let version = params.text_document.version;
                let key = params.text_document.uri.to_string();
                let doc = analysis::open_doc(
                    params.text_document.uri.clone(),
                    params.text_document.text,
                    params.text_document.version,
                );
                let outgoing = publish_all(open, published, &key, &doc, Some(version));
                open.insert(key, doc);
                return outgoing;
            }
        }
        "textDocument/didChange" => {
            if let Ok(params) =
                serde_json::from_value::<lsp_types::DidChangeTextDocumentParams>(note.params)
            {
                if let Some(change) = params.content_changes.into_iter().last() {
                    let version = params.text_document.version;
                    let key = params.text_document.uri.to_string();
                    let doc = analysis::open_doc(
                        params.text_document.uri.clone(),
                        change.text,
                        params.text_document.version,
                    );
                    let outgoing = publish_all(open, published, &key, &doc, Some(version));
                    open.insert(key, doc);
                    return outgoing;
                }
            }
        }
        "textDocument/didClose" => {
            if let Ok(params) =
                serde_json::from_value::<lsp_types::DidCloseTextDocumentParams>(note.params)
            {
                let key = params.text_document.uri.to_string();
                open.remove(&key);
                // Clear everything this document ever published, but
                // preserve URIs another open document still owns
                // (shared includes, still-open files): those
                // republish from the remaining owner (spec §4).
                let outgoing = match published.remove(&key) {
                    Some(uris) => withdraw(open, published, &key, uris),
                    None => Vec::new(),
                };
                return outgoing;
            }
        }
        _ => {}
    }
    Vec::new()
}

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    let (connection, io_threads) = Connection::stdio();
    serve(connection)?;
    io_threads.join()?;
    Ok(())
}
