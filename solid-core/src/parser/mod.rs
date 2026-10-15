//! Full recursive-descent parser for the SOLID alpha.
//!
//! Supported surface language:
//! - `let name = expr`
//! - `[table]` blocks with `key = expr`
//! - top-level and nested `key = expr`
//! - `env.NAME ?: default`
//! - `secret("…")` / `secret(env.NAME)`
//! - integers, floats, bools, null, strings (single & double quoted)
//! - unit suffixes on numbers (`5s`, `512MiB`, `100/req/s` …) → Quantity nodes
//! - full string interpolation `"…${ident}…"` (and nested simple expressions)
//! - arrays `[ … ]`
//! - inline tables `{ k = v, … }`
//! - binary operators with correct precedence:
//!   `||` `&&` `==` `!=` `<` `<=` `>` `>=` `+` `-` `*` `/`
//! - unary `-` and `!`
//! - parentheses for grouping
//!
//! After parsing, a resolution pass turns Ident nodes that match `let`
//! bindings into Ref nodes and builds the dependency graph.

mod lexer;

pub use lexer::{Lexer, Token, TokenKind};

/// Extract the span of a token.
fn span_of(tok: &Token) -> Span {
    Span {
        line: tok.line,
        column: tok.col,
        end_line: tok.end_line,
        end_column: tok.end_col,
    }
}

/// Maximum expression nesting depth (parenthesized expressions, unary
/// operator chains, nested arrays, nested inline tables). A hard limit
/// that prevents stack exhaustion from pathological input — the
/// parser-side half of the execution budget (see C-037).
const MAX_EXPR_DEPTH: usize = 64;

/// Maximum structural (table/block) nesting depth. Bounds parser recursion
/// on pathological input; exceeding it is E011 (see C-037).
const MAX_BLOCK_DEPTH: usize = 64;

use crate::diag::{thresholds, Diagnostic, Repair, RepairKind, Severity, Span};
use crate::ir::*;
use crate::types::Unit;

/// Options controlling parsing and healing behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct ParseOptions {
    /// When true, medium-confidence repairs (0.70–0.94) are applied and
    /// reported as warnings. When false (strict, the default), they are
    /// declined and the underlying syntax diagnostic stands.
    pub resilient: bool,
}

pub fn parse(source: &str, name: &str) -> Result<Module, Vec<Diagnostic>> {
    parse_with(source, name, ParseOptions::default())
}

pub fn parse_with(
    source: &str,
    name: &str,
    options: ParseOptions,
) -> Result<Module, Vec<Diagnostic>> {
    let (module, errors) = parse_recovering(source, name, options);
    if errors.is_empty() {
        Ok(module)
    } else {
        Err(errors)
    }
}

/// Parse and return the module even when diagnostics were recorded.
///
/// Tooling (editor diagnostics on broken files) and recovery tests need the
/// partial tree; `parse` stays the strict entry point that fails on any
/// error. Trivia and spans remain accurate for the parts that parsed.
pub fn parse_recovering(
    source: &str,
    name: &str,
    options: ParseOptions,
) -> (Module, Vec<Diagnostic>) {
    let mut lexer = Lexer::new(source);
    let lexed = lexer.tokenize();
    let mut parser = Parser {
        tokens: lexed.tokens,
        pos: 0,
        module: Module::new(name),
        errors: Vec::new(),
        ident_scopes: Vec::new(),
        depth: 0,
        options,
        block_depth: 0,
        comments: lexed.comments,
        statements: Vec::new(),
        blocks: Vec::new(),
        current_path: Vec::new(),
        brace_tables: 0,
        indent_tables: 0,
    };
    parser.module.healing.extend(lexed.repairs);
    parser.errors.extend(lexed.errors);
    parser.parse_document();
    parser.attach_trivia();
    parser.module.source_style = if parser.indent_tables > parser.brace_tables {
        Style::Indent
    } else {
        Style::Brace
    };
    parser.build_graph();
    (parser.module, parser.errors)
}

/// Compute each comment's normative attachment (spec/dual-syntax.md §7.2)
/// and store it in the module's trivia list.
impl Parser {
    fn attach_trivia(&mut self) {
        let comments = std::mem::take(&mut self.comments);
        let statements = std::mem::take(&mut self.statements);
        let blocks = std::mem::take(&mut self.blocks);

        let mut trivia = Vec::with_capacity(comments.len());
        for (text, span) in comments {
            let attachment = self.attach_one(&span, &statements, &blocks);
            trivia.push(TriviaComment {
                text,
                span,
                attachment,
            });
        }
        self.module.trivia = trivia;
    }

    fn attach_one(&self, span: &Span, stmts: &[StmtInfo], blocks: &[BlockInfo]) -> Attachment {
        let line = span.line;
        let level = span.column.saturating_sub(1) as usize;

        // Rule 1: after code on the same line as any statement — either the
        // line the statement begins on (a table header, `[app] # note`) or
        // the line it ends on (a multi-line value).
        if let Some(stmt) = stmts
            .iter()
            .filter(|s| s.start_line == line || s.end_line == line)
            .max_by_key(|s| s.start_line)
        {
            return Attachment::Trailing(qualified(&stmt.path, &stmt.key));
        }

        // Determine the comment's block (spec §7.1): the innermost block
        // that still owns the comment's line, preferring one whose body is
        // at exactly the comment's own indentation level.
        let owner = self.owning_block(line, level, stmts, blocks);
        if let Some(b) = &owner {
            // Rule 2: any statement (including a nested table header)
            // follows within the same block.
            if let Some(stmt) = stmts
                .iter()
                .filter(|s| s.start_line > line && s.path == b.path)
                .min_by_key(|s| s.start_line)
            {
                return Attachment::Leading(qualified(&stmt.path, &stmt.key));
            }
            // Rule 3: trailing comment of that block.
            return Attachment::TrailingBlock(b.path.clone());
        }

        // Rule 2 at document level, then rule 4.
        if let Some(stmt) = stmts
            .iter()
            .filter(|s| s.start_line > line && s.path.is_empty())
            .min_by_key(|s| s.start_line)
        {
            return Attachment::Leading(qualified(&stmt.path, &stmt.key));
        }
        Attachment::TrailingDocument
    }

