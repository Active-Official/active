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

/// Map every named node back to its name (let bindings and table keys,
/// including profile trees so profile-internal references render).
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
    for id in module.profiles.values() {
        walk(module, *id, &mut names);
    }
    names
}

impl<'a> Formatter<'a> {
    // Indexes key by path only (no file): the formatter parses
    // single-file documents (resolve_includes=false), so paths are
    // unique by construction. Multi-file merging would need
    // file-qualified keys — see the parser's (line, file) matching.
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

        // The root schema block, when the document declares one
        // (spec §2.10). Schemas are metadata: position is canonical.
        if let Some(schema) = self.module.schema {
            self.emit_leading("schema");
            self.emit_schema_table(schema, 0);
        }

        // Profile blocks, in source order (spec §2.8). They merge at
        // evaluation and never export, but formatting preserves them.
        let profiles: Vec<(String, NodeId)> = self
            .module
            .profiles
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        for (name, id) in &profiles {
            self.emit_leading(name);
            self.emit_profile(*id, name, 0);
        }

        // Patch directives, in source order (spec §2.9). They apply
        // post-evaluation and never export, but formatting preserves
        // them so the composition structure survives a fmt pass.
        // Root includes and patches emit together sorted by source line:
        // patch application order is positional, so hoisting includes
        // above textually-earlier patches would change values on
        // reparse. Includes in nested scopes emit with their scope.
        let mut root_items: Vec<(u32, bool, NodeId, Option<IncludeStmt>)> = Vec::new();
        for id in &self.module.patches {
            let line = self
                .module
                .node_spans
                .get(id)
                .map(|s| s.line)
                .unwrap_or(u32::MAX);
            root_items.push((line, false, *id, None));
        }
        for inc in self
            .module
            .includes
            .iter()
            .filter(|inc| inc.scope_path.is_empty())
        {
            root_items.push((inc.line, true, NodeId(u32::MAX), Some(inc.clone())));
        }
        root_items.sort_by_key(|(line, is_include, id, _)| (*line, *is_include, id.0));
        for (_, is_include, id, inc) in &root_items {
            if *is_include {
                let inc = inc.as_ref().expect("include entry");
                self.emit_include(inc, 0);
            } else {
                self.emit_patch(*id);
            }
        }

