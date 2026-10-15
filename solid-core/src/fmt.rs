//! Dual-style formatter (C-022).
//!
//! Re-emits a parsed module in **brace style**, **indentation style**, or
//! the **source's own style**, preserving every comment (with its normative
//! attachment, spec/dual-syntax.md §7) and canonical blank-line structure.
//!
//! Canonical layout (spec §8):
//! - indentation style: two spaces per level, `key:` headers;
//! - brace style: `[name]` headers at the top level, `key = value` pairs,
//!   nested tables as `name { … }` blocks;
//! - no blank lines inside a block; one blank line between top-level
//!   entries when either side is a table;
//! - a leading comment sits directly above its statement, a trailing
//!   comment on the same line after the value.
//!
//! The guarantees tested in C-022 are: idempotence (`fmt(fmt(d)) == fmt(d)`),
//! byte-for-byte preservation of canonical documents, and comment survival
//! in every mode.

use crate::ir::*;
use std::collections::HashMap;

const INDENT: &str = "  ";

/// Format a module in the requested style. `Preserve` uses the style the
/// source predominantly used.
pub fn format(module: &Module, style: Style) -> String {
    let style = match style {
        Style::Preserve => module.source_style,
        other => other,
    };
    let mut f = Formatter {
        module,
        style,
        names: build_names(module),
        leading: HashMap::new(),
        trailing: HashMap::new(),
        trailing_block: HashMap::new(),
        trailing_document: Vec::new(),
        out: String::new(),
    };
    f.index_trivia();
    f.emit_module();
    f.out
}

struct Formatter<'a> {
    module: &'a Module,
    style: Style,
    /// Reverse map: node id → the name it is bound or keyed under.
    names: HashMap<NodeId, String>,
    leading: HashMap<String, Vec<String>>,
    trailing: HashMap<String, Vec<String>>,
    trailing_block: HashMap<String, Vec<String>>,
    trailing_document: Vec<String>,
    out: String,
}

/// Map every named node back to its name (let bindings and table keys).
fn build_names(module: &Module) -> HashMap<NodeId, String> {
    fn walk(module: &Module, id: NodeId, names: &mut HashMap<NodeId, String>) {
        if let Some(Node::Table { fields }) = module.get(id) {
            for (key, child) in fields {
                names.entry(*child).or_insert_with(|| key.clone());
                walk(module, *child, names);
            }
        }
    }
    let mut names = HashMap::new();
    for (name, id) in &module.bindings {
        names.entry(*id).or_insert_with(|| name.clone());
    }
    if let Some(root) = module.root {
        walk(module, root, &mut names);
    }
    names
}

impl<'a> Formatter<'a> {
    fn index_trivia(&mut self) {
        for c in &self.module.trivia {
            match &c.attachment {
                Attachment::Leading(key) => self
                    .leading
                    .entry(key.clone())
                    .or_default()
                    .push(c.text.clone()),
                Attachment::Trailing(key) => {
                    // Duplicate keys share one surviving statement; keep
                    // every comment so none is lost.
                    self.trailing
                        .entry(key.clone())
                        .or_default()
                        .push(c.text.clone());
                }
                Attachment::TrailingBlock(path) => self
                    .trailing_block
                    .entry(path.clone())
                    .or_default()
                    .push(c.text.clone()),
                Attachment::TrailingDocument => self.trailing_document.push(c.text.clone()),
            }
        }
    }

    fn emit_module(&mut self) {
        // Let bindings first, in source order.
        let mut lets: Vec<(&String, &NodeId)> = self.module.bindings.iter().collect();
        lets.sort_by_key(|(_, id)| {
            self.module
                .node_spans
                .get(id)
                .map(|s| (s.line, s.column))
                .unwrap_or((u32::MAX, 0))
        });
        for (name, id) in lets {
            let id = *id;
            self.emit_leading(name);
            let value = self.render_expr(id);
            self.out.push_str(&format!("let {name} = {value}"));
            self.emit_trailing(name);
            self.out.push('\n');
        }

        if let Some(root) = self.module.root {
            let has_entries = !self.out.is_empty()
                && matches!(self.module.get(root), Some(Node::Table { fields }) if !fields.is_empty());
            if has_entries && !self.out.ends_with("\n\n") {
                self.out.push('\n');
            }
            self.emit_table_body(root, 0, "", true);
        }
        for text in self.trailing_document.clone() {
            self.out.push_str(&text);
            self.out.push('\n');
        }
    }

