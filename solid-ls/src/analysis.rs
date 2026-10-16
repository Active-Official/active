//! Analysis backend for the SOLID language server (C-043/044/045,
//! spec/lsp.md): document model plus hover, definition, references,
//! rename, completions, diagnostics, inlay hints, code actions, and
//! semantic tokens over solid-core.
//!
//! Conventions: positions are 0-based LSP line/UTF-16 character;
//! core spans are 1-based character columns. Conversions are exact
//! through UTF-16 helpers. Every handler is total:
//! unresolvable positions yield empty success, never errors. Secret
//! values render redacted in every response (C-036).

use std::collections::HashMap;

use lsp_types::{
    CodeAction, CodeActionKind, CompletionItem, CompletionItemKind, Hover, HoverContents,
    InlayHint, InlayHintKind, Location, MarkupContent, MarkupKind, Position, Range, TextEdit, Url,
    WorkspaceEdit,
};
use solid_core::{
    lint, parse_recovering, parse_text_recovering_with_file, AnnotationKind, Diagnostic, Engine,
    Module, Node, NodeId, ParseOptions, RepairKind, Severity, Value,
};

/// One open document: parsed module plus resilient evaluation.
/// `version` and the diagnostic lists feed C-044 publishing; kept
/// populated from day one so the model never changes shape.
pub struct Doc {
    pub uri: Url,
    pub text: String,
    /// Document version from the client; C-044 incremental sync uses
    /// it for out-of-order change detection.
    #[allow(dead_code)]
    pub version: i32,
    pub module: Module,
    /// Parse diagnostics: published by C-044 on open/change.
    #[allow(dead_code)]
    pub parse_diags: Vec<Diagnostic>,
    pub value: Option<Value>,
    /// Evaluation diagnostics (resilient): published by C-044.
    #[allow(dead_code)]
    pub eval_diags: Vec<Diagnostic>,
    pub lint_diags: Vec<Diagnostic>,
}

/// Parse buffer text, resolving includes against the file's own
/// directory for `file://` URIs (spec/lsp.md §1). Untitled or
/// unparseable-decision inputs parse hermetically instead.
pub fn open_doc(uri: Url, text: String, version: i32) -> Doc {
    let options = ParseOptions {
        enable_includes: true,
        ..Default::default()
    };
    let (module, parse_diags) = match uri.to_file_path() {
        // File context is preserved on every path — including the
        // error path — so includes never fall back to resolving from
        // the server's working directory.
        Ok(path) => parse_text_recovering_with_file(&text, uri.as_str(), options, &path),
        Err(_) => {
            let (module, diags) = parse_recovering(&text, uri.as_str(), options);
            (module, diags)
        }
    };
    let (value, eval_diags) = match Engine::new().evaluate(&module) {
        Ok((v, d)) => (Some(v), d),
        // Hard evaluation failures (cycles, budgets, validation in
        // strict mode) still publish: the error becomes one
        // diagnostic alongside the module's own records, so
        // open/change handlers surface everything (C-044).
        // Messages are secret-safe by the C-036 audit.
        Err(e) => {
            let mut diags = vec![Diagnostic {
                severity: Severity::Error,
                code: e.code().to_string(),
                message: e.to_string(),
                path: None,
                span: e.span().cloned(),
                repair: None,
                confidence: 1.0,
                file: e.file().map(str::to_string),
            }];
            diags.extend(module.diagnostics.iter().cloned());
            diags.extend(module.healing.iter().cloned());
            (None, diags)
        }
    };
    let lint_diags = lint(&module);
    Doc {
        uri,
        text,
        version,
        module,
        parse_diags,
        value,
        eval_diags,
        lint_diags,
    }
}

/// Local file path backing a document, when it has one.
/// (C-044 incremental sync consumes `version`; kept here so the
/// document model never changes shape.)
#[allow(dead_code)]
pub struct DocVersion(pub i32);

// ------------------------------------------------------------------
// Positions and spans.
//
// Columns cross three frames: core spans are 1-based character
// columns, LSP uses 0-based UTF-16 code units. Every conversion
// below is explicit; non-ASCII documents are covered by tests.
// ------------------------------------------------------------------

/// UTF-16 code units in a string (LSP character units).
fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// LSP content width of a `\n`-split line: LSP line positions
/// exclude the terminator, so a trailing carriage return (CRLF
/// documents) never counts toward columns or edit ranges.
fn line_content_width(line: &str) -> u32 {
    utf16_len(line.strip_suffix('\r').unwrap_or(line))
}

/// 0-based character index → UTF-16 column within one line.
fn char_to_utf16(line: &str, char_idx: usize) -> u32 {
    let mut units = 0u32;
    for (i, c) in line.chars().enumerate() {
        if i >= char_idx {
            break;
        }
        units += c.len_utf16() as u32;
    }
    units
}

/// UTF-16 column → 0-based character index, clamped to the line end
/// and snapped back to a character boundary (a cursor landing inside
/// a surrogate pair resolves to that character's start).
fn utf16_to_char(line: &str, col: u32) -> usize {
    let mut units = 0u32;
    for (i, c) in line.chars().enumerate() {
        let width = c.len_utf16() as u32;
        if units + width > col {
            return i;
        }
        units += width;
    }
    line.chars().count()
}

/// Byte offset of a 0-based (line, UTF-16 character).
pub fn offset_of(text: &str, line: u32, character: u32) -> usize {
    let mut offset = 0usize;
    for (i, l) in text.split('\n').enumerate() {
        if i as u32 == line {
            let char_idx = utf16_to_char(l, character);
            let mut byte = 0usize;
            for (j, c) in l.chars().enumerate() {
                if j >= char_idx {
                    break;
                }
                byte += c.len_utf8();
            }
            return offset + byte.min(l.len());
        }
        offset += l.len() + 1;
    }
    text.len()
}

/// 0-based LSP position of a byte offset (always a character
/// boundary at every call site: searches advance by whole
/// characters, scanners respect quotes and escapes).
pub fn position_of(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let mut line = 0u32;
    let mut start = 0usize;
    for l in text.split('\n') {
        if start + l.len() >= offset {
            let chars = l
                .get(..offset - start)
                .map(|p| p.chars().count())
                .unwrap_or(0);
            return Position {
                line,
                character: char_to_utf16(l, chars),
            };
        }
        start += l.len() + 1;
        line += 1;
    }
    Position { line, character: 0 }
}

/// Core 1-based span → LSP range.
pub fn range_of_span(text: &str, span: &solid_core::diag::Span) -> Range {
    Range {
        start: core_to_lsp(text, span.line, span.column),
        end: core_to_lsp(text, span.end_line, span.end_column),
    }
}

/// Core 1-based (line, char column) → LSP position. Out-of-range
/// lines/columns clamp to the document (spans from recovering parses
/// can point past truncated text).
fn core_to_lsp(text: &str, line_1: u32, col_1: u32) -> Position {
    let lines: Vec<&str> = text.split('\n').collect();
    let line_0 = line_1
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1) as u32);
    let line_text = lines.get(line_0 as usize).copied().unwrap_or("");
    let char_idx = (col_1.saturating_sub(1) as usize).min(line_text.chars().count());
    Position {
        line: line_0,
        character: char_to_utf16(line_text, char_idx),
    }
}

/// Identifier characters for symbol lookup.
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Identifier under a 0-based cursor (UTF-16 column), with its
/// 0-based UTF-16 column span. Operates on characters throughout.
pub fn word_at(text: &str, line: u32, character: u32) -> Option<(String, u32, u32)> {
    let line_text = text.split('\n').nth(line as usize)?;
    let chars: Vec<char> = line_text.chars().collect();
    let mut col = utf16_to_char(line_text, character).min(chars.len());
    // A cursor just past a word (or on punctuation) steps back onto
    // the word when possible, like editors do.
    if col > 0 && (col >= chars.len() || !is_word_char(chars[col])) {
        if !is_word_char(chars[col - 1]) {
            return None;
        }
        col -= 1;
    }
    if col >= chars.len() || !is_word_char(chars[col]) {
        return None;
    }
    let mut start = col;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < chars.len() && is_word_char(chars[end]) {
        end += 1;
    }
    let start_u16 = char_to_utf16(line_text, start);
    let end_u16 = char_to_utf16(line_text, end);
    let text: String = chars[start..end].iter().collect();
    Some((text, start_u16, end_u16))
}

/// Source range of a defining name on a 1-based line: the first
/// post-indent occurrence with identifier boundaries. Statement
/// heads are single-line by grammar, so the key always starts its
/// line (after `[` for headers). Keys are identifiers (ASCII), so
/// byte search is exact; emitted columns are UTF-16.
pub fn key_range_on_line(text: &str, line_1based: u32, key: &str) -> Option<Range> {
    key_range_on_line_before(text, line_1based, key, usize::MAX, true)
}

/// Same, but only occurrences ending at or before `max_end_char`
/// (0-based character count) qualify, and the *last* qualifier wins:
/// the key immediately precedes its value, so anchoring on the value
/// span's start disambiguates same-line duplicates (inline tables
/// like `[{p = 1}, {p = 2}]`). With no qualifier the first match
/// wins, preserving the unanchored contract.
fn key_range_on_line_before(
    text: &str,
    line_1based: u32,
    key: &str,
    max_end_char: usize,
    first_wins: bool,
) -> Option<Range> {
    let line_text = text.split('\n').nth(line_1based.checked_sub(1)? as usize)?;
    let indent = line_text.len() - line_text.trim_start().len();
    let mut search_from = indent;
    let mut found = None;
    while let Some(rel) = line_text[search_from..].find(key) {
        let start = search_from + rel;
        let after = start + key.len();
        let boundary = line_text
            .as_bytes()
            .get(after)
            .is_none_or(|c| !is_word_char(*c as char));
        if boundary {
            // Byte offsets here are character offsets: everything up
            // to and including the ASCII key is single-byte.
            let start_char = line_text[..start].chars().count();
            let end_char = start_char + key.chars().count();
            if end_char <= max_end_char {
                found = Some(Range {
                    start: Position {
                        line: line_1based.saturating_sub(1),
                        character: char_to_utf16(line_text, start_char),
                    },
                    end: Position {
                        line: line_1based.saturating_sub(1),
                        character: char_to_utf16(line_text, end_char),
                    },
                });
                if first_wins {
                    return found;
                }
            }
        }
        search_from = start + 1;
    }
    found
}

// ------------------------------------------------------------------
// Symbols: definition targets.
// ------------------------------------------------------------------

/// A resolvable symbol: a `let` binding or a table key.
#[derive(Debug, Clone)]
pub enum Target {
    Let { name: String, node: NodeId },
    Key { path: String, node: NodeId },
}

/// A brace token mark: (start, end, is_open) in 1-based character
/// coordinates for lexical scope-boundary detection.
type BraceMark = ((u32, u32), (u32, u32), bool);