    /// The innermost block owning `line` at indentation `level`, per §7.1.
    fn owning_block(
        &self,
        line: u32,
        level: usize,
        stmts: &[StmtInfo],
        blocks: &[BlockInfo],
    ) -> Option<BlockInfo> {
        // Score prefers an exact indentation-level match, then a deeper
        // block, then a later start.
        fn score(b: &BlockInfo, level: usize) -> (u8, usize, u32) {
            (if b.level == level { 1 } else { 0 }, b.level, b.start_line)
        }
        let mut best: Option<&BlockInfo> = None;
        for b in blocks {
            if b.start_line > line {
                continue;
            }
            // The block owns lines up to the next statement at or above its
            // own level *after this comment*.
            let next_sibling = stmts
                .iter()
                .filter(|s| s.start_line > line && s.level <= b.level)
                .min_by_key(|s| s.start_line)
                .map(|s| s.start_line)
                .unwrap_or(u32::MAX);
            if line >= next_sibling {
                continue;
            }
            let better = match best {
                None => true,
                Some(prev) => score(b, level) >= score(prev, level),
            };
            if better {
                best = Some(b);
            }
        }
        best.cloned()
    }
}

/// The fully qualified key of a statement (`table.key`, or just `key` at
/// document level) — the identity comments attach to.
fn qualified(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    module: Module,
    errors: Vec<Diagnostic>,
    /// Stack of per-scope (owning key, Ident node id) pairs, used to resolve
    /// sibling-key references (e.g. `can_scale = !is_prod` inside the same
    /// table). The owning key prevents a key's own value from resolving to
    /// itself.
    ident_scopes: Vec<Vec<(String, NodeId)>>,
    /// Current expression nesting depth, guarded by MAX_EXPR_DEPTH.
    depth: usize,
    /// Healing mode for this parse.
    options: ParseOptions,
    /// Current block nesting depth, guarded by MAX_BLOCK_DEPTH.
    block_depth: usize,
    /// Comments from the lexer, awaiting attachment computation.
    comments: Vec<(String, Span)>,
    /// Statement records used for comment attachment (spec §7).
    statements: Vec<StmtInfo>,
    /// Block (table) records used for comment attachment (spec §7).
    blocks: Vec<BlockInfo>,
    /// Enclosing table path while parsing.
    current_path: Vec<String>,
    /// Counts of brace-style and indentation-style table declarations.
    brace_tables: usize,
    indent_tables: usize,
}

impl Parser {
    /// The dotted path of the enclosing table (empty at document level).
    fn path_str(&self) -> String {
        self.current_path.join(".")
    }

    /// End line of the most recently consumed token.
    fn last_end_line(&self) -> u32 {
        self.tokens
            .get(self.pos.saturating_sub(1))
            .map(|t| t.end_line)
            .unwrap_or(1)
    }

    /// Record a statement for comment attachment.
    fn record_stmt(&mut self, key: &str, start_line: u32, end_line: u32, level: usize) {
        self.statements.push(StmtInfo {
            key: key.to_string(),
            path: self.path_str(),
            start_line,
            end_line,
            level,
        });
    }
}

/// A statement's source extent, for comment attachment.
#[derive(Debug, Clone)]
struct StmtInfo {
    key: String,
    path: String,
    start_line: u32,
    end_line: u32,
    level: usize,
}

/// A table/block's source extent, for comment attachment.
#[derive(Debug, Clone)]
struct BlockInfo {
    path: String,
    level: usize,
    start_line: u32,
}

/// What terminates a statement body (spec/dual-syntax.md §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyEnd {
    /// Document root: Dedents are the EOF flush and carry no block meaning.
    Document,
    /// An indented block at indentation level `n`: ends at a Dedent whose
    /// target level is below `n`, which it consumes.
    Block(usize),
    /// A brace block (`key { … }`): ends at and consumes its `}`.
    Brace,
    /// The body of a `[header]` written at column `n`: ends (without
    /// consuming) when the next statement belongs to the enclosing scope.
    HeaderBody(usize),
}

/// The indentation level an Indent/Dedent token refers to (its text).
fn indent_level(tok: &Token) -> usize {
    tok.text.parse().unwrap_or(0)
}

impl Parser {
    // ------------------------------------------------------------------
    // Low-level token helpers
    // ------------------------------------------------------------------