    /// Emit a table's entries. `path` is the dotted path of the table
    /// (empty for the document root); `is_root` marks the document level.
    fn emit_table_body(&mut self, id: NodeId, depth: usize, path: &str, is_root: bool) {
        let fields = match self.module.get(id) {
            Some(Node::Table { fields }) => fields.clone(),
            _ => return,
        };
        let is_table_node =
            |f: &Formatter, n: &NodeId| matches!(f.module.get(*n), Some(Node::Table { .. }));
        // At the document root, scalars must be hoisted above tables: a
        // `key = value` line after a table header would otherwise be
        // absorbed by that table (spec §5.1). Inside a block, the original
        // order is preserved.
        let ordered: Vec<(String, NodeId)> = if is_root {
            let mut scalars: Vec<(String, NodeId)> = Vec::new();
            let mut tables: Vec<(String, NodeId)> = Vec::new();
            for (k, v) in &fields {
                if is_table_node(self, v) {
                    tables.push((k.clone(), *v));
                } else {
                    scalars.push((k.clone(), *v));
                }
            }
            scalars.into_iter().chain(tables).collect()
        } else {
            fields.iter().map(|(k, v)| (k.clone(), *v)).collect()
        };
        let tables: Vec<(String, NodeId)> = ordered
            .iter()
            .filter(|(_, v)| is_table_node(self, v))
            .cloned()
            .collect();

        // Scalars-first at the root, source order inside blocks.
        let mut first = true;
        for (key, node) in ordered.iter() {
            let child_path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            let is_table = tables.iter().any(|(k, _)| k == key);
            if is_root && !first {
                self.out.push('\n');
            }
            first = false;

            let qkey = child_path.clone();
            self.emit_leading(&qkey);
            if is_table {
                self.emit_table_block(*node, &child_path, key, depth, is_root);
            } else {
                let value = self.render_expr(*node);
                let indent = INDENT.repeat(depth);
                self.out.push_str(&format!("{indent}{key} = {value}"));
                self.emit_trailing(&qkey);
                self.out.push('\n');
            }
        }

        if let Some(comments) = self.trailing_block.get(path).cloned() {
            for text in comments {
                self.out.push_str(&INDENT.repeat(depth));
                self.out.push_str(&text);
                self.out.push('\n');
            }
        }
    }

    fn emit_table_block(&mut self, id: NodeId, path: &str, key: &str, depth: usize, is_root: bool) {
        match self.style {
            Style::Indent => {
                // Indentation style cannot express a bodyless block, so an
                // empty table is emitted in its inline form (`key = {}`).
                let empty = matches!(
                    self.module.get(id),
                    Some(Node::Table { fields }) if fields.is_empty()
                );
                if empty {
                    self.out
                        .push_str(&format!("{}{key} = {{}}", INDENT.repeat(depth)));
                    self.emit_trailing(path);
                    self.out.push('\n');
                    // An empty table can still own trailing comments.
                    if let Some(comments) = self.trailing_block.get(path).cloned() {
                        for text in comments {
                            self.out.push_str(&INDENT.repeat(depth));
                            self.out.push_str(&text);
                            self.out.push('\n');
                        }
                    }
                    return;
                }
                self.out
                    .push_str(&format!("{}{key}:", INDENT.repeat(depth)));
                self.emit_trailing(path);
                self.out.push('\n');
                self.emit_table_body(id, depth + 1, path, false);
            }
            _ => {
                if is_root && depth == 0 {
                    // Top-level tables use `[name]` headers (TOML-ish); a
                    // trailing comment stays on the header line.
                    self.out.push_str(&format!("[{key}]"));
                    self.emit_trailing(path);
                    self.out.push('\n');
                    self.emit_table_body(id, 0, path, false);
                } else {
                    self.out
                        .push_str(&format!("{}{key} {{", INDENT.repeat(depth)));
                    self.emit_trailing(path);
                    self.out.push('\n');
                    self.emit_table_body(id, depth + 1, path, false);
                    self.out.push_str(&format!("{}}}\n", INDENT.repeat(depth)));
                }
            }
        }
    }

    fn emit_leading(&mut self, key: &str) {
        if let Some(comments) = self.leading.get(key).cloned() {
            for text in comments {
                self.out.push_str(&text);
                self.out.push('\n');
            }
        }
    }