/// A table scope derived from node spans: header start through the
/// deepest descendant end, at (line, column) precision. Mirrors
/// evaluator name resolution (siblings, then outer scopes, then
/// `let` bindings). Column precision matters: same-line sibling
/// blocks (`server { host = "a" } client { host = "b" }`) overlap in
/// lines but not in columns. Profile bodies are separate arena
/// trees and resolve as document scope — an approximation documented
/// in spec/lsp.md.
struct Scope {
    node: NodeId,
    path: String,
    depth: usize,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
}

/// Whether a node belongs to this document: unstamped nodes do;
/// stamped nodes must match the document's own file. Includes merge
/// into one module with lifted spans that share the line space, so
/// file identity — not position — separates their scopes and values.
fn node_in_doc(doc: &Doc, id: NodeId) -> bool {
    match (doc.uri.to_file_path().ok(), doc.module.node_files.get(&id)) {
        (_, None) => true,
        (Some(main), Some(stamped)) => stamped == &main.to_string_lossy().into_owned(),
        (None, Some(_)) => false,
    }
}

/// All table scopes with computed extents. The root clamps to line 1
/// so leading comments and gaps still resolve. Scopes from other
/// files (includes merged into this module, whose lifted spans share
/// the line space) are excluded: the cursor lives in this document,
/// and an overlapping line number in an included file must never
/// capture resolution. Nodes without a file stamp belong to this
/// document.
fn scopes(doc: &Doc) -> Vec<Scope> {
    let module = &doc.module;
    let mut out = Vec::new();
    let Some(root) = module.root else {
        return out;
    };
    // (node, path, depth); extents computed bottom-up after the walk.
    let mut order: Vec<(NodeId, String, usize)> = Vec::new();
    let mut stack = vec![(root, String::new(), 0usize)];
    while let Some((id, path, depth)) = stack.pop() {
        if !node_in_doc(doc, id) {
            continue;
        }
        if let Some(Node::Table { fields }) = module.get(id) {
            order.push((id, path.clone(), depth));
            let mut nested: Vec<_> = fields.iter().collect();
            nested.reverse();
            for (key, child) in nested {
                if matches!(module.get(*child), Some(Node::Table { .. })) {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    stack.push((*child, child_path, depth + 1));
                }
            }
        }
    }
    // End positions bottom-up: a scope ends at its deepest
    // descendant's end. Other-file children never extend an extent:
    // their lifted spans live in this document's line space but
    // describe other text.
    let mut ends: HashMap<NodeId, (u32, u32)> = HashMap::new();
    for (id, _, _) in order.iter().rev() {
        let mut end = module
            .node_spans
            .get(id)
            .map(|s| (s.end_line, s.end_column))
            .unwrap_or((1, 1));
        if let Some(Node::Table { fields }) = module.get(*id) {
            for child in fields.values() {
                if !node_in_doc(doc, *child) {
                    continue;
                }
                end = end.max(
                    ends.get(child)
                        .copied()
                        .or_else(|| {
                            module
                                .node_spans
                                .get(child)
                                .map(|s| (s.end_line, s.end_column))
                        })
                        .unwrap_or(end),
                );
            }
        }
        ends.insert(*id, end);
    }
    for (id, path, depth) in order {
        let (start_line, start_col) = if depth == 0 {
            // The root clamps to the document start so leading
            // comments and gaps still resolve.
            (1, 1)
        } else {
            module
                .node_spans
                .get(&id)
                .map(|s| (s.line, s.column))
                .unwrap_or((1, 1))
        };
        let (end_line, end_col) = *ends.get(&id).unwrap_or(&(start_line, start_col));
        out.push(Scope {
            node: id,
            path,
            depth,
            start_line,
            start_col,
            end_line,
            end_col,
        });
    }
    // Lexical extension: a scope runs to its table's lexical
    // boundary, so fresh lines inside an open table — including
    // empty tables — still resolve and complete in that table.
    // Brace tables (`server { … }`) end at the matching close brace
    // (depth-counted over the lexer stream from the table's own
    // `{`); header tables (`[server]`, `server:`) run until the next
    // scope at their own depth or shallower begins, or the document
    // end. The root spans the whole document; deeper scopes never
    // bound their parents. (Overreach note: a dedented trailing key
    // after an indent-style block still falls inside the extended
    // scope — name resolution falls through to the outer scope
    // correctly, so only the completion source may differ. Documented
    // in spec/lsp.md §4.)
    let mut starts: Vec<((u32, u32), usize)> = out
        .iter()
        .map(|s| ((s.start_line, s.start_col), s.depth))
        .collect();
    starts.sort();
    let mut braces: Vec<BraceMark> = Vec::new();
    {
        use solid_core::parser::TokenKind as K;
        let mut lexer = solid_core::parser::Lexer::new(&doc.text);
        for tok in lexer.tokenize().tokens {
            match tok.kind {
                K::LBrace => braces.push(((tok.line, tok.col), (tok.end_line, tok.end_col), true)),
                K::RBrace => {
                    braces.push(((tok.line, tok.col), (tok.end_line, tok.end_col), false));
                }
                _ => {}
            }
        }
    }
    for scope in &mut out {
        if scope.depth == 0 {
            scope.end_line = u32::MAX;
            scope.end_col = u32::MAX;
            continue;
        }
        let start = (scope.start_line, scope.start_col);
        let bound = starts
            .iter()
            .find(|(s, d)| *s > start && *d <= scope.depth && *d > 0)
            .map(|(s, _)| *s);
        // The table's own `{`, if brace-style. Inline tables span
        // their opening brace, so a brace exactly AT the scope start
        // is the table's own; statement tables take the brace on the
        // name's own line past the name end (the grammar requires it
        // there). Anything later opens a nested block, not this
        // table — header tables (`[server]`, `server:`) match
        // nothing here.
        let name_len = scope
            .path
            .rsplit('.')
            .next()
            .map(|s| s.chars().count())
            .unwrap_or(0) as u32;
        let name_end = (scope.start_line, scope.start_col + name_len);
        let own_open = braces.iter().find(|(pos, _, is_open)| {
            *is_open && (*pos == start || (*pos >= name_end && pos.0 == scope.start_line))
        });
        let mut end = (scope.end_line, scope.end_col);
        // Header-style fallback extent: the line before the next
        // scope, or the document end. Brace tables with no matching
        // close (broken input) degrade to this rather than losing
        // their trailing lines.
        let header_ext = match bound {
            Some(limit) => {
                if limit.1 > 1 {
                    (limit.0, limit.1 - 1)
                } else if limit.0 > 1 {
                    (limit.0 - 1, u32::MAX)
                } else {
                    start
                }
            }
            None => (u32::MAX, u32::MAX),
        };
        match own_open {
            // Brace-style, with its `{` before any following scope:
            // close at the depth-matched `}` (never crossing into
            // the next scope's braces on broken input).
            Some((open_pos, _, _)) if bound.is_none_or(|limit| *open_pos < limit) => {
                let mut brace_depth = 0u32;
                let mut closed = false;
                for (pos, epos, is_open) in &braces {
                    if *pos < *open_pos {
                        continue;
                    }
                    if bound.is_some_and(|limit| *pos >= limit) {
                        break;
                    }
                    if *is_open {
                        brace_depth += 1;
                    } else if brace_depth > 0 {
                        brace_depth -= 1;
                        if brace_depth == 0 {
                            end = end.max(*epos);
                            closed = true;
                            break;
                        }
                    }
                }
                if !closed {
                    end = end.max(header_ext);
                }
            }
            _ => {
                end = end.max(header_ext);
            }
        }
        (scope.end_line, scope.end_col) = end;
    }
    out
}

/// Resolve the identifier under the cursor: innermost containing
/// scope's fields first, then outer scopes, then `let` bindings —
/// the evaluator's own precedence (siblings before lets).
pub fn symbol_at(doc: &Doc, line: u32, character: u32) -> Option<(String, Target)> {
    let (word, start_u16, _) = word_at(&doc.text, line, character)?;
    // Words inside comments and string literals are not symbols: the
    // word must sit on a main-stream identifier token or an
    // interpolation reference. Without this, hover/definition — and
    // worse, rename — can be initiated from prose that merely
    // mentions a real name.
    if !on_ident_token(doc, line, start_u16, &word) {
        return None;
    }
    let cursor_line = line + 1;
    let cursor_col = doc
        .text
        .split('\n')
        .nth(line as usize)
        .map(|l| utf16_to_char(l, character) as u32 + 1)
        .unwrap_or(1);
    let mut containing: Vec<&Scope> = Vec::new();
    // Scopes computed per call: documents are small and calls are
    // user-paced; clarity over caching (revisit with C-055 data).
    let all = scopes(doc);
    for scope in &all {
        if (scope.start_line, scope.start_col) <= (cursor_line, cursor_col)
            && (cursor_line, cursor_col) <= (scope.end_line, scope.end_col)
        {
            containing.push(scope);
        }
    }
    // Innermost first; ties (identical extents) break by path for
    // determinism.
    containing.sort_by(|a, b| b.depth.cmp(&a.depth).then_with(|| a.path.cmp(&b.path)));
    for scope in containing {
        if let Some(Node::Table { fields }) = doc.module.get(scope.node) {
            if let Some(child) = fields.get(&word) {
                let path = if scope.path.is_empty() {
                    word.clone()
                } else {
                    format!("{}.{word}", scope.path)
                };
                return Some((word, Target::Key { path, node: *child }));
            }
        }
    }
    if let Some(node) = doc.module.bindings.get(&word) {
        return Some((
            word.clone(),
            Target::Let {
                name: word,
                node: *node,
            },
        ));
    }
    None
}

/// Whether the word starting at a 0-based UTF-16 column sits on a
/// real identifier: a main-stream `Ident` token with the same text,
/// or an interpolation reference (`Ref` node, sub-lexed by the
/// parser with source-rebased spans) whose target's leaf name
/// matches. Comments, string contents, numbers, and keywords fail.
fn on_ident_token(doc: &Doc, line: u32, start_u16: u32, word: &str) -> bool {
    let Some(line_text) = doc.text.split('\n').nth(line as usize) else {
        return false;
    };
    // 1-based (line, character column) of the word start.
    let target = (line + 1, utf16_to_char(line_text, start_u16) as u32 + 1);
    let mut lexer = solid_core::parser::Lexer::new(&doc.text);
    let lexed = lexer.tokenize();
    for tok in &lexed.tokens {
        if !matches!(tok.kind, solid_core::parser::TokenKind::Ident) || tok.text != word {
            continue;
        }
        if tok.line == target.0 && tok.col <= target.1 && target.1 < tok.end_col {
            return true;
        }
    }
    // Interpolation uses live outside the main token stream.
    let mut names: HashMap<NodeId, String> = HashMap::new();
    for (name, id) in &doc.module.bindings {
        names.insert(*id, name.clone());
    }
    for (id, path) in doc.module.value_paths() {
        names
            .entry(id)
            .or_insert_with(|| path.rsplit('.').next().unwrap_or(&path).to_string());
    }
    for (i, node) in doc.module.arena.iter().enumerate() {
        let Node::Ref(target_id) = node else {
            continue;
        };
        if names.get(target_id).is_none_or(|n| n != word) {
            continue;
        }
        if let Some(span) = doc.module.node_spans.get(&NodeId(i as u32)) {
            if span.line == target.0 && span.column <= target.1 && target.1 < span.end_column {
                return true;
            }
        }
    }
    false
}

