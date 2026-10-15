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

use crate::diag::{Diagnostic, Severity, Span};
use crate::ir::*;
use crate::types::Unit;
use lexer::{Lexer, Token, TokenKind};

pub fn parse(source: &str, name: &str) -> Result<Module, Vec<Diagnostic>> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize();
    let mut parser = Parser {
        tokens,
        pos: 0,
        module: Module::new(name),
        errors: Vec::new(),
        ident_scopes: Vec::new(),
    };
    parser.parse_document();
    if parser.errors.is_empty() {
        parser.build_graph();
        Ok(parser.module)
    } else {
        Err(parser.errors)
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

    // ------------------------------------------------------------------
    // Document entry point
    // ------------------------------------------------------------------

    fn parse_document(&mut self) {
        let mut root_fields = indexmap::IndexMap::new();
        self.ident_scopes.push(Vec::new());

        while !matches!(self.peek_kind(), TokenKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek_kind(), TokenKind::Eof) {
                break;
            }

            match self.peek_kind() {
                TokenKind::Let => self.parse_let(),
                TokenKind::LBracket => self.parse_table_header(&mut root_fields),
                TokenKind::Ident => {
                    if let Some((k, id)) = self.parse_key_value() {
                        root_fields.insert(k, id);
                    }
                }
                _ => {
                    // Alpha resilience: skip unexpected token instead of dying
                    self.bump();
                }
            }
        }

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
        self.bump(); // consume `let`
        let name_tok = match self.expect(TokenKind::Ident) {
            Some(t) => t,
            None => return,
        };
        if self.expect(TokenKind::Assign).is_none() {
            return;
        }
        let expr = self.parse_expression();
        self.module.bindings.insert(name_tok.text, expr);
    }

    // ------------------------------------------------------------------
    // [table] … key = expr
    // ------------------------------------------------------------------

    fn parse_table_header(&mut self, parent: &mut indexmap::IndexMap<String, NodeId>) {
        self.bump(); // [
        let name = match self.expect(TokenKind::Ident) {
            Some(t) => t.text,
            None => return,
        };
        // TODO(solid C-021): dotted table paths [a.b.c] (nested tables)
        if self.expect(TokenKind::RBracket).is_none() {
            return;
        }

        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        loop {
            self.skip_newlines();
            match self.peek_kind() {
                TokenKind::Eof | TokenKind::LBracket | TokenKind::Let => break,
                TokenKind::Ident => {
                    if let Some((k, id)) = self.parse_key_value() {
                        fields.insert(k, id);
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        let idents = self.ident_scopes.pop().unwrap_or_default();
        self.resolve_sibling_refs(idents, &fields);
        let table_id = self.module.add_node(Node::Table { fields });
        parent.insert(name, table_id);
    }

    // ------------------------------------------------------------------
    // key = expr
    // ------------------------------------------------------------------

    fn parse_key_value(&mut self) -> Option<(String, NodeId)> {
        let key = self.expect(TokenKind::Ident)?.text;
        self.expect(TokenKind::Assign)?;
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
        self.parse_or()
    }

    fn parse_or(&mut self) -> NodeId {
        let mut left = self.parse_and();
        while matches!(self.peek_kind(), TokenKind::OrOr) {
            self.bump();
            let right = self.parse_and();
            left = self.module.add_node(Node::Binary {
                op: BinOp::Or,
                lhs: left,
                rhs: right,
            });
        }
        left
    }

    fn parse_and(&mut self) -> NodeId {
        let mut left = self.parse_equality();
        while matches!(self.peek_kind(), TokenKind::AndAnd) {
            self.bump();
            let right = self.parse_equality();
            left = self.module.add_node(Node::Binary {
                op: BinOp::And,
                lhs: left,
                rhs: right,
            });
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
            self.bump();
            let right = self.parse_comparison();
            left = self.module.add_node(Node::Binary {
                op,
                lhs: left,
                rhs: right,
            });
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
            self.bump();
            let right = self.parse_term();
            left = self.module.add_node(Node::Binary {
                op,
                lhs: left,
                rhs: right,
            });
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
            self.bump();
            let right = self.parse_factor();
            left = self.module.add_node(Node::Binary {
                op,
                lhs: left,
                rhs: right,
            });
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
            self.bump();
            let right = self.parse_unary();
            left = self.module.add_node(Node::Binary {
                op,
                lhs: left,
                rhs: right,
            });
        }
        left
    }

    fn parse_unary(&mut self) -> NodeId {
        match self.peek_kind() {
            TokenKind::Minus => {
                self.bump();
                let expr = self.parse_unary();
                self.module.add_node(Node::Unary {
                    op: UnOp::Neg,
                    expr,
                })
            }
            TokenKind::Bang => {
                self.bump();
                let expr = self.parse_unary();
                self.module.add_node(Node::Unary {
                    op: UnOp::Not,
                    expr,
                })
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
                self.parse_string_literal(&tok.text)
            }

            TokenKind::Integer => {
                self.bump();
                let n: i64 = tok.text.parse().unwrap_or(0);
                let mut node = self.module.add_node(Node::Literal(Value::Int(n)));
                // optional unit suffix
                if let TokenKind::Unit(_) = self.peek_kind() {
                    let unit_tok = self.bump();
                    if let Some(unit) = Unit::from_suffix(&unit_tok.text) {
                        node = self.module.add_node(Node::Quantity { value: node, unit });
                    }
                }
                node
            }

            TokenKind::Float => {
                self.bump();
                let f: f64 = tok.text.parse().unwrap_or(0.0);
                let mut node = self.module.add_node(Node::Literal(Value::Float(f)));
                if let TokenKind::Unit(_) = self.peek_kind() {
                    let unit_tok = self.bump();
                    if let Some(unit) = Unit::from_suffix(&unit_tok.text) {
                        node = self.module.add_node(Node::Quantity { value: node, unit });
                    }
                }
                node
            }

            TokenKind::True => {
                self.bump();
                self.module.add_node(Node::Literal(Value::Bool(true)))
            }
            TokenKind::False => {
                self.bump();
                self.module.add_node(Node::Literal(Value::Bool(false)))
            }
            TokenKind::Null => {
                self.bump();
                self.module.add_node(Node::Literal(Value::Null))
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
                self.module.add_node(Node::Env { name: var, default })
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
                        Some(Node::Literal(Value::String(name))) => {
                            self.module.add_node(Node::Env {
                                name: name.clone(),
                                default: None,
                            })
                        }
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
                self.module.add_node(Node::Secret { locator })
            }

            TokenKind::Ident => {
                self.bump();
                // Will be turned into Ref during build_graph if it matches a
                // let, or during sibling resolution if it matches a key in the
                // enclosing table scope.
                let id = self.module.add_node(Node::Ident(tok.text));
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
    fn parse_string_literal(&mut self, raw: &str) -> NodeId {
        if !raw.contains("${") {
            return self
                .module
                .add_node(Node::Literal(Value::String(raw.to_string())));
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
                let id = self.module.add_node(Node::Ident(name.to_string()));
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

        self.module.add_node(Node::Interpolate { parts })
    }

    fn parse_array(&mut self) -> NodeId {
        self.bump(); // [
        let mut elements = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek_kind(), TokenKind::RBracket | TokenKind::Eof) {
                break;
            }
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
                continue;
            }
            elements.push(self.parse_expression());
            self.skip_newlines();
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
            }
        }
        self.expect(TokenKind::RBracket);
        self.module.add_node(Node::Array { elements })
    }

    fn parse_inline_table(&mut self) -> NodeId {
        self.bump(); // {
        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                break;
            }
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
                continue;
            }
            if let Some((k, id)) = self.parse_key_value() {
                fields.insert(k, id);
            } else {
                break;
            }
            self.skip_newlines();
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
            }
        }
        self.expect(TokenKind::RBrace);
        let idents = self.ident_scopes.pop().unwrap_or_default();
        self.resolve_sibling_refs(idents, &fields);
        self.module.add_node(Node::Table { fields })
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