    fn peek(&self) -> &Token {
        // The lexer always appends a trailing Eof token and `bump` never
        // advances past it, so the fallback borrows from the token stream
        // itself instead of a function-local temporary (fixes E0515).
        self.tokens.get(self.pos).unwrap_or_else(|| {
            self.tokens
                .last()
                .expect("token stream is never empty; the lexer appends Eof")
        })
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn bump(&mut self) -> Token {
        let t = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, kind: TokenKind) -> Option<Token> {
        // Compare by discriminant so Unit(String) still matches Unit(_)
        if std::mem::discriminant(self.peek_kind()) == std::mem::discriminant(&kind) {
            Some(self.bump())
        } else {
            self.error(
                "E001",
                format!("expected {:?}, found {:?}", kind, self.peek_kind()),
            );
            None
        }
    }

    fn error(&mut self, code: &str, message: impl Into<String>) {
        let t = self.peek();
        self.errors.push(Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            path: None,
            span: Some(Span {
                line: t.line,
                column: t.col,
                end_line: t.end_line,
                end_column: t.end_col,
            }),
            repair: None,
            confidence: 1.0,
        });
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.bump();
        }
    }

    /// Token kind `n` positions ahead (0 = current). Used for the two-token
    /// lookahead that distinguishes `key =`, `key:`, and `key {`.
    fn peek_kind_at(&self, n: usize) -> TokenKind {
        self.tokens
            .get(self.pos + n)
            .map(|t| t.kind.clone())
            .unwrap_or(TokenKind::Eof)
    }

    /// Parse statements into `fields` until the body terminator
    /// (spec/dual-syntax.md §2, §5). Structural nesting is bounded by
    /// MAX_BLOCK_DEPTH so pathological input cannot exhaust the stack.
    fn parse_body(&mut self, fields: &mut indexmap::IndexMap<String, NodeId>, end: BodyEnd) {
        self.block_depth += 1;
        if self.block_depth > MAX_BLOCK_DEPTH {
            self.error(
                "E011",
                format!("maximum block nesting depth ({MAX_BLOCK_DEPTH}) exceeded"),
            );
            self.block_depth -= 1;
            self.recover_block(end);
            return;
        }
        self.parse_body_inner(fields, end);
        self.block_depth -= 1;
    }

    /// Skip tokens until the terminator of an over-deep body, so recovery
    /// makes progress without recursing further. Terminator- and
    /// level-aware: a Dedent that closes a deeper child must not stop the
    /// skip early, or later statements would leak into an outer scope.
    fn recover_block(&mut self, end: BodyEnd) {
        let mut open_braces = 0usize;
        loop {
            match self.peek_kind() {
                TokenKind::Eof => return,
                TokenKind::LBrace => {
                    open_braces += 1;
                    self.bump();
                }
                TokenKind::RBrace => {
                    if open_braces > 0 {
                        open_braces -= 1;
                        self.bump();
                    } else if end == BodyEnd::Brace {
                        self.bump();
                        return;
                    } else {
                        self.bump();
                    }
                }
                TokenKind::Dedent => {
                    let tok = self.peek().clone();
                    let target = indent_level(&tok);
                    match end {
                        BodyEnd::Block(level) | BodyEnd::HeaderBody(level) => {
                            self.bump();
                            if target < level {
                                return;
                            }
                        }
                        _ => {
                            self.bump();
                        }
                    }
                }
                TokenKind::LBracket | TokenKind::Let if matches!(end, BodyEnd::HeaderBody(_)) => {
                    return
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn parse_body_inner(&mut self, fields: &mut indexmap::IndexMap<String, NodeId>, end: BodyEnd) {
        loop {
            self.skip_newlines();
            match self.peek_kind() {
                TokenKind::Eof => {
                    if end == BodyEnd::Brace {
                        self.error("E001", "expected `}`, found end of input");
                    }
                    return;
                }
                TokenKind::Dedent => {
                    let tok = self.peek().clone();
                    let target = indent_level(&tok);
                    match end {
                        // The EOF flush emits Dedent tokens; they carry no
                        // block meaning at document level.
                        BodyEnd::Document | BodyEnd::Brace => {
                            self.bump();
                        }
                        BodyEnd::Block(level) => {
                            self.bump();
                            if target >= level {
                                // Closes a nested block that was recovered
                                // under E010 (§6), not this block.
                                continue;
                            }
                            return;
                        }
                        BodyEnd::HeaderBody(level) => {
                            if target >= level {
                                self.bump();
                                continue;
                            }
                            return;
                        }
                    }
                }
                TokenKind::RBrace => match end {
                    BodyEnd::Brace => {
                        self.bump();
                        return;
                    }
                    // §5.3: a brace cannot close an indentation block.
                    BodyEnd::Block(_) => {
                        self.error("E001", "expected Dedent to close this block, found `}`");
                        self.bump();
                        return;
                    }
                    BodyEnd::HeaderBody(_) => return,
                    BodyEnd::Document => {
                        self.error("E002", "unexpected `}`");
                        self.bump();
                    }
                },
                // A header body ends (without consuming) when the next
                // statement belongs to the enclosing scope.
                TokenKind::LBracket | TokenKind::Let if matches!(end, BodyEnd::HeaderBody(_)) => {
                    return
                }
                TokenKind::Indent => {
                    // E010 unexpected indentation (§6): consume the Indent,
                    // parse the line in the enclosing block, and let the
                    // level-aware Dedent rule above discard its matching
                    // Dedent (the enclosing block's level comparison).
                    self.error("E010", "unexpected indentation");
                    self.bump();
                }
                TokenKind::Let => self.parse_let(),
                TokenKind::LBracket => self.parse_table_header(fields),
                TokenKind::Ident => self.parse_ident_statement(fields),
                _ => {
                    // Alpha resilience: skip an unexpected token.
                    self.bump();
                }
            }
        }
    }

    /// Dispatch a statement that begins with an identifier: `key = value`,
    /// `key: value`, `key:` block header, or `key { … }` table block.
    fn parse_ident_statement(&mut self, fields: &mut indexmap::IndexMap<String, NodeId>) {
        let start = self.peek().clone();
        let level = start.col.saturating_sub(1) as usize;
        match self.peek_kind_at(1) {
            TokenKind::Assign => {
                if let Some((k, id)) = self.parse_key_value_sep(TokenKind::Assign) {
                    let end = self.last_end_line();
                    self.record_stmt(&k, start.line, end, level);
                    fields.insert(k, id);
                }
            }
            TokenKind::Colon => {
                if matches!(self.peek_kind_at(2), TokenKind::Newline | TokenKind::Eof) {
                    self.parse_block_header(fields);
                } else if let Some((k, id)) = self.parse_key_value_sep(TokenKind::Colon) {
                    let end = self.last_end_line();
                    self.record_stmt(&k, start.line, end, level);
                    fields.insert(k, id);
                }
            }
            TokenKind::LBrace => self.parse_table_block(fields),
            _ => {
                // Not a statement form the grammar accepts; keep progress.
                let tok = self.peek().clone();
                self.error_at("E002", "unexpected token after identifier", &tok);
                self.bump();
            }
        }
    }

    /// `key:` followed by an indented block (indentation style, §2 and §5).
    fn parse_block_header(&mut self, parent: &mut indexmap::IndexMap<String, NodeId>) {
        let key_tok = self.peek().clone();
        let key = self
            .expect(TokenKind::Ident)
            .map(|t| t.text)
            .unwrap_or_default();
        let colon = self.expect(TokenKind::Colon);
        self.skip_newlines();
        let level = if matches!(self.peek_kind(), TokenKind::Indent) {
            let indent = self.bump();
            indent_level(&indent)
        } else {
            // E008: the header has no indented body (including at EOF).
            let tok = colon.unwrap_or_else(|| self.peek().clone());
            self.error_at(
                "E008",
                format!("block header `{key}:` has no indented body"),
                &tok,
            );
            return;
        };
        self.indent_tables += 1;
        let stmt_idx = self.statements.len();
        self.statements.push(StmtInfo {
            key: key.clone(),
            path: self.path_str(),
            start_line: key_tok.line,
            end_line: key_tok.line,
            level: key_tok.col.saturating_sub(1) as usize,
        });
        self.current_path.push(key.clone());
        self.blocks.push(BlockInfo {
            path: self.path_str(),
            level,
            start_line: key_tok.line,
        });
        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        self.parse_body(&mut fields, BodyEnd::Block(level));
        let idents = self.ident_scopes.pop().unwrap_or_default();
        self.resolve_sibling_refs(idents, &fields);
        let table_id = self.module.add_node(Node::Table { fields });
        self.statements[stmt_idx].end_line = self.last_end_line();
        self.current_path.pop();
        parent.insert(key, table_id);
    }

    /// `key { … }` brace-style table block (HCL-style, §2).
    fn parse_table_block(&mut self, parent: &mut indexmap::IndexMap<String, NodeId>) {
        let key_tok = self.peek().clone();
        let key = self
            .expect(TokenKind::Ident)
            .map(|t| t.text)
            .unwrap_or_default();
        self.expect(TokenKind::LBrace);
        self.brace_tables += 1;
        let stmt_idx = self.statements.len();
        self.statements.push(StmtInfo {
            key: key.clone(),
            path: self.path_str(),
            start_line: key_tok.line,
            end_line: key_tok.line,
            level: key_tok.col.saturating_sub(1) as usize,
        });
        self.current_path.push(key.clone());
        self.blocks.push(BlockInfo {
            path: self.path_str(),
            level: key_tok.col.saturating_sub(1) as usize,
            start_line: key_tok.line,
        });
        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        self.parse_body(&mut fields, BodyEnd::Brace);
        let idents = self.ident_scopes.pop().unwrap_or_default();
        self.resolve_sibling_refs(idents, &fields);
        let table_id = self.module.add_node(Node::Table { fields });
        self.statements[stmt_idx].end_line = self.last_end_line();
        self.current_path.pop();
        parent.insert(key, table_id);
    }

    /// Diagnostic at an explicit token's span.
    fn error_at(&mut self, code: &str, message: impl Into<String>, tok: &Token) {
        self.errors.push(Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            path: None,
            span: Some(span_of(tok)),
            repair: None,
            confidence: 1.0,
        });
    }

    /// Add a node and record the span of the token that produced it, so
    /// evaluation-time diagnostics can point at real source positions.
    fn add_spanned(&mut self, node: Node, tok: &Token) -> NodeId {
        let id = self.module.add_node(node);
        self.module.record_span(id, span_of(tok));
        id
    }

    // ------------------------------------------------------------------
    // Healing (C-016): syntactic repairs with confidence scoring
    // ------------------------------------------------------------------

    /// Record a healing diagnostic. The confidence policy decides whether
    /// the repair is applied at all:
    /// - ≥ 0.95: applied in every mode (Info)
    /// - 0.85–0.94: applied in every mode (Warning; strict fails on it at
    ///   the exit-code level)
    /// - 0.70–0.84: applied only in resilient mode
    /// - < 0.70: never applied — the caller's error path stands
    fn heal(
        &mut self,
        code: &str,
        message: String,
        tok: &Token,
        repair: Repair,
        confidence: f64,
    ) -> bool {
        let applies = confidence >= thresholds::HIGH
            || (self.options.resilient && confidence >= thresholds::MEDIUM);
        if !applies {
            return false;
        }
        self.module.healing.push(Diagnostic {
            severity: if confidence >= thresholds::AUTO_APPLY {
                Severity::Info
            } else {
                Severity::Warning
            },
            code: code.into(),
            message,
            path: None,
            span: Some(span_of(tok)),
            repair: Some(repair),
            confidence,
        });
        true
    }

    /// Consume a closing delimiter, healing a bracket/brace mismatch when
    /// the mode allows it (H004, medium confidence: single-token
    /// substitution). Returns true when the delimiter was consumed.
    fn expect_closing_healing(&mut self, expected: TokenKind, heal_code: &str) -> bool {
        let next = self.peek().clone();
        if next.kind == expected {
            self.bump();
            return true;
        }
        let healable = matches!(
            (&expected, &next.kind),
            (TokenKind::RBracket, TokenKind::RBrace) | (TokenKind::RBrace, TokenKind::RBracket)
        );
        if healable {
            let applied = self.heal(
                heal_code,
                format!(
                    "mismatched closing delimiter; replaced {:?} with {:?}",
                    next.kind, expected
                ),
                &next,
                Repair {
                    original: format!("{:?}", next.kind),
                    suggested: format!("{:?}", expected),
                    kind: RepairKind::Other("bracket_mismatch".into()),
                    safe: false,
                },
                thresholds::MEDIUM + 0.12, // 0.82 — resilient mode only
            );
            if applied {
                self.bump();
                return true;
            }
        }
        // Not healed: report the missing delimiter and leave the token for
        // the caller's error path.
        self.error(
            "E001",
            format!("expected {:?}, found {:?}", expected, next.kind),
        );
        false
    }

    // ------------------------------------------------------------------
    // Document entry point
    // ------------------------------------------------------------------

    fn parse_document(&mut self) {
        let mut root_fields = indexmap::IndexMap::new();
        self.ident_scopes.push(Vec::new());

        self.parse_body(&mut root_fields, BodyEnd::Document);

        let root_idents = self.ident_scopes.pop().unwrap_or_default();
        self.resolve_sibling_refs(root_idents, &root_fields);

        let root_id = self.module.add_node(Node::Table {
            fields: root_fields,
        });
        self.module.root = Some(root_id);
    }

    // ------------------------------------------------------------------
    // let name = expr
    // ------------------------------------------------------------------

    fn parse_let(&mut self) {
        let let_tok = self.peek().clone();
        self.bump(); // consume `let`
        let name_tok = match self.expect(TokenKind::Ident) {
            Some(t) => t,
            None => return,
        };
        if self.expect(TokenKind::Assign).is_none() {
            return;
        }
        let expr = self.parse_expression();
        self.record_stmt(
            &name_tok.text,
            let_tok.line,
            self.last_end_line(),
            let_tok.col.saturating_sub(1) as usize,
        );
        self.module.bindings.insert(name_tok.text, expr);
    }

    // ------------------------------------------------------------------
    // [table] … key = expr
    // ------------------------------------------------------------------

    fn parse_table_header(&mut self, parent: &mut indexmap::IndexMap<String, NodeId>) {
        let open = self.peek().clone();
        // Source columns are 1-based; indentation levels are 0-based.
        let header_level = open.col.saturating_sub(1) as usize;
        self.bump(); // [
        let name = match self.expect(TokenKind::Ident) {
            Some(t) => t.text,
            None => return,
        };
        // TODO(solid C-021): dotted table paths [a.b.c] (nested tables)
        if self.expect(TokenKind::RBracket).is_none() {
            return;
        }
        self.brace_tables += 1;
        let stmt_idx = self.statements.len();
        self.statements.push(StmtInfo {
            key: name.clone(),
            path: self.path_str(),
            start_line: open.line,
            end_line: open.line,
            level: header_level,
        });
        self.current_path.push(name.clone());
        self.blocks.push(BlockInfo {
            path: self.path_str(),
            level: header_level,
            start_line: open.line,
        });

        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        self.parse_body(&mut fields, BodyEnd::HeaderBody(header_level));
        let idents = self.ident_scopes.pop().unwrap_or_default();
        self.resolve_sibling_refs(idents, &fields);
        let table_id = self.module.add_node(Node::Table { fields });
        self.statements[stmt_idx].end_line = self.last_end_line();
        self.current_path.pop();
        parent.insert(name, table_id);
    }

    // ------------------------------------------------------------------
    // key = expr
    // ------------------------------------------------------------------

    fn parse_key_value(&mut self) -> Option<(String, NodeId)> {
        self.parse_key_value_sep(TokenKind::Assign)
    }

    /// `key = expr` or `key: expr` (spec/dual-syntax.md §2).
    fn parse_key_value_sep(&mut self, sep: TokenKind) -> Option<(String, NodeId)> {
        let key = self.expect(TokenKind::Ident)?.text;
        self.expect(sep)?;
        // Tag Ident nodes created directly in this key's value expression
        // with the owning key so sibling resolution can skip self-references.
        let scope_start = self.ident_scopes.last().map_or(0, |s| s.len());
        let value = self.parse_expression();
        if let Some(scope) = self.ident_scopes.last_mut() {
            for entry in scope.iter_mut().skip(scope_start) {
                entry.0 = key.clone();
            }
        }
        Some((key, value))
    }

    /// Rewrite Ident nodes collected in `idents` that match a sibling key of
    /// the just-parsed scope into Ref nodes with dependency edges. Runs after
    /// the whole scope is parsed, so forward references within the scope
    /// resolve too.
    fn resolve_sibling_refs(
        &mut self,
        idents: Vec<(String, NodeId)>,
        fields: &indexmap::IndexMap<String, NodeId>,
    ) {
        for (owner_key, id) in idents {
            let name = match self.module.get(id) {
                Some(Node::Ident(n)) => n.clone(),
                _ => continue,
            };
            // A key's own value expression must not resolve to its own
            // binding; it falls through to let-binding resolution instead.
            if name == owner_key {
                continue;
            }
            if let Some(&target) = fields.get(&name) {
                if let Some(slot) = self.module.get_mut(id) {
                    *slot = Node::Ref(target);
                }
                self.module.add_edge(target, id);
            }
        }
    }

    // ------------------------------------------------------------------
    // Expression parser — classic precedence climbing
    // ------------------------------------------------------------------

    fn parse_expression(&mut self) -> NodeId {
        let guard_tok = self.peek().clone();
        self.depth += 1;
        if self.depth > MAX_EXPR_DEPTH {
            self.error(
                "E004",
                format!("maximum expression nesting depth ({MAX_EXPR_DEPTH}) exceeded"),
            );
            self.depth -= 1;
            self.bump(); // consume the current token to guarantee progress
            return self.add_spanned(Node::Literal(Value::Null), &guard_tok);
        }
        let node = self.parse_or();
        self.depth -= 1;
        node
    }

    fn parse_or(&mut self) -> NodeId {
        let mut left = self.parse_and();
        while matches!(self.peek_kind(), TokenKind::OrOr) {
            let op_tok = self.bump();
            let right = self.parse_and();
            left = self.add_spanned(
                Node::Binary {
                    op: BinOp::Or,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_and(&mut self) -> NodeId {
        let mut left = self.parse_equality();
        while matches!(self.peek_kind(), TokenKind::AndAnd) {
            let op_tok = self.bump();
            let right = self.parse_equality();
            left = self.add_spanned(
                Node::Binary {
                    op: BinOp::And,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_equality(&mut self) -> NodeId {
        let mut left = self.parse_comparison();
        loop {
            let op = match self.peek_kind() {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::NotEq => BinOp::Ne,
                _ => break,
            };
            let op_tok = self.bump();
            let right = self.parse_comparison();
            left = self.add_spanned(
                Node::Binary {
                    op,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_comparison(&mut self) -> NodeId {
        let mut left = self.parse_term();
        loop {
            let op = match self.peek_kind() {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::LtEq => BinOp::Le,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::GtEq => BinOp::Ge,
                _ => break,
            };
            let op_tok = self.bump();
            let right = self.parse_term();
            left = self.add_spanned(
                Node::Binary {
                    op,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_term(&mut self) -> NodeId {
        let mut left = self.parse_factor();
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            let op_tok = self.bump();
            let right = self.parse_factor();
            left = self.add_spanned(
                Node::Binary {
                    op,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_factor(&mut self) -> NodeId {
        let mut left = self.parse_unary();
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                _ => break,
            };
            let op_tok = self.bump();
            let right = self.parse_unary();
            left = self.add_spanned(
                Node::Binary {
                    op,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_unary(&mut self) -> NodeId {
        match self.peek_kind() {
            TokenKind::Minus | TokenKind::Bang => {
                let op_tok = self.peek().clone();
                // Unary chains recurse through parse_unary, so the depth
                // guard must live here too (parse_expression cannot see it).
                self.depth += 1;
                if self.depth > MAX_EXPR_DEPTH {
                    self.error(
                        "E004",
                        format!("maximum expression nesting depth ({MAX_EXPR_DEPTH}) exceeded"),
                    );
                    self.depth -= 1;
                    self.bump(); // consume the operator to guarantee progress
                    return self.add_spanned(Node::Literal(Value::Null), &op_tok);
                }
                let op = if matches!(self.peek_kind(), TokenKind::Minus) {
                    UnOp::Neg
                } else {
                    UnOp::Not
                };
                self.bump();
                let expr = self.parse_unary();
                self.depth -= 1;
                self.add_spanned(Node::Unary { op, expr }, &op_tok)
            }
            _ => self.parse_primary(),
        }
    }

    // ------------------------------------------------------------------
    // Primary expressions
    // ------------------------------------------------------------------

    fn parse_primary(&mut self) -> NodeId {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::String => {
                self.bump();
                self.parse_string_literal(&tok.text, &tok)
            }

            TokenKind::Integer => {
                self.bump();
                let n: i64 = tok.text.parse().unwrap_or(0);
                let mut node = self.add_spanned(Node::Literal(Value::Int(n)), &tok);
                // optional unit suffix
                if let TokenKind::Unit(_) = self.peek_kind() {
                    let unit_tok = self.bump();
                    if let Some(unit) = Unit::from_suffix(&unit_tok.text) {
                        node = self.add_spanned(Node::Quantity { value: node, unit }, &tok);
                    }
                }
                node
            }

            TokenKind::Float => {
                self.bump();
                let f: f64 = tok.text.parse().unwrap_or(0.0);
                let mut node = self.add_spanned(Node::Literal(Value::Float(f)), &tok);
                if let TokenKind::Unit(_) = self.peek_kind() {
                    let unit_tok = self.bump();
                    if let Some(unit) = Unit::from_suffix(&unit_tok.text) {
                        node = self.add_spanned(Node::Quantity { value: node, unit }, &tok);
                    }
                }
                node
            }

            TokenKind::True => {
                self.bump();
                self.add_spanned(Node::Literal(Value::Bool(true)), &tok)
            }
            TokenKind::False => {
                self.bump();
                self.add_spanned(Node::Literal(Value::Bool(false)), &tok)
            }
            TokenKind::Null => {
                self.bump();
                self.add_spanned(Node::Literal(Value::Null), &tok)
            }

            TokenKind::Env => {
                self.bump();
                self.expect(TokenKind::Dot);
                let var = self
                    .expect(TokenKind::Ident)
                    .map(|t| t.text)
                    .unwrap_or_default();
                let default = if matches!(self.peek_kind(), TokenKind::QuestionColon) {
                    self.bump();
                    Some(self.parse_expression())
                } else {
                    None
                };
                self.add_spanned(Node::Env { name: var, default }, &tok)
            }

            TokenKind::Secret => {
                self.bump();
                // `secret(expr)` — locator is the expression; or
                // `secret.env(name)` — name is a string literal and the
                // secret is the env var's value.
                let from_env = if matches!(self.peek_kind(), TokenKind::Dot) {
                    self.bump();
                    if !matches!(self.peek_kind(), TokenKind::Env) {
                        self.error(
                            "E003",
                            "expected `env` after `secret.` (use secret.env(name))",
                        );
                    } else {
                        self.bump();
                    }
                    true
                } else {
                    false
                };
                self.expect(TokenKind::LParen);
                let arg = self.parse_expression();
                self.expect(TokenKind::RParen);
                let locator = if from_env {
                    match self.module.get(arg) {
                        Some(Node::Literal(Value::String(name))) => self.add_spanned(
                            Node::Env {
                                name: name.clone(),
                                default: None,
                            },
                            &tok,
                        ),
                        _ => {
                            self.error(
                                "E003",
                                "secret.env() requires a string literal env var name",
                            );
                            arg
                        }
                    }
                } else {
                    arg
                };
                self.add_spanned(Node::Secret { locator }, &tok)
            }

            TokenKind::Ident => {
                self.bump();
                // Will be turned into Ref during build_graph if it matches a
                // let, or during sibling resolution if it matches a key in the
                // enclosing table scope.
                let text = tok.text.clone();
                let id = self.add_spanned(Node::Ident(text), &tok);
                if let Some(scope) = self.ident_scopes.last_mut() {
                    scope.push((String::new(), id));
                }
                id
            }

            TokenKind::LBracket => self.parse_array(),
            TokenKind::LBrace => self.parse_inline_table(),

            TokenKind::LParen => {
                self.bump();
                let expr = self.parse_expression();
                self.expect(TokenKind::RParen);
                expr
            }

            _ => {
                self.error("E002", format!("unexpected token {:?}", tok.kind));
                self.bump();
                self.module.add_node(Node::Literal(Value::Null))
            }
        }
    }

    /// Convert a raw string that may contain `${…}` into either a plain
    /// Literal or a full Interpolate node with Expr parts.
    fn parse_string_literal(&mut self, raw: &str, tok: &Token) -> NodeId {
        if !raw.contains("${") {
            return self.add_spanned(Node::Literal(Value::String(raw.to_string())), tok);
        }

        let mut parts: Vec<InterpPart> = Vec::new();
        let mut rest = raw;

        while let Some(start) = rest.find("${") {
            if start > 0 {
                parts.push(InterpPart::Literal(rest[..start].to_string()));
            }
            rest = &rest[start + 2..];
            if let Some(end) = rest.find('}') {
                let name = rest[..end].trim();
                // Alpha: only simple identifiers inside ${}.
                // TODO(solid C-023): full expression parsing inside the braces.
                let id = self.add_spanned(Node::Ident(name.to_string()), tok);
                if let Some(scope) = self.ident_scopes.last_mut() {
                    scope.push((String::new(), id));
                }
                parts.push(InterpPart::Expr(id));
                rest = &rest[end + 1..];
            } else {
                // Unclosed — treat remainder as literal
                parts.push(InterpPart::Literal(format!("${{{}", rest)));
                rest = "";
                break;
            }
        }
        if !rest.is_empty() {
            parts.push(InterpPart::Literal(rest.to_string()));
        }

        self.add_spanned(Node::Interpolate { parts }, tok)
    }

    fn parse_array(&mut self) -> NodeId {
        let open = self.peek().clone();
        self.bump(); // [
        let mut elements = Vec::new();
        loop {
            self.skip_newlines();
            // RBrace is a mismatched closer: stop so the closing-delimiter
            // healer (H004) can see it instead of parse_primary eating it.
            if matches!(
                self.peek_kind(),
                TokenKind::RBracket | TokenKind::RBrace | TokenKind::Eof
            ) {
                break;
            }
            if matches!(self.peek_kind(), TokenKind::Comma) {
                // Stray or repeated comma: heal as a removal.
                let comma = self.bump();
                if !self.heal(
                    "H002",
                    "removed stray comma".into(),
                    &comma,
                    Repair {
                        original: ",".into(),
                        suggested: String::new(),
                        kind: RepairKind::RemoveTrailingComma,
                        safe: true,
                    },
                    thresholds::AUTO_APPLY + 0.03,
                ) {
                    self.error("E002", "unexpected comma");
                }
                continue;
            }
            elements.push(self.parse_expression());
            self.skip_newlines();
            if matches!(self.peek_kind(), TokenKind::Comma) {
                let comma = self.bump();
                self.skip_newlines();
                if matches!(
                    self.peek_kind(),
                    TokenKind::RBracket | TokenKind::RBrace | TokenKind::Eof
                ) {
                    self.heal(
                        "H002",
                        "removed trailing comma before closing bracket".into(),
                        &comma,
                        Repair {
                            original: ",".into(),
                            suggested: String::new(),
                            kind: RepairKind::RemoveTrailingComma,
                            safe: true,
                        },
                        thresholds::AUTO_APPLY + 0.03,
                    );
                }
            } else if !matches!(
                self.peek_kind(),
                TokenKind::RBracket | TokenKind::RBrace | TokenKind::Eof
            ) {
                // Missing comma between elements.
                let tok = self.peek().clone();
                if !self.heal(
                    "H003",
                    "inserted missing comma between array elements".into(),
                    &tok,
                    Repair {
                        original: String::new(),
                        suggested: ",".into(),
                        kind: RepairKind::Other("insert_comma".into()),
                        safe: false,
                    },
                    thresholds::HIGH + 0.05, // 0.90 — auto-apply with warning
                ) {
                    self.error(
                        "E001",
                        format!("expected `,` between array elements, found {:?}", tok.kind),
                    );
                }
            }
        }
        self.expect_closing_healing(TokenKind::RBracket, "H004");
        self.add_spanned(Node::Array { elements }, &open)
    }

    fn parse_inline_table(&mut self) -> NodeId {
        let open = self.peek().clone();
        self.bump(); // {
        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        loop {
            self.skip_newlines();
            if matches!(
                self.peek_kind(),
                TokenKind::RBrace | TokenKind::RBracket | TokenKind::Eof
            ) {
                break;
            }
            if matches!(self.peek_kind(), TokenKind::Comma) {
                let comma = self.bump();
                if !self.heal(
                    "H002",
                    "removed stray comma".into(),
                    &comma,
                    Repair {
                        original: ",".into(),
                        suggested: String::new(),
                        kind: RepairKind::RemoveTrailingComma,
                        safe: true,
                    },
                    thresholds::AUTO_APPLY + 0.03,
                ) {
                    self.error("E002", "unexpected comma");
                }
                continue;
            }
            if let Some((k, id)) = self.parse_key_value() {
                fields.insert(k, id);
            } else {
                break;
            }
            self.skip_newlines();
            if matches!(self.peek_kind(), TokenKind::Comma) {
                let comma = self.bump();
                self.skip_newlines();
                if matches!(
                    self.peek_kind(),
                    TokenKind::RBrace | TokenKind::RBracket | TokenKind::Eof
                ) {
                    self.heal(
                        "H002",
                        "removed trailing comma before closing brace".into(),
                        &comma,
                        Repair {
                            original: ",".into(),
                            suggested: String::new(),
                            kind: RepairKind::RemoveTrailingComma,
                            safe: true,
                        },
                        thresholds::AUTO_APPLY + 0.03,
                    );
                }
            } else if !matches!(
                self.peek_kind(),
                TokenKind::RBrace | TokenKind::RBracket | TokenKind::Eof
            ) {
                let tok = self.peek().clone();
                if !self.heal(
                    "H003",
                    "inserted missing comma between table entries".into(),
                    &tok,
                    Repair {
                        original: String::new(),
                        suggested: ",".into(),
                        kind: RepairKind::Other("insert_comma".into()),
                        safe: false,
                    },
                    thresholds::HIGH + 0.05, // 0.90 — auto-apply with warning
                ) {
                    self.error(
                        "E001",
                        format!("expected `,` between table entries, found {:?}", tok.kind),
                    );
                }
            }
        }
        self.expect_closing_healing(TokenKind::RBrace, "H004");
        let idents = self.ident_scopes.pop().unwrap_or_default();
        self.resolve_sibling_refs(idents, &fields);
        self.add_spanned(Node::Table { fields }, &open)
    }

    // ------------------------------------------------------------------
    // Dependency-graph construction & Ident → Ref resolution
    // ------------------------------------------------------------------

    fn build_graph(&mut self) {
        let bindings = self.module.bindings.clone();

        for i in 0..self.module.arena.len() {
            let id = NodeId(i as u32);
            let node = self.module.get(id).cloned();
            match node {
                Some(Node::Ident(name)) => {
                    if let Some(&target) = bindings.get(&name) {
                        if let Some(slot) = self.module.get_mut(id) {
                            *slot = Node::Ref(target);
                        }
                        // edge: dependency → dependent
                        self.module.add_edge(target, id);
                    }
                }
                Some(Node::Env {
                    default: Some(def), ..
                }) => {
                    self.module.add_edge(def, id);
                }
                Some(Node::Interpolate { parts }) => {
                    for part in &parts {
                        if let InterpPart::Expr(expr_id) = part {
                            self.module.add_edge(*expr_id, id);
                        }
                    }
                }
                Some(Node::Binary { lhs, rhs, .. }) => {
                    self.module.add_edge(lhs, id);
                    self.module.add_edge(rhs, id);
                }
                Some(Node::Unary { expr, .. }) => {
                    self.module.add_edge(expr, id);
                }
                Some(Node::Quantity { value, .. }) => {
                    self.module.add_edge(value, id);
                }
                Some(Node::Array { elements }) => {
                    for e in &elements {
                        self.module.add_edge(*e, id);
                    }
                }
                Some(Node::Table { fields }) => {
                    for (_, fid) in &fields {
                        self.module.add_edge(*fid, id);
                    }
                }
                Some(Node::Secret { locator }) => {
                    self.module.add_edge(locator, id);
                }
                _ => {}
            }
        }
    }
}