/// Owning key (or binding) for a cursor on a literal value: the
/// smallest value-node span containing the position. Powers hover on
/// non-identifier positions (spec/lsp.md §2).
fn owner_at(doc: &Doc, line: u32, character: u32) -> Option<(String, NodeId)> {
    let cursor_line = line + 1;
    // Character-based cursor column for span comparison.
    let line_text = doc.text.split('\n').nth(line as usize)?;
    let cursor_char = {
        let mut units = 0u32;
        let mut idx = 0usize;
        for (i, c) in line_text.chars().enumerate() {
            if units >= character {
                idx = i;
                break;
            }
            units += c.len_utf16() as u32;
            idx = i + 1;
        }
        idx
    };
    // (node, path, span area): bindings plus every table value in
    // this document. Included-file nodes are excluded (as in
    // `scopes`): their lifted spans share the line space but describe
    // other text, and must never win over local values.
    let mut candidates: Vec<(NodeId, String, u64)> = Vec::new();
    let mut consider = |id: NodeId, path: String| {
        if !node_in_doc(doc, id) {
            return;
        }
        let span = match doc.module.node_spans.get(&id) {
            Some(span) => span,
            None => return,
        };
        // Containment in character space (multi-line spans included).
        let starts_before = span.line < cursor_line
            || (span.line == cursor_line && span.column.saturating_sub(1) as usize <= cursor_char);
        let ends_after = span.end_line > cursor_line
            || (span.end_line == cursor_line
                && (span.end_column.saturating_sub(1) as usize) >= cursor_char);
        if starts_before && ends_after {
            let area = (span.end_line - span.line) as u64 * 1_000_000
                + (span.end_column.saturating_sub(span.column)) as u64;
            candidates.push((id, path, area));
        }
    };
    for (name, id) in &doc.module.bindings {
        consider(*id, name.clone());
    }
    if let Some(root) = doc.module.root {
        let mut stack = vec![(root, String::new())];
        while let Some((id, path)) = stack.pop() {
            if let Some(Node::Table { fields }) = doc.module.get(id) {
                for (key, child) in fields {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    consider(*child, child_path.clone());
                    stack.push((*child, child_path));
                }
            }
        }
    }
    candidates.sort_by_key(|a| a.2);
    candidates
        .into_iter()
        .next()
        .map(|(id, path, _)| (path, id))
}

/// Defining range for a target in the owning document's text.
fn def_range(doc: &Doc, target: &Target) -> Option<Range> {
    let (name, node) = match target {
        Target::Let { name, node } => (name.as_str(), *node),
        Target::Key { path, node } => (path.rsplit('.').next().unwrap_or(path), *node),
    };
    let span = doc.module.node_spans.get(&node)?;
    // Table spans cover the name token: anchor on the end (see
    // `definition_at`).
    let anchor_col = match doc.module.get(node) {
        Some(Node::Table { .. }) => span.end_column,
        _ => span.column,
    };
    let anchor = anchor_col.saturating_sub(1) as usize;
    key_range_on_line_before(&doc.text, span.line, name, anchor, false)
        .or_else(|| key_range_on_line(&doc.text, span.line, name))
}

// ------------------------------------------------------------------
// Cross-file text.
// ------------------------------------------------------------------

/// File-identity comparison for paths: exact, case-insensitive
/// (Windows client URIs vs canonical stamps), then filesystem
/// canonicalization (8.3 short-names vs long names — the CI Temp
/// lesson). Unresolvable paths fall back to string equality.
fn paths_equal(a: &str, b: &str) -> bool {
    if a == b || a.eq_ignore_ascii_case(b) {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// URI-identity comparison for published groups: same string, or
/// same file by `paths_equal`. Non-file URIs (untitled:) compare by
/// string only — there is no filesystem identity to consult.
pub fn same_uri(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (Ok(ua), Ok(ub)) = (Url::parse(a), Url::parse(b)) else {
        return false;
    };
    match (ua.to_file_path(), ub.to_file_path()) {
        (Ok(pa), Ok(pb)) => paths_equal(&pa.to_string_lossy(), &pb.to_string_lossy()),
        _ => false,
    }
}

/// Version of the open buffer behind a URI: exact URI match first,
/// then file-identity matching. Unknown files yield None (the
/// publisher omits the version rather than stamping another file's).
pub fn open_version(open: &HashMap<String, Doc>, uri: &Url) -> Option<i32> {
    if let Some(doc) = open.get(&uri.to_string()) {
        return Some(doc.version);
    }
    let target = uri.to_file_path().ok()?;
    let target_str = target.to_string_lossy();
    let mut fuzzy = None;
    for doc in open.values() {
        if let Ok(p) = doc.uri.to_file_path() {
            let candidate = p.to_string_lossy();
            if candidate == target_str {
                return Some(doc.version);
            }
            if fuzzy.is_none() && paths_equal(&candidate, &target_str) {
                fuzzy = Some(doc.version);
            }
        }
    }
    fuzzy
}

/// Text lookup by file path: open buffers first, then disk (saved
/// state). Unreadable files yield None and their references degrade
/// gracefully (node-span ranges still work — spans are file-local).
/// Buffer matching is by file identity: include stamps carry
/// canonical paths that may differ textually from the client's URI
/// for the same file, and a live buffer always beats stale disk.
pub fn lookup_text(open: &HashMap<String, Doc>, path: &str) -> Option<String> {
    let mut fuzzy = None;
    for doc in open.values() {
        if let Ok(p) = doc.uri.to_file_path() {
            let candidate = p.to_string_lossy();
            if candidate == path {
                return Some(doc.text.clone());
            }
            if fuzzy.is_none() && paths_equal(&candidate, path) {
                fuzzy = Some(doc.text.clone());
            }
        }
    }
    fuzzy.or_else(|| std::fs::read_to_string(path).ok())
}

/// File URI for a node, falling back to the owning document.
pub fn uri_for_node(doc: &Doc, node: NodeId) -> Url {
    if let Some(file) = doc.module.node_files.get(&node) {
        if let Ok(uri) = Url::from_file_path(file) {
            return uri;
        }
    }
    doc.uri.clone()
}

/// Text owning a URI: the open buffer when it is this document,
/// otherwise the lookup (other open buffers, then disk). Reference
/// sub-spans need source text; node-span ranges do not.
fn text_for_uri(doc: &Doc, lookup: &impl Fn(&str) -> Option<String>, uri: &Url) -> Option<String> {
    if *uri == doc.uri {
        return Some(doc.text.clone());
    }
    let path = uri.to_file_path().ok()?.to_string_lossy().into_owned();
    lookup(&path)
}

// ------------------------------------------------------------------
// Hover (C-043, spec/lsp.md §2).
// ------------------------------------------------------------------

/// Value kind name for the hover type fallback (spec/lsp.md §2).
/// `Secret` names the kind only — the value itself always renders
/// redacted through `render_hover_value`.
fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Secret(_) => "Secret",
        Value::String(_) => "String",
        Value::Int(_) => "Int",
        Value::Float(_) => "Float",
        Value::Bool(_) => "Bool",
        Value::Null => "Null",
        Value::DurationMs(_) => "Duration",
        Value::Quantity(_, _) => "Quantity",
        Value::Array(_) => "Array",
        Value::Table(_) => "Table",
    }
}

/// Short safe rendering for hover values: scalars inline (redacted),
/// composites by kind only.
fn render_hover_value(value: &Value) -> String {
    match value {
        Value::Secret(_) => "***SECRET***".to_string(),
        Value::String(s) => {
            let quoted = format!("{s:?}");
            if quoted.chars().count() > 80 {
                format!("{}…", quoted.chars().take(79).collect::<String>())
            } else {
                quoted
            }
        }
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::DurationMs(ms) => format!("{ms} ms"),
        Value::Quantity(n, unit) => format!("{n} {}", unit.suffix()),
        Value::Array(items) => format!("array[{}]", items.len()),
        Value::Table(map) => format!("table{{{}}}", map.len()),
    }
}

/// Value at a dotted table path in the evaluated tree (tables only).
fn value_at_path<'v>(value: &'v Value, path: &str) -> Option<&'v Value> {
    let mut cursor = value;
    for segment in path.split('.') {
        match cursor {
            Value::Table(map) => cursor = map.get(segment)?,
            _ => return None,
        }
    }
    Some(cursor)
}

/// Schema type string at a dotted path (`Int`, `Array<String>`,
/// `Table`, …), or None when no schema covers it.
fn schema_type_at(module: &Module, path: &str) -> Option<String> {
    fn render(module: &Module, id: NodeId) -> String {
        match module.get(id) {
            Some(Node::SchemaField { field }) => field.typ.name(),
            Some(Node::SchemaArray { element }) => format!("Array<{}>", render(module, *element)),
            Some(Node::SchemaTable { .. }) => "Table".to_string(),
            _ => "Any".to_string(),
        }
    }
    let mut id = module.schema?;
    if path.is_empty() {
        return Some(render(module, id));
    }
    for segment in path.split('.') {
        match module.get(id) {
            Some(Node::SchemaTable { fields, .. }) => id = *fields.get(segment)?,
            _ => return None,
        }
    }
    Some(render(module, id))
}

/// Owning key (or binding) as a target for non-identifier
/// positions: literals and string contents resolve to the key whose
/// value contains them.
fn owner_target(doc: &Doc, line: u32, character: u32) -> Option<(String, Target)> {
    let (path, node) = owner_at(doc, line, character)?;
    let name = path.rsplit('.').next().unwrap_or(&path).to_string();
    let target = if doc.module.bindings.values().any(|id| *id == node) {
        Target::Let {
            name: name.clone(),
            node,
        }
    } else {
        Target::Key { path, node }
    };
    Some((name, target))
}