    fn emit_trailing(&mut self, key: &str) {
        if let Some(texts) = self.trailing.get(key).cloned() {
            for text in texts {
                self.out.push(' ');
                self.out.push_str(&text);
            }
        }
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    fn render_expr(&self, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::Literal(v)) => render_literal(v),
            Some(Node::Env { name, default }) => match default {
                Some(d) => {
                    let d = self.render_expr(*d);
                    format!("env.{name} ?: {d}")
                }
                None => format!("env.{name}"),
            },
            Some(Node::Secret { locator }) => {
                format!("secret({})", self.render_expr(*locator))
            }
            Some(Node::Ref(target)) => self
                .names
                .get(target)
                .cloned()
                .unwrap_or_else(|| "<?>".into()),
            Some(Node::Ident(name)) => name.clone(),
            Some(Node::Interpolate { parts }) => {
                let mut s = String::from("\"");
                for part in parts {
                    match part {
                        InterpPart::Literal(t) => s.push_str(&escape(t)),
                        InterpPart::Expr(e) => {
                            s.push_str("${");
                            s.push_str(&self.render_expr(*e));
                            s.push('}');
                        }
                    }
                }
                s.push('"');
                s
            }
            Some(Node::Binary { .. }) => self.render_binary(id),
            Some(Node::Unary { op, expr }) => {
                let symbol = match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                };
                format!("{symbol}{}", self.render_child(*expr, 7, false))
            }
            Some(Node::Array { elements }) => {
                let parts: Vec<String> = elements.iter().map(|e| self.render_expr(*e)).collect();
                format!("[{}]", parts.join(", "))
            }
            Some(Node::Table { fields }) => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| format!("{k} = {}", self.render_expr(*v)))
                    .collect();
                format!("{{ {} }}", parts.join(", "))
            }
            Some(Node::Quantity { value, unit }) => {
                format!("{}{}", self.render_expr(*value), unit.suffix())
            }
            _ => "<?>".into(),
        }
    }

    /// Render a binary expression by walking its left spine iteratively, so
    /// a long left-associative chain (`1 + 1 + …`) cannot exhaust the stack.
    /// Right-nested trees only arise through parentheses, which the parser
    /// bounds at MAX_EXPR_DEPTH.
    fn render_binary(&self, id: NodeId) -> String {
        let top_prec = match self.module.get(id) {
            Some(Node::Binary { op, .. }) => prec(*op),
            _ => 0,
        };
        let mut rights: Vec<String> = Vec::new();
        let mut ops: Vec<BinOp> = Vec::new();
        let mut cur = id;
        loop {
            match self.module.get(cur) {
                Some(Node::Binary { op, lhs, rhs }) => {
                    ops.push(*op);
                    rights.push(self.render_child(*rhs, prec(*op), true));
                    // Only continue along the spine while the left child is
                    // the same precedence (a left-associative chain); a
                    // tighter or looser left child is an operand that may
                    // need parentheses.
                    let next = *lhs;
                    match self.module.get(next) {
                        Some(Node::Binary { op: next_op, .. }) if prec(*next_op) == top_prec => {
                            cur = next;
                        }
                        _ => {
                            let leftmost = self.render_child(next, top_prec, false);
                            rights.push(leftmost);
                            break;
                        }
                    }
                }
                _ => {
                    let leftmost = self.render_child(cur, top_prec, false);
                    rights.push(leftmost);
                    break;
                }
            }
        }
        // rights/ops are innermost-first; emit outermost-first.
        rights.reverse();
        ops.reverse();
        let mut out = rights[0].clone();
        for (i, op) in ops.iter().enumerate() {
            out.push_str(&format!(" {} {}", op_symbol(*op), rights[i + 1]));
        }
        out
    }

    /// Render a binary operand, parenthesizing when precedence requires it.
    /// An `env` node with a default consumes an expression, so it binds
    /// looser than any operator and is always parenthesized as an operand.
    fn render_child(&self, id: NodeId, parent_prec: u8, is_right: bool) -> String {
        let text = self.render_expr(id);
        match self.module.get(id) {
            Some(Node::Binary { op, .. }) => {
                let child = prec(*op);
                if child < parent_prec || (child == parent_prec && is_right) {
                    return format!("({text})");
                }
            }
            Some(Node::Env {
                default: Some(_), ..
            }) => {
                return format!("({text})");
            }
            _ => {}
        }
        text
    }
}

fn prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::Eq | BinOp::Ne => 3,
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 4,
        BinOp::Add | BinOp::Sub => 5,
        BinOp::Mul | BinOp::Div => 6,
    }
}

fn op_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn render_literal(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => {
            // Display never uses scientific notation (which the lexer does
            // not accept) and round-trips exactly; ensure a decimal point so
            // the value stays a float.
            let mut s = format!("{f}");
            if !s.contains('.') {
                s.push_str(".0");
            }
            s
        }
        Value::String(s) => format!("\"{}\"", escape(s)),
        Value::DurationMs(ms) => format!("{ms}ms"),
        Value::Quantity(n, unit) => format!("{n}{}", unit.suffix()),
        Value::Array(a) => {
            let parts: Vec<String> = a.iter().map(render_literal).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Table(t) => {
            let parts: Vec<String> = t
                .iter()
                .map(|(k, v)| format!("{k} = {}", render_literal(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        // Never emit secret material; the marker is the canonical form.
        Value::Secret(_) => "\"***SECRET***\"".into(),
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}