        if let Some(root) = self.module.root {
            let has_entries = !self.out.is_empty()
                && matches!(self.module.get(root), Some(Node::Table { fields }) if !fields.is_empty());
            if has_entries && !self.out.ends_with("\n\n") {
                self.out.push('\n');
            }
            // Root dependency metadata leads the body (spec/linter.md
            // §2), like the root schema block above.
            self.emit_depends_on(root, 0);
            self.emit_table_body(root, 0, "", true);
        }
        for text in self.trailing_document.clone() {
            self.out.push_str(&text);
            self.out.push('\n');
        }
    }

    /// Emit a table's entries. `path` is the dotted path of the table
    /// (empty for the document root); `is_root` marks the document level.
    /// `schema` is this table's schema node for parallel emission:
    /// a `schema = { … }` block leads the body (spec §2.10).
    fn emit_table_body(&mut self, id: NodeId, depth: usize, path: &str, is_root: bool) {
        let fields = match self.module.get(id) {
            Some(Node::Table { fields }) => fields.clone(),
            _ => return,
        };
        // Include statements recorded for this scope come first, in
        // source order: under the defaults rule (spec §2.7) their
        // position relative to keys never changes semantics. Root
        // includes emit with root patches (positional application
        // order), so only nested scopes emit here.
        if !path.is_empty() {
            let mut includes: Vec<&IncludeStmt> = self
                .module
                .includes
                .iter()
                .filter(|inc| inc.scope_path.join(".") == path)
                .collect();
            includes.sort_by_key(|inc| inc.line);
            for inc in includes {
                self.emit_include(inc, depth);
            }
            // A nested `schema` block leads its table's body (spec
            // §2.10). It was detached from value fields at parse, so
            // re-emit it here from the schema tree. The root schema
            // emits once from emit_module instead.
            if let Some(schema) = self.schema_for_path(path) {
                self.emit_schema_table(schema, depth);
            }
            // Dependency metadata follows the schema (spec/linter.md
            // §2), ahead of the keys it constrains.
            self.emit_depends_on(id, depth);
        }
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

    /// Emit a `depends_on` metadata line for a table scope
    /// (spec/linter.md §2). Detached at parse like schemas, so it
    /// re-emits here from the side table; a no-op for scopes without
    /// declarations.
    fn emit_depends_on(&mut self, id: NodeId, depth: usize) {
        let Some(dep) = self.module.depends_on.get(&id).cloned() else {
            return;
        };
        let quoted: Vec<std::string::String> = dep
            .paths
            .iter()
            .map(|p| format!("\"{}\"", escape(p)))
            .collect();
        self.out.push_str(&format!(
            "{}depends_on = [{}]",
            INDENT.repeat(depth),
            quoted.join(", ")
        ));
        self.out.push('\n');
    }

    /// Look up this table's schema node by dotted path, descending from
    /// the root schema through SchemaTable fields (spec §2.10). Returns
    /// None when no schema covers this table.
    fn schema_for_path(&self, path: &str) -> Option<NodeId> {
        let mut id = self.module.schema?;
        if path.is_empty() {
            return Some(id);
        }
        for segment in path.split('.') {
            match self.module.get(id) {
                Some(Node::SchemaTable { fields, .. }) => {
                    id = *fields.get(segment)?;
                }
                _ => return None,
            }
        }
        Some(id)
    }

    /// Emit a `schema` block at the given depth (spec §2.10). Indentation
    /// style uses `schema:` with an indented body; brace style uses
    /// `schema { … }`. A closed table leads with `closed = true`.
    /// Entries keep source order: the schema tree preserves it. An empty
    /// schema emits inline (`schema = {}`) in every style: indentation
    /// style cannot express a bodyless block.
    fn emit_schema_table(&mut self, id: NodeId, depth: usize) {
        let (fields, closed) = match self.module.get(id) {
            Some(Node::SchemaTable { fields, closed }) => (fields.clone(), *closed),
            _ => return,
        };
        if fields.is_empty() && !closed {
            let indent = INDENT.repeat(depth);
            self.out.push_str(&format!("{indent}schema = {{}}"));
            self.emit_trailing("schema");
            self.out.push('\n');
            return;
        }
        let indent = INDENT.repeat(depth);
        match self.style {
            Style::Indent => {
                self.out.push_str(&format!("{indent}schema:"));
                self.emit_trailing("schema");
                self.out.push('\n');
                let inner = INDENT.repeat(depth + 1);
                if closed {
                    self.out.push_str(&format!("{inner}closed = true\n"));
                }
                for (name, entry) in &fields {
                    let typ = self.render_schema_type(*entry);
                    self.out.push_str(&format!("{inner}{name} = {typ}\n"));
                }
            }
            _ => {
                self.out.push_str(&format!("{indent}schema {{"));
                self.emit_trailing("schema");
                self.out.push('\n');
                let inner = INDENT.repeat(depth + 1);
                if closed {
                    self.out.push_str(&format!("{inner}closed = true\n"));
                }
                for (name, entry) in &fields {
                    let typ = self.render_schema_type(*entry);
                    self.out.push_str(&format!("{inner}{name} = {typ}\n"));
                }
                self.out.push_str(&format!("{indent}}}\n"));
            }
        }
    }

    /// Render one schema type expression: a field (`Int(min = …)`),
    /// a nested table (`Table { … }`), or an element schema
    /// (`Array<…>`), recursing as needed.
    fn render_schema_type(&self, id: NodeId) -> String {
        match self.module.get(id).cloned() {
            Some(Node::SchemaField { field }) => self.render_schema_field(&field),
            Some(Node::SchemaArray { element }) => {
                format!("Array<{}>", self.render_schema_type(element))
            }
            Some(Node::SchemaTable { fields, closed }) => {
                let mut parts: Vec<std::string::String> = Vec::new();
                if closed {
                    parts.push("closed = true".to_string());
                }
                for (name, entry) in fields {
                    parts.push(format!("{name} = {}", self.render_schema_type(entry)));
                }
                if parts.is_empty() {
                    "Table {}".to_string()
                } else {
                    format!("Table {{ {} }}", parts.join(", "))
                }
            }
            _ => "Any".to_string(),
        }
    }

    /// Render a leaf field type with its present constraint arguments.
    /// Absent arguments are omitted (never `None` placeholders).
    fn render_schema_field(&self, field: &crate::ir::SchemaField) -> String {
        use crate::types::ActiveType::*;
        // `deprecated` renders inside the parens for every leaf type
        // (spec/linter.md §3), including Enum's positional list.
        let deprecated = match &field.deprecated {
            None => std::string::String::new(),
            Some(reason) if reason.is_empty() => ", deprecated = true".to_string(),
            Some(reason) => format!(", deprecated = \"{}\"", escape(reason)),
        };
        if let Enum(values) = &field.typ {
            let quoted: Vec<std::string::String> = values
                .iter()
                .map(|v| format!("\"{}\"", escape(v)))
                .collect();
            let mut out = format!("Enum({}", quoted.join(", "));
            if let Some(descriptions) = field.enum_descriptions {
                out.push_str(&format!(
                    ", descriptions = {}",
                    self.render_expr(descriptions)
                ));
            }
            out.push_str(&deprecated);
            out.push(')');
            return out;
        }
        let mut args: Vec<std::string::String> = Vec::new();
        for (name, slot) in [
            ("min", field.min),
            ("max", field.max),
            ("min_len", field.min_len),
            ("max_len", field.max_len),
            ("pattern", field.pattern),
            ("format", field.format),
            ("media_type", field.blob_media_type),
            ("max_size", field.blob_max_size),
            ("description", field.description),
            ("requires", field.requires),
            ("default", field.default),
        ] {
            if let Some(arg) = slot {
                args.push(format!("{name} = {}", self.render_expr(arg)));
            }
        }
        match &field.deprecated {
            None => {}
            Some(reason) if reason.is_empty() => args.push("deprecated = true".to_string()),
            Some(reason) => args.push(format!("deprecated = \"{}\"", escape(reason))),
        }
        let base = match &field.typ {
            Table { closed: true } => "Table(closed)".to_string(),
            other => other.name(),
        };
        if args.is_empty() {
            base
        } else {
            format!("{base}({})", args.join(", "))
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

    /// Emit a `profile "name"` block (spec §2.8). Brace style uses
    /// `profile "name" { … }`; indentation style uses `profile "name":`
    /// with an indented body. The body's comment paths are the profile
    /// name (parse_profile pushes it), so trivia survives. Names escape
    /// like interpolation-safe literals so quotes, dollars, and newlines
    /// round-trip.
    fn emit_profile(&mut self, id: NodeId, name: &str, depth: usize) {
        let qname = escape_interp_literal(name);
        let empty = matches!(
            self.module.get(id),
            Some(Node::Table { fields }) if fields.is_empty()
        );
        match self.style {
            Style::Indent if !empty => {
                self.out
                    .push_str(&format!("{}profile \"{qname}\":", INDENT.repeat(depth)));
                self.emit_trailing(name);
                self.out.push('\n');
                self.emit_table_body(id, depth + 1, name, false);
            }
            _ => {
                if empty {
                    self.out
                        .push_str(&format!("{}profile \"{qname}\" {{}}", INDENT.repeat(depth)));
                    self.emit_trailing(name);
                    self.out.push('\n');
                } else {
                    self.out
                        .push_str(&format!("{}profile \"{qname}\" {{", INDENT.repeat(depth)));
                    self.emit_trailing(name);
                    self.out.push('\n');
                    self.emit_table_body(id, depth + 1, name, false);
                    self.out.push_str(&format!("{}}}\n", INDENT.repeat(depth)));
                }
            }
        }
    }

    /// Emit one `include` statement (spec §2.7) at the given depth.
    fn emit_include(&mut self, inc: &IncludeStmt, depth: usize) {
        self.out.push_str(&INDENT.repeat(depth));
        self.out.push_str(&format!("include {:?}", inc.path));
        if let Some(alias) = &inc.alias {
            self.out.push_str(&format!(" as {alias}"));
        }
        if inc.private {
            self.out.push_str(" private");
        }
        self.out.push('\n');
    }

    /// Emit one patch directive (`@patch` / `@remove` / `@rename`,
    /// spec §2.9). Paths render as quoted strings; patch values render
    /// as expressions. Statement comments attach under the `@op(path)`
    /// key recorded at parse time.
    fn emit_patch(&mut self, id: NodeId) {
        fn q(s: &str) -> String {
            format!("\"{}\"", escape(s))
        }
        let (text, key) = match self.module.get(id) {
            Some(Node::PatchDirective { op }) => match op {
                PatchOp::Set { path, value } => (
                    format!("@patch({}, {})", q(path), self.render_expr(*value)),
                    format!("@patch({path})"),
                ),
                PatchOp::Remove { path } => {
                    (format!("@remove({})", q(path)), format!("@remove({path})"))
                }
                PatchOp::Rename { from, to } => (
                    format!("@rename({}, {})", q(from), q(to)),
                    format!("@rename({from})"),
                ),
            },
            _ => return,
        };
        self.emit_leading(&key);
        self.out.push_str(&text);
        self.emit_trailing(&key);
        self.out.push('\n');
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
            Some(Node::ContentAddressed { media_type, data }) => {
                format!(
                    "blob({}, {})",
                    self.render_expr(*media_type),
                    self.render_expr(*data)
                )
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
                        // A literal `${` can only have come from an escaped
                        // dollar; re-escape it so it stays literal.
                        InterpPart::Literal(t) => s.push_str(&escape_interp_literal(t)),
                        InterpPart::Expr(e) => {
                            s.push_str("${");
                            let inner = self.render_expr(*e);
                            s.push_str(&escape_embedded(&inner));
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
            Some(Node::Semantic { tag, inner }) => {
                format!("@{} {}", tag.as_str(), self.render_ml(*inner))
            }
            _ => "<?>".into(),
        }
    }

    /// Render a multi-line string payload (spec §1.4.1): `"""` form for
    /// interpolated strings, `'''` verbatim form for plain literals that
    /// avoid the delimiter, with escaping fallback when both delimiters
    /// appear in the content.
    fn render_ml(&self, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::Interpolate { parts }) => {
                let mut s = String::from("\"\"\"");
                for part in parts {
                    match part {
                        InterpPart::Literal(t) => s.push_str(&escape_ml_literal(t)),
                        InterpPart::Expr(e) => {
                            s.push_str("${");
                            let inner = self.render_expr(*e);
                            s.push_str(&escape_embedded(&inner));
                            s.push('}');
                        }
                    }
                }
                s.push_str("\"\"\"");
                s
            }
            Some(Node::Literal(Value::String(text))) if !text.contains("'''") => {
                format!("'''{text}'''")
            }
            Some(Node::Literal(Value::String(text))) => {
                format!("\"\"\"{}\"\"\"", escape_ml_literal(text))
            }
            _ => self.render_expr(id),
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
        Value::String(s) => {
            // A string containing `${` must not interpolate when re-read, so
            // emit it single-quoted (single-quoted strings are literal).
            if s.contains("${") {
                format!("'{}'", escape_single(s))
            } else {
                format!("\"{}\"", escape(s))
            }
        }
        Value::DurationMs(ms) => format!("{ms}ms"),
        Value::Quantity(n, unit) => unit.format_amount(*n),
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
        // Source fidelity: the formatter reprints what the user wrote.
        // Blob payloads are expressions in source; re-render the call
        // shape with the same literal escaping as strings.
        Value::Blob { .. } => "\"$active.blob\"".into(),
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

/// Escape for a single-quoted string. Only `'` and `\` are special there:
/// the single-quote lexer does not decode `\n`/`\t`/`\r`, so those are
/// emitted literally (a single-quoted string may contain them).
fn escape_single(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out
}

/// Escape the literal text of an interpolation part so a `${` that was
/// escaped in the source stays literal.
fn escape_interp_literal(s: &str) -> String {
    escape(s).replace("${", "\\${")
}

/// Escape a literal part for the `"""` multi-line form. This is exactly
/// `escape_interp_literal`: `escape` already covers `"`, `\`, and newlines,
/// and the `${` rule keeps literal dollars from becoming interpolation.
/// No further substitution — an extra pass would double-escape (e.g. `\"`
/// becoming `\\"` and corrupting the content on reparse).
fn escape_ml_literal(s: &str) -> String {
    escape_interp_literal(s)
}

/// Escape a rendered sub-expression for embedding inside a double-quoted
/// interpolation: the outer lexer must see the original expression text.
fn escape_embedded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out
}