pub fn hover_at(doc: &Doc, line: u32, character: u32) -> Option<Hover> {
    // Identifiers resolve by scope; anything else (literals,
    // punctuation-adjacent values) falls back to the owning key.
    let (name, target) = match symbol_at(doc, line, character) {
        Some(found) => found,
        None => owner_target(doc, line, character)?,
    };
    let (path, node) = match &target {
        Target::Let { name, node } => (name.clone(), *node),
        Target::Key { path, node } => (path.clone(), *node),
    };
    let mut lines: Vec<String> = Vec::new();
    // Type: schema first, evaluated value kind as fallback (spec §2
    // requires one or the other — never a bare name when a value
    // exists).
    if let Some(typ) = schema_type_at(&doc.module, &path) {
        lines.push(format!("**{name}**: `{typ}`"));
    } else if let Target::Let { .. } = target {
        lines.push(format!("**{name}** (let binding)"));
    } else {
        match doc
            .value
            .as_ref()
            .and_then(|root| value_at_path(root, &path))
        {
            Some(value) => lines.push(format!("**{name}**: `{}`", value_kind(value))),
            None => lines.push(format!("**{name}**")),
        }
    }
    // Value at the path (redacted; lets have no tree value).
    if let Some(root) = doc.value.as_ref() {
        if let Some(value) = value_at_path(root, &path) {
            lines.push(format!("= {}", render_hover_value(value)));
        }
    }
    // Origin, include-aware.
    if let Some(span) = doc.module.node_spans.get(&node) {
        let origin = doc
            .module
            .node_files
            .get(&node)
            .cloned()
            .unwrap_or_else(|| doc.uri.to_string());
        lines.push(format!("defined at `{origin}:{}`", span.line));
    }
    // Annotations: intent, constraints, flags.
    if let Some(anns) = doc.module.annotations.get(&node) {
        for ann in anns {
            match &ann.kind {
                AnnotationKind::Intent(text) => lines.push(format!("> {text}")),
                AnnotationKind::Constraint { source, .. } => {
                    lines.push(format!("constraint: `{source}`"));
                }
                AnnotationKind::AiImmutable(true) => lines.push("🔒 `ai_immutable`".to_string()),
                AnnotationKind::AiImmutable(false) => {}
            }
        }
    }
    // Schema deprecation flag at the path.
    if let Some(reason) = schema_deprecated_at(&doc.module, &path) {
        if reason.is_empty() {
            lines.push("⚠️ deprecated".to_string());
        } else {
            lines.push(format!("⚠️ deprecated: {reason}"));
        }
    }
    // Active lint diagnostics on this exact path, if any.
    for diag in doc
        .lint_diags
        .iter()
        .filter(|d| d.path.as_deref() == Some(path.as_str()))
    {
        lines.push(format!("{}: {}", diag.code, diag.message));
    }
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: lines.join("\n\n"),
        }),
        range: def_range(doc, &target),
    })
}

/// Deprecated flag (`Some(reason)`, empty = bare) at a schema path.
fn schema_deprecated_at(module: &Module, path: &str) -> Option<String> {
    let mut id = module.schema?;
    let mut segments = path.split('.').peekable();
    loop {
        match module.get(id) {
            Some(Node::SchemaTable { fields, .. }) => {
                let segment = segments.next()?;
                id = *fields.get(segment)?;
            }
            Some(Node::SchemaField { field }) => {
                if segments.next().is_some() {
                    return None;
                }
                return field.deprecated.clone();
            }
            _ => return None,
        }
    }
}

// ------------------------------------------------------------------
// Definition, references, rename (C-043, spec/lsp.md §3).
// ------------------------------------------------------------------

pub fn definition_at(
    doc: &Doc,
    lookup: &impl Fn(&str) -> Option<String>,
    line: u32,
    character: u32,
) -> Vec<Location> {
    // Identifiers resolve by scope; value positions fall back to the
    // owning key (spec §3 promises definition for values).
    let Some((_, target)) =
        symbol_at(doc, line, character).or_else(|| owner_target(doc, line, character))
    else {
        return Vec::new();
    };
    let (node, name) = match &target {
        Target::Let { name, node } => (*node, name.as_str()),
        Target::Key { path, node } => (*node, path.rsplit('.').next().unwrap_or(path.as_str())),
    };
    let uri = uri_for_node(doc, node);
    // Key range needs the defining file's text; the search anchors
    // on the value span's start so same-line duplicates resolve to
    // the right declaration (unanchored first match as fallback).
    // Table nodes span their name token rather than a value, so they
    // anchor on the span end instead: anchoring on the start would
    // exclude the name itself and select an earlier duplicate.
    // Without text, the value-node span range still works — spans
    // are file-local.
    let range = text_for_uri(doc, lookup, &uri)
        .and_then(|text| {
            doc.module.node_spans.get(&node).and_then(|span| {
                let anchor_col = match doc.module.get(node) {
                    Some(Node::Table { .. }) => span.end_column,
                    _ => span.column,
                };
                let anchor = anchor_col.saturating_sub(1) as usize;
                key_range_on_line_before(&text, span.line, name, anchor, false)
                    .or_else(|| key_range_on_line(&text, span.line, name))
            })
        })
        .or_else(|| {
            doc.module.node_spans.get(&node).and_then(|span| {
                text_for_uri(doc, lookup, &uri).map(|text| range_of_span(&text, span))
            })
        });
    range
        .map(|range| vec![Location { uri, range }])
        .unwrap_or_default()
}

/// All reference ranges for a target node: resolved `Ref`s anywhere
/// in the merged arena (includes included — spans stay file-local),
/// plus `depends_on` string elements and patch source-path strings
/// that resolve to it. Unresolved idents are excluded: renaming one
/// would change its meaning.
fn reference_ranges(
    doc: &Doc,
    lookup: &impl Fn(&str) -> Option<String>,
    target: NodeId,
) -> Vec<(Url, Range)> {
    let mut out: Vec<(Url, Range)> = Vec::new();
    for (i, node) in doc.module.arena.iter().enumerate() {
        let id = NodeId(i as u32);
        match node {
            Node::Ref(t) if *t == target => {
                if let Some(span) = doc.module.node_spans.get(&id) {
                    let uri = uri_for_node(doc, id);
                    // Ranges need the owning file's text for UTF-16
                    // columns; without it the reference is skipped
                    // rather than mis-ranged (rename safety).
                    if let Some(text) = text_for_uri(doc, lookup, &uri) {
                        out.push((uri, range_of_span(&text, span)));
                    }
                }
            }
            Node::PatchDirective { op } => {
                // Patch source paths are string-encoded references.
                // The directive span covers only `@`; the helper
                // re-derives the source-path argument and sub-spans
                // the segments that resolve to the target.
                if let Some(span) = doc.module.node_spans.get(&id) {
                    let uri = uri_for_node(doc, id);
                    if let Some(text) = text_for_uri(doc, lookup, &uri) {
                        for range in patch_reference_ranges(&text, span, op, target, &doc.module) {
                            out.push((uri.clone(), range));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // `depends_on` string elements with the same treatment. The
    // DependsOn span covers the array's `[`: element strings scan
    // forward from there, bounded by the element count.
    for (table, dep) in &doc.module.depends_on {
        for (element_index, path) in dep.paths.iter().enumerate() {
            // Prefix resolution: segment `i` refers to the node its
            // dot-prefix resolves to (paths are root-relative, like
            // the resolver). Only prefixes resolving to the target
            // are references of it — a same-named key in another
            // table never matches.
            let mut prefix = String::new();
            for (seg_index, segment) in path.split('.').enumerate() {
                if !prefix.is_empty() {
                    prefix.push('.');
                }
                prefix.push_str(segment);
                if doc.module.resolve_value_path(&prefix) != Some(target) {
                    continue;
                }
                if let Some(range) = depends_on_segment_range(
                    doc,
                    lookup,
                    *table,
                    &dep.span,
                    element_index,
                    seg_index,
                ) {
                    out.push((uri_for_node(doc, *table), range));
                }
            }
        }
    }
    out
}

/// Sub-spans of the segments of a patch directive's *source* path
/// that resolve to `target`. The directive span covers only `@`, so
/// the source-path argument — always argument 0 (`@patch`/`@remove`
/// take one path; `@rename`'s second argument is a destination name,
/// never a reference) — is re-derived by scanning `@name (` plus one
/// quoted string with backslash-escape awareness. Decoded characters
/// carry their absolute source offsets, so escapes never shift
/// ranges. Each dot-prefix of the decoded path resolves from the
/// document root; only prefixes resolving to `target` yield ranges
/// (a same-named key in another table never matches). Without text
/// (or on any shape surprise) the reference is skipped, never
/// mis-ranged.
fn patch_reference_ranges(
    text: &str,
    span: &solid_core::diag::Span,
    op: &solid_core::PatchOp,
    target: NodeId,
    module: &Module,
) -> Vec<Range> {
    let mut out = Vec::new();
    // Directive spans are 1-based character coordinates
    // (core_offset, never offset_of's UTF-16 frame).
    let base = core_offset(text, span.line, span.column);
    let rest = match text.get(base..) {
        Some(rest) => rest,
        None => return out,
    };
    // Directive name: `@` + word.
    let mut chars = rest.char_indices().peekable();
    if chars.next().map(|(_, c)| c) != Some('@') {
        return out;
    }
    let mut opname = String::new();
    while let Some(&(_, c)) = chars.peek() {
        if c.is_ascii_alphanumeric() || c == '_' {
            opname.push(c);
            chars.next();
        } else {
            break;
        }
    }
    // Sanity: the directive word matches the IR op; otherwise the
    // text and the tree disagree and no range is safe.
    match (opname.as_str(), op) {
        ("patch", solid_core::PatchOp::Set { .. })
        | ("remove", solid_core::PatchOp::Remove { .. })
        | ("rename", solid_core::PatchOp::Rename { .. }) => {}
        _ => return out,
    }
    // The source path is always argument 0: skip whitespace, expect
    // `(`, skip whitespace, then read exactly one quoted string.
    // Anything else means the text and the tree disagree — skip.
    skip_ws(&mut chars);
    if chars.next().map(|(_, c)| c) != Some('(') {
        return out;
    }
    skip_ws(&mut chars);
    let quote = match chars.next() {
        Some((_, q @ ('"' | '\''))) => q,
        _ => return out,
    };
    // (absolute byte offset, byte length, char) per decoded char, so
    // escapes never shift the ranges below.
    let mut content: Vec<(usize, usize, char)> = Vec::new();
    let mut escaped = false;
    let closed = loop {
        match chars.next() {
            None => break false,
            Some((off, c)) => {
                if escaped {
                    content.push((base + off, c.len_utf8(), c));
                    escaped = false;
                    continue;
                }
                if c == '\\' {
                    escaped = true;
                    continue;
                }
                if c == quote {
                    break true;
                }
                if c == '\n' {
                    break false;
                }
                content.push((base + off, c.len_utf8(), c));
            }
        }
    };
    if !closed {
        return out;
    }
    // Split the decoded path into segments with record ranges.
    let mut segments: Vec<(String, usize, usize)> = Vec::new();
    let mut seg_start = 0usize;
    for (i, &(_, _, c)) in content.iter().enumerate() {
        if c == '.' {
            segments.push((
                content[seg_start..i].iter().map(|(_, _, c)| *c).collect(),
                seg_start,
                i,
            ));
            seg_start = i + 1;
        }
    }
    segments.push((
        content[seg_start..].iter().map(|(_, _, c)| *c).collect(),
        seg_start,
        content.len(),
    ));
    // Prefix resolution: segment `i` refers to the node its
    // dot-prefix resolves to. Only target prefixes yield ranges.
    let mut prefix = String::new();
    for (decoded, s, e) in &segments {
        if !prefix.is_empty() {
            prefix.push('.');
        }
        prefix.push_str(decoded);
        if module.resolve_value_path(&prefix) != Some(target) {
            continue;
        }
        if s == e {
            continue;
        }
        let abs_start = content[*s].0;
        let last = &content[e - 1];
        let abs_end = last.0 + last.1;
        out.push(Range {
            start: position_of(text, abs_start),
            end: position_of(text, abs_end),
        });
    }
    out
}

/// Skip ASCII whitespace (spaces, newlines) in a directive scan.
fn skip_ws(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) {
    while matches!(chars.peek(), Some(&(_, c)) if c.is_whitespace()) {
        chars.next();
    }
}

/// Same as above, but the anchor span covers a whole `depends_on`
/// array: find the `element_index`-th string first, then the segment
/// within it. The caller gates correctness by resolving the element
/// path to the target node; this only locates the segment range
/// (element paths are plain dotted idents — escapes inside them are
/// out of scope and fall back to no range via the payload checks).
fn depends_on_segment_range(
    doc: &Doc,
    lookup: &impl Fn(&str) -> Option<String>,
    _table: NodeId,
    span: &solid_core::diag::Span,
    element_index: usize,
    seg_index: usize,
) -> Option<Range> {
    let uri = uri_for_node(doc, _table);
    let text = text_for_uri(doc, lookup, &uri)?;
    // Array span starts at `[` (character coordinates, as above):
    // scan string literals in order.
    let start = core_offset(&text, span.line, span.column);
    let rest = text.get(start..)?;
    let mut found = 0usize;
    let mut chars = rest.char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        if c != '"' && c != '\'' {
            continue;
        }
        let quote = c;
        let content_start = offset + quote.len_utf8();
        // Find the closer, honoring backslash escapes.
        let mut escaped = false;
        let mut end = None;
        for (inner_offset, inner) in chars.by_ref() {
            if escaped {
                escaped = false;
                continue;
            }
            if inner == '\\' {
                escaped = true;
                continue;
            }
            if inner == quote {
                end = Some(inner_offset);
                break;
            }
        }
        let end = end?;
        if found == element_index {
            let payload = rest.get(content_start..end)?;
            let mut seg_start = 0usize;
            for (index, segment) in payload.split('.').enumerate() {
                if index == seg_index {
                    let abs_start = start + content_start + seg_start;
                    let abs_end = abs_start + segment.len();
                    let s = position_of(&text, abs_start);
                    let e = position_of(&text, abs_end);
                    return Some(Range { start: s, end: e });
                }
                seg_start += segment.len() + 1;
            }
            return None;
        }
        found += 1;
    }
    None
}

pub fn references_at(
    doc: &Doc,
    lookup: &impl Fn(&str) -> Option<String>,
    line: u32,
    character: u32,
    include_declaration: bool,
) -> Vec<Location> {
    let Some((_, target)) = symbol_at(doc, line, character) else {
        return Vec::new();
    };
    let node = match &target {
        Target::Let { node, .. } | Target::Key { node, .. } => *node,
    };
    let mut out: Vec<Location> = Vec::new();
    if include_declaration {
        out.extend(definition_at(doc, lookup, line, character));
    }
    for (uri, range) in reference_ranges(doc, lookup, node) {
        // The declaration itself is not a use (definition covers it
        // when requested).
        out.push(Location { uri, range });
    }
    // Deterministic: sort by file, then position.
    out.sort_by(|a, b| {
        a.uri
            .as_str()
            .cmp(b.uri.as_str())
            .then(a.range.start.line.cmp(&b.range.start.line))
            .then(a.range.start.character.cmp(&b.range.start.character))
    });
    out
}

/// Validate an identifier for rename targets. Beyond the shape
/// rule, reserved spellings are refused: hard keywords (`let`, `env`,
/// `secret`, `true`, `false`, `null` lex as non-identifier tokens)
/// and statement keywords (`include`, `as`, `private`, `profile`
/// steer parsing) would make the edited document invalid or change
/// how it parses. Contextual names that remain plain identifiers
/// (`prompt`, `policy`, `template`) stay allowed.
fn is_valid_ident(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "let", "env", "secret", "true", "false", "null", "include", "as", "private", "profile",
    ];
    if RESERVED.contains(&name) {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn rename_at(
    doc: &Doc,
    lookup: &impl Fn(&str) -> Option<String>,
    line: u32,
    character: u32,
    new_name: &str,
) -> Result<WorkspaceEdit, String> {
    if !is_valid_ident(new_name) {
        return Err(format!("`{new_name}` is not a valid identifier"));
    }
    let Some((_, target)) = symbol_at(doc, line, character) else {
        return Err("no symbol at cursor".to_string());
    };
    let (node, path) = match &target {
        Target::Let { name, node } => (node, name.clone()),
        Target::Key { path, node } => (node, path.clone()),
    };
    // Hard boundary: immutable paths cannot be renamed (consistent
    // with the patch guard, spec/annotations.md §4).
    for anns in doc.module.annotations.values() {
        for ann in anns {
            if !matches!(ann.kind, AnnotationKind::AiImmutable(true)) {
                continue;
            }
            let overlaps = ann.path == path
                || ann.path.starts_with(&format!("{path}."))
                || path.starts_with(&format!("{}.", ann.path));
            if overlaps {
                return Err(format!(
                    "cannot rename `{}`: it overlaps ai_immutable `{}`",
                    path, ann.path
                ));
            }
        }
    }
    // Refuse renames onto an existing sibling: bindings collide in
    // the binding map, keys collide in their parent scope's fields
    // (not just the document root). The target itself is excluded by
    // node identity.
    if doc
        .module
        .bindings
        .get(new_name)
        .is_some_and(|taken| taken != node)
    {
        return Err(format!("`{new_name}` already names a let binding"));
    }
    // A `let` lives at document scope: renaming onto a root key would
    // shadow the binding everywhere (table fields win ties per
    // evaluator precedence) and rewrite uses into self-references.
    if let Target::Let { .. } = &target {
        let root_taken = match doc.module.root.and_then(|root| doc.module.get(root)) {
            Some(Node::Table { fields }) => fields.contains_key(new_name),
            _ => false,
        };
        if root_taken {
            return Err(format!("`{new_name}` already names a root key"));
        }
    }
    if let Target::Key { path, .. } = &target {
        let parent_path = path.rsplit_once('.').map(|(parent, _)| parent);
        let sibling_taken = match parent_path {
            None => match doc.module.root.and_then(|root| doc.module.get(root)) {
                Some(Node::Table { fields }) => {
                    fields.get(new_name).is_some_and(|sibling| sibling != node)
                }
                _ => false,
            },
            Some(parent) => match doc.module.resolve_value_path(parent) {
                Some(table) => match doc.module.get(table) {
                    Some(Node::Table { fields }) => {
                        fields.get(new_name).is_some_and(|sibling| sibling != node)
                    }
                    _ => false,
                },
                // Unresolvable parent (stale tree): fall back to
                // refusing only exact root collisions.
                None => false,
            },
        };
        if sibling_taken {
            return Err(format!("`{new_name}` already names a sibling key"));
        }
    }
    let mut refs = reference_ranges(doc, lookup, *node);
    // Include the declaration itself.
    if let Some(def) = definition_at(doc, lookup, line, character)
        .into_iter()
        .next()
    {
        refs.push((def.uri, def.range));
    }
    refs.sort_by(|a, b| {
        a.0.as_str()
            .cmp(b.0.as_str())
            .then(a.1.start.line.cmp(&b.1.start.line))
            .then(a.1.start.character.cmp(&b.1.start.character))
    });
    refs.dedup();
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for (uri, range) in refs {
        changes.entry(uri).or_default().push(TextEdit {
            range,
            new_text: new_name.to_string(),
        });
    }
    Ok(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
}

// ------------------------------------------------------------------
// Semantic tokens (C-043, spec/lsp.md §6).
// ------------------------------------------------------------------

/// Fixed legend: indices are contractual.
pub fn token_legend() -> lsp_types::SemanticTokensLegend {
    use lsp_types::SemanticTokenType as T;
    lsp_types::SemanticTokensLegend {
        token_types: vec![
            T::KEYWORD,
            T::STRING,
            T::NUMBER,
            T::COMMENT,
            T::PROPERTY,
            T::VARIABLE,
            T::OPERATOR,
        ],
        token_modifiers: vec![],
    }
}

const TOK_KEYWORD: u32 = 0;
const TOK_STRING: u32 = 1;
const TOK_NUMBER: u32 = 2;
const TOK_COMMENT: u32 = 3;
const TOK_PROPERTY: u32 = 4;
const TOK_VARIABLE: u32 = 5;
const TOK_OPERATOR: u32 = 6;

/// Classify one lexer token, with statement context for names:
/// binding names after `let`, tag names after `@`, table names
/// after `[`, and contextual keywords by text — otherwise keys
/// (followed by `=`, `:`, `{`, `[`) are properties and the rest are
/// variables.
fn classify(
    kind: &solid_core::parser::TokenKind,
    text: &str,
    prev_significant: Option<&solid_core::parser::TokenKind>,
    next_significant: Option<&solid_core::parser::TokenKind>,
) -> Option<u32> {
    use solid_core::parser::TokenKind as K;
    match kind {
        K::Let | K::Env | K::Secret | K::True | K::False | K::Null => Some(TOK_KEYWORD),
        K::String(_) | K::SingleString | K::MlString(_) | K::MlSingleString => Some(TOK_STRING),
        K::Integer | K::Float | K::Unit(_) => Some(TOK_NUMBER),
        K::Ident => {
            if text == "prompt" || text == "policy" || text == "template" {
                return Some(TOK_KEYWORD);
            }
            if matches!(text, "include" | "as" | "private" | "profile") {
                return Some(TOK_KEYWORD);
            }
            match prev_significant {
                // `let limit`: a definition, not a property.
                Some(K::Let) => Some(TOK_VARIABLE),
                // `[server]`: the table's name.
                Some(K::LBracket) => Some(TOK_PROPERTY),
                _ => match next_significant {
                    // `key =`, `key:`, `key {`, `key [` → property.
                    Some(K::Assign) | Some(K::Colon) | Some(K::LBrace) | Some(K::LBracket) => {
                        Some(TOK_PROPERTY)
                    }
                    _ => Some(TOK_VARIABLE),
                },
            }
        }
        K::At => Some(TOK_OPERATOR),
        K::Assign
        | K::Dot
        | K::Comma
        | K::QuestionColon
        | K::Plus
        | K::Minus
        | K::Star
        | K::Slash
        | K::EqEq
        | K::NotEq
        | K::Lt
        | K::LtEq
        | K::Gt
        | K::GtEq
        | K::AndAnd
        | K::OrOr
        | K::Bang
        | K::Colon
        | K::LBracket
        | K::RBracket
        | K::LBrace
        | K::RBrace
        | K::LParen
        | K::RParen => Some(TOK_OPERATOR),
        // Layout and terminators carry no semantic token.
        K::Eof | K::Newline | K::Indent | K::Dedent => None,
    }
}

/// Absolute (line, start, length, type) tokens for a document,
/// comments included. Multi-line tokens split per line; start and
/// length are UTF-16 code units per LSP.
pub fn semantic_tokens_absolute(text: &str) -> Vec<(u32, u32, u32, u32)> {
    let mut lexer = solid_core::parser::Lexer::new(text);
    let lexed = lexer.tokenize();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<(u32, u32, u32, u32)> = Vec::new();
    // Core/lexer columns are 1-based character columns; LSP semantic
    // tokens use 0-based UTF-16. Convert per line segment.
    let mut push_span = |line_1: u32, col_1: u32, end_line_1: u32, end_col_1: u32, typ: u32| {
        // Split multi-line spans per line.
        let mut line = line_1;
        let mut col = col_1;
        while line < end_line_1 {
            let line_text = lines.get(line as usize - 1).copied().unwrap_or("");
            let char_count = line_text.chars().count();
            let start_char = (col.saturating_sub(1) as usize).min(char_count);
            let start_u16 = char_to_utf16(line_text, start_char);
            let end_u16 = utf16_len(line_text);
            if end_u16 > start_u16 {
                out.push((line - 1, start_u16, end_u16 - start_u16, typ));
            }
            line += 1;
            col = 1;
        }
        let line_text = lines.get(line as usize - 1).copied().unwrap_or("");
        let char_count = line_text.chars().count();
        let start_char = (col.saturating_sub(1) as usize).min(char_count);
        let end_char = (end_col_1.saturating_sub(1) as usize).min(char_count);
        if end_char > start_char {
            let start_u16 = char_to_utf16(line_text, start_char);
            let end_u16 = char_to_utf16(line_text, end_char);
            out.push((line - 1, start_u16, end_u16 - start_u16, typ));
        }
    };
    // Skip layout tokens when peeking for key-vs-variable: one
    // filtered pass keeps the lookahead linear.
    let significant: Vec<&solid_core::parser::Token> = lexed
        .tokens
        .iter()
        .filter(|tok| {
            use solid_core::parser::TokenKind as K;
            !matches!(tok.kind, K::Newline | K::Indent | K::Dedent | K::Eof)
        })
        .collect();
    for (i, tok) in significant.iter().enumerate() {
        let prev_significant = if i > 0 {
            Some(&significant[i - 1].kind)
        } else {
            None
        };
        let next_significant = significant.get(i + 1).map(|later| &later.kind);
        if let Some(typ) = classify(&tok.kind, &tok.text, prev_significant, next_significant) {
            push_span(tok.line, tok.col, tok.end_line, tok.end_col, typ);
        }
    }
    // Comments ride the separate trivia stream: same encoding.
    for (_comment, span) in &lexed.comments {
        push_span(
            span.line,
            span.column,
            span.end_line,
            span.end_column,
            TOK_COMMENT,
        );
    }
    // Already line-ordered from the lexer; comments append in order
    // too (single pass). Sort defensively for the delta encoder.
    out.sort();
    out
}

/// Delta-encode absolute tokens per LSP (`deltaLine`, `deltaStart`,
/// `length`, `type`, `modifiers`).
pub fn encode_semantic_tokens(absolute: &[(u32, u32, u32, u32)]) -> Vec<lsp_types::SemanticToken> {
    let mut data = Vec::with_capacity(absolute.len());
    let (mut prev_line, mut prev_start) = (0u32, 0u32);
    for (line, start, length, typ) in absolute {
        data.push(lsp_types::SemanticToken {
            delta_line: line - prev_line,
            delta_start: if *line == prev_line {
                start - prev_start
            } else {
                *start
            },
            length: *length,
            token_type: *typ,
            token_modifiers_bitset: 0,
        });
        prev_line = *line;
        prev_start = *start;
    }
    data
}

// ------------------------------------------------------------------
// Completions, diagnostics, inlay hints (C-044, spec/lsp.md §4).
// ------------------------------------------------------------------

/// Identifier prefix immediately before a 0-based cursor: ASCII word
/// characters on the cursor's line. Empty when the cursor sits past
/// the line end or directly after a non-identifier character.
fn completion_prefix(text: &str, line: u32, character: u32) -> String {
    let Some(line_text) = text.split('\n').nth(line as usize) else {
        return String::new();
    };
    let chars: Vec<char> = line_text.chars().collect();
    let mut end = utf16_to_char(line_text, character).min(chars.len());
    end = end.min(chars.len());
    let mut start = end;
    while start > 0 && is_completion_char(chars[start - 1]) {
        start -= 1;
    }
    chars[start..end].iter().collect()
}

/// Completion identifier characters: word characters plus `.` so
/// dotted paths keep their qualifier (only the leaf filters).
fn is_completion_char(c: char) -> bool {
    is_word_char(c) || c == '.'
}

/// Schema field names valid at a dotted scope path: walk the active
/// schema tree by segment; a `SchemaTable` yields its field names
/// with type details, anything else yields nothing.
fn schema_names_at(module: &Module, scope_path: &str) -> Vec<(String, String)> {
    let Some(mut id) = module.schema else {
        return Vec::new();
    };
    if !scope_path.is_empty() {
        for segment in scope_path.split('.') {
            match module.get(id) {
                Some(Node::SchemaTable { fields, .. }) => {
                    id = match fields.get(segment) {
                        Some(next) => *next,
                        None => return Vec::new(),
                    };
                }
                _ => return Vec::new(),
            }
        }
    }
    match module.get(id) {
        Some(Node::SchemaTable { fields, .. }) => fields
            .iter()
            .map(|(name, field)| {
                let detail = match module.get(*field) {
                    Some(Node::SchemaField { field }) => field.typ.name(),
                    Some(Node::SchemaArray { element }) => {
                        format!("Array<{}>", schema_type_name(module, *element))
                    }
                    Some(Node::SchemaTable { .. }) => "Table".to_string(),
                    _ => "Any".to_string(),
                };
                (name.clone(), detail)
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Type name of a schema node for completion detail lines.
fn schema_type_name(module: &Module, id: NodeId) -> String {
    match module.get(id) {
        Some(Node::SchemaField { field }) => field.typ.name(),
        Some(Node::SchemaArray { element }) => {
            format!("Array<{}>", schema_type_name(module, *element))
        }
        Some(Node::SchemaTable { .. }) => "Table".to_string(),
        _ => "Any".to_string(),
    }
}

/// Completion items at a 0-based cursor: schema fields for the
/// innermost containing scope, in-scope `let` bindings, and the
/// keywords `true`/`false`/`null` — prefix-filtered, alphabetical,
/// deterministic (spec/lsp.md §4).
pub fn completion_at(doc: &Doc, line: u32, character: u32) -> Vec<CompletionItem> {
    let raw_prefix = completion_prefix(&doc.text, line, character);
    // Only the leaf filters: `server.ho` completes `host`.
    let prefix = raw_prefix.rsplit('.').next().unwrap_or("").to_string();
    let cursor_line = line + 1;
    let cursor_col = doc
        .text
        .split('\n')
        .nth(line as usize)
        .map(|l| utf16_to_char(l, character) as u32 + 1)
        .unwrap_or(1);
    // Innermost containing scope's dotted path (file-local scopes):
    // same ordering as symbol resolution, first match wins.
    let mut containing: Vec<&Scope> = Vec::new();
    let all = scopes(doc);
    for scope in &all {
        if (scope.start_line, scope.start_col) <= (cursor_line, cursor_col)
            && (cursor_line, cursor_col) <= (scope.end_line, scope.end_col)
        {
            containing.push(scope);
        }
    }
    containing.sort_by(|a, b| b.depth.cmp(&a.depth).then_with(|| a.path.cmp(&b.path)));
    let scope_path = containing
        .first()
        .map(|scope| scope.path.clone())
        .unwrap_or_default();
    let mut items: Vec<CompletionItem> = Vec::new();
    for (name, detail) in schema_names_at(&doc.module, &scope_path) {
        if name.starts_with(&prefix) {
            items.push(CompletionItem {
                label: name,
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(detail),
                ..Default::default()
            });
        }
    }
    let mut lets: Vec<&String> = doc.module.bindings.keys().collect();
    lets.sort();
    for name in lets {
        if name.starts_with(&prefix) {
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                ..Default::default()
            });
        }
    }
    for keyword in ["true", "false", "null"] {
        if keyword.starts_with(&prefix) {
            items.push(CompletionItem {
                label: keyword.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            });
        }
    }
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

/// One core diagnostic as an LSP diagnostic: stable code, severity
/// mapping per spec §4, span ranges, `"solid"` source. Span-less
/// diagnostics publish as zero-length ranges at (0,0) rather than
/// vanishing.
fn to_lsp_diagnostic(text: &str, diag: &Diagnostic) -> lsp_types::Diagnostic {
    let range = diag
        .span
        .as_ref()
        .map(|span| range_of_span(text, span))
        .unwrap_or(Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        });
    let severity = match diag.severity {
        Severity::Error => Some(lsp_types::DiagnosticSeverity::ERROR),
        Severity::Warning => Some(lsp_types::DiagnosticSeverity::WARNING),
        Severity::Info => Some(lsp_types::DiagnosticSeverity::INFORMATION),
        Severity::Hint => Some(lsp_types::DiagnosticSeverity::HINT),
    };
    lsp_types::Diagnostic {
        range,
        severity,
        code: Some(lsp_types::NumberOrString::String(diag.code.clone())),
        source: Some("solid".to_string()),
        message: diag.message.clone(),
        ..Default::default()
    }
}

/// Everything the client should show for the open document: parse
/// errors, evaluation diagnostics (hard failures converted at
/// open), lint findings, and healing notices (already merged into
/// the evaluation records — never appended twice). Sorted by
/// position then code for determinism (spec/lsp.md §4). Included
/// files publish under their own URIs via `diagnostics_by_file`.
pub fn diagnostics_for(doc: &Doc) -> Vec<lsp_types::Diagnostic> {
    diagnostics_by_file(doc, &|_: &str| None)
        .into_iter()
        .find(|(uri, _)| *uri == doc.uri)
        .map(|(_, diags)| diags)
        .unwrap_or_default()
}

/// Whether a diagnostic belongs to the open document: unstamped
/// records do; stamped ones must match the document's own file.
fn diag_in_doc(doc: &Doc, diag: &Diagnostic) -> bool {
    match (&doc.uri.to_file_path().ok(), &diag.file) {
        (_, None) => true,
        (Some(main), Some(stamped)) => stamped == &main.to_string_lossy().into_owned(),
        (None, Some(_)) => false,
    }
}

/// Zero-length range at the document start for unlocatable
/// diagnostics (span-less records, or foreign files whose text
/// cannot be read): published rather than vanished (spec §4).
fn zero_range() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 0,
            character: 0,
        },
    }
}

/// Diagnostics grouped by owning file URI: the open document plus
/// one entry per included file with diagnostics. Included spans
/// resolve against the included file's text (lookup from open
/// buffers, then disk); unreadable files fall back to zero ranges
/// on their own URI — never mis-ranged against the parent
/// (spec/lsp.md §4). Files sort by URI for determinism.
pub fn diagnostics_by_file(
    doc: &Doc,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Vec<(Url, Vec<lsp_types::Diagnostic>)> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, (Url, Vec<lsp_types::Diagnostic>)> = BTreeMap::new();
    // The open document always publishes (possibly empty): clients
    // clear stale markers on every open/change.
    groups.insert(doc.uri.to_string(), (doc.uri.clone(), Vec::new()));
    for diag in doc
        .parse_diags
        .iter()
        .chain(doc.eval_diags.iter())
        .chain(doc.lint_diags.iter())
    {
        if diag_in_doc(doc, diag) {
            groups
                .entry(doc.uri.to_string())
                .or_insert_with(|| (doc.uri.clone(), Vec::new()))
                .1
                .push(to_lsp_diagnostic(&doc.text, diag));
        } else if let Some(stamped) = diag.file.as_deref() {
            let uri = Url::from_file_path(stamped).unwrap_or_else(|_| doc.uri.clone());
            let converted = match lookup(stamped) {
                Some(text) => to_lsp_diagnostic(&text, diag),
                None => {
                    let mut fallback = to_lsp_diagnostic("", diag);
                    fallback.range = zero_range();
                    fallback
                }
            };
            groups
                .entry(uri.to_string())
                .or_insert_with(|| (uri, Vec::new()))
                .1
                .push(converted);
        }
    }
    groups
        .into_values()
        .map(|(uri, mut diags)| {
            diags.sort_by(|a, b| {
                (a.range.start.line, a.range.start.character)
                    .cmp(&(b.range.start.line, b.range.start.character))
                    .then_with(|| code_key(a).cmp(&code_key(b)))
            });
            (uri, diags)
        })
        .collect()
}

/// Sort key for a published diagnostic's stable code.
fn code_key(d: &lsp_types::Diagnostic) -> String {
    match d.code.as_ref() {
        Some(lsp_types::NumberOrString::String(s)) => s.clone(),
        Some(lsp_types::NumberOrString::Number(n)) => n.to_string(),
        None => String::new(),
    }
}

/// Static kind name of a binding expression for `let` inlay hints:
/// literals, arrays, tables, and strings. Anything dynamic (refs,
/// env, interpolation, arithmetic) yields nothing.
fn static_node_kind(module: &Module, id: NodeId) -> Option<&'static str> {
    match module.get(id) {
        Some(Node::Literal(value)) => Some(value_kind(value)),
        Some(Node::Array { .. }) => Some("Array"),
        Some(Node::Table { .. }) => Some("Table"),
        Some(Node::Interpolate { .. }) => Some("String"),
        _ => None,
    }
}

/// Inlay hints for a document: after each data key's value and each
/// `let` binding, its type (`: Int`); durations and quantities
/// append the normalized form (`: Duration = 5000 ms`). Schema
/// types win over value kinds; secrets hint `: Secret` only.
/// Hints outside `range` (when given) are withheld (spec/lsp.md §4).
pub fn inlay_hints(doc: &Doc, range: Option<Range>) -> Vec<InlayHint> {
    let mut out = Vec::new();
    let mut push = |position: Position, label: String| {
        if let Some(filter) = range {
            if position < filter.start || position > filter.end {
                return;
            }
        }
        out.push(InlayHint {
            position,
            label: lsp_types::InlayHintLabel::String(label),
            kind: Some(InlayHintKind::TYPE),
            text_edits: None,
            tooltip: None,
            padding_left: Some(true),
            padding_right: None,
            data: None,
        });
    };
    // Data keys: schema type, else evaluated value kind.
    for (id, path) in doc.module.value_paths() {
        if !node_in_doc(doc, id) {
            continue;
        }
        let Some(span) = doc.module.node_spans.get(&id) else {
            continue;
        };
        let position = range_of_span(&doc.text, span).end;
        let mut label = match schema_type_at(&doc.module, &path) {
            Some(typ) => format!(": {typ}"),
            None => match doc
                .value
                .as_ref()
                .and_then(|root| value_at_path(root, &path))
            {
                Some(value) => format!(": {}", value_kind(value)),
                None => continue,
            },
        };
        // Normalized form for durations and quantities (redacted
        // values never appear: secrets take the kind-only branch).
        // Byte quantities evaluate to plain integers, so the source
        // unit comes from the Quantity node when the value no longer
        // carries it.
        if let Some(root) = doc.value.as_ref() {
            match value_at_path(root, &path) {
                Some(Value::DurationMs(ms)) => label.push_str(&format!(" = {ms} ms")),
                Some(Value::Quantity(n, unit)) => {
                    label.push_str(&format!(" = {n} {}", unit.suffix()));
                }
                Some(Value::Int(n)) => {
                    if let Some(Node::Quantity { unit, .. }) = doc.module.get(id) {
                        label.push_str(&format!(" = {n} {}", unit.suffix()));
                    }
                }
                _ => {}
            }
        }
        push(position, label);
    }
    // `let` bindings: static kinds only.
    let mut bindings: Vec<(&String, &NodeId)> = doc.module.bindings.iter().collect();
    bindings.sort_by(|a, b| a.0.cmp(b.0));
    for (_name, id) in bindings {
        if !node_in_doc(doc, *id) {
            continue;
        }
        let (Some(kind), Some(span)) = (
            static_node_kind(&doc.module, *id),
            doc.module.node_spans.get(id),
        ) else {
            continue;
        };
        push(range_of_span(&doc.text, span).end, format!(": {kind}"));
    }
    out.sort_by(|a, b| {
        (a.position.line, a.position.character).cmp(&(b.position.line, b.position.character))
    });
    out
}

// ------------------------------------------------------------------
// Code actions (C-045, spec/lsp.md §5).
// ------------------------------------------------------------------

/// Auto-apply confidence tier for healing actions (spec/healing.md,
/// C-016): repairs at or above this apply without asking.
const HEAL_APPLY_CONFIDENCE: f64 = 0.95;

/// Whether two LSP ranges touch (point selections inclusive): a
/// cursor parked at a diagnostic's edge counts as "at" it, so
/// touching edges qualify alongside overlaps.
fn ranges_touch(a: Range, b: Range) -> bool {
    let (a_start, a_end) = (
        (a.start.line, a.start.character),
        (a.end.line, a.end.character),
    );
    let (b_start, b_end) = (
        (b.start.line, b.start.character),
        (b.end.line, b.end.character),
    );
    a_start <= b_end && b_start <= a_end
}

/// All publishable diagnostics for action matching: parse, eval
/// (hard failures converted), lint. Healing rides inside the
/// evaluation records (merged once — see `diagnostics_for`).
fn action_diags(doc: &Doc) -> impl Iterator<Item = &Diagnostic> {
    doc.parse_diags
        .iter()
        .chain(doc.eval_diags.iter())
        .chain(doc.lint_diags.iter())
}

/// Quickfix actions at a request range: healing repairs, extract
/// let, style conversions, missing depends_on entries — sorted by
/// title for determinism (spec/lsp.md §5).
pub fn code_actions(doc: &Doc, range: Range) -> Vec<CodeAction> {
    let mut out = Vec::new();
    out.extend(heal_actions(doc, range));
    out.extend(extract_action(doc, range));
    out.extend(convert_actions(doc));
    out.extend(depends_on_actions(doc, range));
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
}

/// One edit to one file, as a workspace edit.
fn single_file_edit(uri: Url, edit: TextEdit) -> WorkspaceEdit {
    let mut changes = HashMap::new();
    changes.insert(uri, vec![edit]);
    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Apply-healing actions for H-diagnostics touching the range whose
/// repair confidence reaches the auto-apply tier. Application honors
/// repair shape: empty `original` inserts at a point, otherwise the
/// span is replaced — except `InsertQuote`, whose diagnostic span
/// points at the string's opening delimiter while the repair belongs
/// at the string token's end (the coordinates the lexer healed
/// from); without that token no action is offered.
fn heal_actions(doc: &Doc, range: Range) -> Vec<CodeAction> {
    let mut out = Vec::new();
    for diag in action_diags(doc) {
        if !diag.code.starts_with('H') || diag.confidence < HEAL_APPLY_CONFIDENCE {
            continue;
        }
        let Some(repair) = diag.repair.as_ref() else {
            continue;
        };
        // Confidence alone is not enough: an explicitly unsafe
        // repair (H005 similarity without a passing safety check)
        // must never present as apply-ready, however it scored.
        if !repair.safe {
            continue;
        }
        let diag_range = diag
            .span
            .as_ref()
            .map(|span| range_of_span(&doc.text, span))
            .unwrap_or(zero_range());
        if !ranges_touch(diag_range, range) {
            continue;
        }
        let edit_range = match repair.kind {
            RepairKind::InsertQuote => {
                let Some(insert_at) = string_token_end(&doc.text, diag.span.as_ref()) else {
                    continue;
                };
                Range {
                    start: insert_at,
                    end: insert_at,
                }
            }
            _ if repair.original.is_empty() => Range {
                start: diag_range.start,
                end: diag_range.start,
            },
            _ => diag_range,
        };
        out.push(CodeAction {
            title: format!("Apply healing ({})", diag.code),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![to_lsp_diagnostic(&doc.text, diag)]),
            edit: Some(single_file_edit(
                doc.uri.clone(),
                TextEdit {
                    range: edit_range,
                    new_text: repair.suggested.clone(),
                },
            )),
            ..Default::default()
        });
    }
    out
}

/// End position of the string token starting at a span (1-based core
/// coordinates): locates the unterminated-string repair's true
/// insertion point. String tokens never span... they can span lines
/// (multi-line); the token end is authoritative either way.
fn string_token_end(text: &str, span: Option<&solid_core::diag::Span>) -> Option<Position> {
    use solid_core::parser::TokenKind as K;
    let span = span?;
    let mut lexer = solid_core::parser::Lexer::new(text);
    for tok in lexer.tokenize().tokens {
        match tok.kind {
            K::String(_) | K::SingleString | K::MlString(_) | K::MlSingleString
                if tok.line == span.line && tok.col == span.column =>
            {
                return Some(core_to_lsp(text, tok.end_line, tok.end_col));
            }
            _ => {}
        }
    }
    None
}

/// Byte range of a core span, for containment checks against
/// selection offsets. Core positions are 1-based character
/// coordinates — never passed as UTF-16 columns (non-BMP characters
/// occupy two UTF-16 units but one character column).
fn span_byte_range(text: &str, span: &solid_core::diag::Span) -> Option<(usize, usize)> {
    let start = core_offset(text, span.line, span.column);
    let end = core_offset(text, span.end_line, span.end_column);
    (start <= end).then_some((start, end))
}

/// Byte offset of a 1-based core (line, character-column) position:
/// the character-aware counterpart to `offset_of` (which takes
/// 0-based UTF-16). Out-of-range positions clamp to the document,
/// like every other mapping in this backend.
fn core_offset(text: &str, line_1: u32, col_1: u32) -> usize {
    let mut offset = 0usize;
    for (i, l) in text.split('\n').enumerate() {
        if i as u32 == line_1.saturating_sub(1) {
            let char_idx = (col_1.saturating_sub(1) as usize).min(l.chars().count());
            let mut byte = 0usize;
            for (j, c) in l.chars().enumerate() {
                if j >= char_idx {
                    break;
                }
                byte += c.len_utf8();
            }
            return offset + byte;
        }
        offset += l.len() + 1;
    }
    text.len()
}

/// Whether a byte range is extractable as a `let` value: some single
/// non-table value node's span must match it EXACTLY (partial-token
/// selections would emit invalid lets or corrupt the original), and
/// every reference inside must resolve to a `let` binding or a root
/// key — moving a scope-relative reference to document scope would
/// rebind it. Tables (inline or block) are excluded, as are schema
/// and patch nodes. Anything else is withheld rather than emitted
/// as an invalid let.
fn is_value_expression(doc: &Doc, start: usize, end: usize) -> bool {
    let mut matched = false;
    for (id, node) in doc.module.arena.iter().enumerate() {
        match node {
            Node::Table { .. }
            | Node::SchemaTable { .. }
            | Node::SchemaField { .. }
            | Node::SchemaArray { .. }
            | Node::PatchDirective { .. } => continue,
            _ => {}
        }
        let id = NodeId(id as u32);
        if !node_in_doc(doc, id) {
            continue;
        }
        if let Some(span) = doc.module.node_spans.get(&id) {
            if span_byte_range(&doc.text, span) == Some((start, end)) {
                matched = true;
                break;
            }
        }
    }
    if !matched {
        return false;
    }
    // Scope safety for references inside the selection.
    let mut paths: HashMap<NodeId, String> = HashMap::new();
    for (id, path) in doc.module.value_paths() {
        paths.insert(id, path);
    }
    for (i, node) in doc.module.arena.iter().enumerate() {
        let Node::Ref(target) = node else {
            continue;
        };
        let Some(span) = doc.module.node_spans.get(&NodeId(i as u32)) else {
            continue;
        };
        let Some((ref_start, ref_end)) = span_byte_range(&doc.text, span) else {
            continue;
        };
        if ref_start < start || ref_end > end {
            continue;
        }
        if doc.module.bindings.values().any(|id| id == target) {
            continue;
        }
        match paths.get(target) {
            // Root keys resolve at document scope like bindings.
            Some(path) if !path.contains('.') => {}
            _ => return false,
        }
    }
    true
}

/// Extract-let action for a non-empty selection: insert the binding
/// after the last top-level `let` (or at the top of file) and
/// replace the selection with the fresh name.
fn extract_action(doc: &Doc, range: Range) -> Vec<CodeAction> {
    if range.start == range.end {
        return Vec::new();
    }
    let start = offset_of(&doc.text, range.start.line, range.start.character);
    let end = offset_of(&doc.text, range.end.line, range.end.character);
    if start >= end || !is_value_expression(doc, start, end) {
        return Vec::new();
    }
    let Some(selected) = doc.text.get(start..end).map(str::to_string) else {
        return Vec::new();
    };
    if selected.trim().is_empty() {
        return Vec::new();
    }
    // Fresh name past existing bindings and root keys.
    let mut name = "extracted".to_string();
    let mut counter = 2u32;
    let root_taken = |candidate: &str| {
        doc.module.bindings.contains_key(candidate)
            || doc
                .module
                .root
                .and_then(|root| doc.module.get(root))
                .and_then(|node| match node {
                    Node::Table { fields } => Some(fields.contains_key(candidate)),
                    _ => None,
                })
                .unwrap_or(false)
    };
    while root_taken(&name) {
        name = format!("extracted{counter}");
        counter += 1;
    }
    // Insertion after the last top-level `let` (or at the top of
    // file). Top-level means column 1: nested lets inside brace
    // blocks are indented by the canonical layout (a column-1 let
    // inside braces is pathological formatting the formatter would
    // fix — and inserting beside it stays valid regardless).
    let mut lexer = solid_core::parser::Lexer::new(&doc.text);
    let last_let = lexer
        .tokenize()
        .tokens
        .iter()
        .filter(|tok| matches!(tok.kind, solid_core::parser::TokenKind::Let) && tok.col == 1)
        .map(|tok| tok.line)
        .max();
    let (insert_at, insert_text) = match last_let {
        Some(line_1) => {
            let line_0 = line_1.saturating_sub(1);
            let width = doc
                .text
                .split('\n')
                .nth(line_0 as usize)
                .map(line_content_width)
                .unwrap_or(0);
            (
                Position {
                    line: line_0,
                    character: width,
                },
                format!("\nlet {name} = {selected}"),
            )
        }
        None => (
            Position {
                line: 0,
                character: 0,
            },
            format!("let {name} = {selected}\n"),
        ),
    };
    let mut changes = HashMap::new();
    changes.insert(
        doc.uri.clone(),
        vec![
            TextEdit {
                range: Range {
                    start: insert_at,
                    end: insert_at,
                },
                new_text: insert_text,
            },
            TextEdit {
                range,
                new_text: name.clone(),
            },
        ],
    );
    vec![CodeAction {
        title: format!("Extract to let `{name}`"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        ..Default::default()
    }]
}

/// Style-conversion actions: each direction offered only when it
/// would change the text, through the exact formatter.
fn convert_actions(doc: &Doc) -> Vec<CodeAction> {
    let mut out = Vec::new();
    let lines: Vec<&str> = doc.text.split('\n').collect();
    let full = Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: lines.len().saturating_sub(1) as u32,
            character: lines.last().map(|l| line_content_width(l)).unwrap_or(0),
        },
    };
    for (label, style) in [
        ("brace", solid_core::Style::Brace),
        ("indent", solid_core::Style::Indent),
    ] {
        let converted = solid_core::fmt::format(&doc.module, style);
        if converted == doc.text {
            continue;
        }
        out.push(CodeAction {
            title: format!("Convert to {label} style"),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(single_file_edit(
                doc.uri.clone(),
                TextEdit {
                    range: full,
                    new_text: converted,
                },
            )),
            ..Default::default()
        });
    }
    out
}

/// Missing-depends_on actions for L002 diagnostics touching the
/// range: append into the existing array, or create the line at the
/// scope's depth. The missing name parses from the diagnostic's
/// quoted message; the scope resolves from its path field.
fn depends_on_actions(doc: &Doc, range: Range) -> Vec<CodeAction> {
    let mut out = Vec::new();
    for diag in doc.lint_diags.iter() {
        if diag.code != "L002" {
            continue;
        }
        // Machine repair metadata identifies the diagnostic even if
        // the prose ever changes; the message prefix is the fallback
        // for records predating the metadata.
        let has_repair = matches!(&diag.repair, Some(repair)
            if matches!(&repair.kind, RepairKind::Other(kind) if kind == "add_depends_on"));
        if !(has_repair || diag.message.starts_with("missing depends_on entry for '")) {
            continue;
        }
        let diag_range = diag
            .span
            .as_ref()
            .map(|span| range_of_span(&doc.text, span))
            .unwrap_or(zero_range());
        if !ranges_touch(diag_range, range) {
            continue;
        }
        // The missing path rides the repair record when present
        // (machine-readable); prose parsing is the fallback for
        // diagnostics predating the repair metadata.
        let name = match &diag.repair {
            Some(repair) if repair.original.is_empty() && !repair.suggested.is_empty() => {
                repair.suggested.clone()
            }
            _ => {
                let Some(parsed) = diag
                    .message
                    .strip_prefix("missing depends_on entry for '")
                    .and_then(|rest| rest.strip_suffix('\''))
                else {
                    continue;
                };
                parsed.to_string()
            }
        };
        let scope_path = diag.path.as_deref().unwrap_or("");
        let table = if scope_path.is_empty() {
            doc.module.root
        } else {
            doc.module.resolve_value_path(scope_path)
        };
        let Some(table) = table else {
            continue;
        };
        let edit = match doc.module.depends_on.get(&table) {
            // Append after the `[`: `"name", ` (or `"name"` into an
            // empty array, avoiding a trailing comma).
            Some(dep) => {
                let at = offset_of(
                    &doc.text,
                    dep.span.line.saturating_sub(1),
                    dep.span.column.saturating_sub(1),
                );
                let insert_text = if dep.paths.is_empty() {
                    format!("\"{name}\"")
                } else {
                    format!("\"{name}\", ")
                };
                // The span starts at the array's `[` by construction
                // (spec/linter.md §2); anything else is unsafe.
                if doc.text.get(at..at + 1) != Some("[") {
                    continue;
                }
                let position = position_of(&doc.text, at + 1);
                TextEdit {
                    range: Range {
                        start: position,
                        end: position,
                    },
                    new_text: insert_text,
                }
            }
            // Create the line at the scope's depth (formatter
            // canonicalizes placement on the next run).
            None => {
                let depth = scopes(doc)
                    .iter()
                    .find(|scope| scope.path == scope_path)
                    .map(|scope| scope.depth)
                    .unwrap_or(0);
                let (line_1, own_span) = match doc.module.node_spans.get(&table) {
                    Some(span) => (span.line, true),
                    None => (1, false),
                };
                let at = if scope_path.is_empty() || !own_span {
                    Position {
                        line: 0,
                        character: 0,
                    }
                } else {
                    Position {
                        line: line_1,
                        character: 0,
                    }
                };
                TextEdit {
                    range: Range { start: at, end: at },
                    new_text: format!("{}depends_on = [\"{name}\"]\n", "  ".repeat(depth)),
                }
            }
        };
        out.push(CodeAction {
            title: format!("Add depends_on entry for '{name}'"),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![to_lsp_diagnostic(&doc.text, diag)]),
            edit: Some(single_file_edit(doc.uri.clone(), edit)),
            ..Default::default()
        });
    }
    out
}
