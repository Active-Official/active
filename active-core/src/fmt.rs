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

/// The parser rejects block nesting deeper than this (`E011`,
/// `spec/budgets.md`). The formatter must never emit syntax the parser
/// rejects, so a deeper tree is rendered with dotted headers instead of
/// nesting: only lexical bodies consume the block budget, header paths do
/// not.
const PARSER_MAX_BLOCK_DEPTH: usize = 64;

/// Start header rendering this far below the parser limit. Leaves room for
/// the document body itself and for re-parsing/re-formatting the output.
const FLAT_DEPTH_THRESHOLD: usize = PARSER_MAX_BLOCK_DEPTH - 4;

/// Format a module in the requested style. `Preserve` uses the style the
/// source predominantly used.
pub fn format(module: &Module, style: Style) -> String {
    let style = match style {
        Style::Preserve => module.source_style,
        other => other,
    };
    // Stabilize the output (spec/formatter.md section 2). Comment
    // attachment is scored by source position, so a comment whose canonical
    // position differs from where it was written can attach elsewhere after
    // one rewrite. The contract agents rely on is `format(parse(out)) ==
    // out`, so re-format the result until it stops changing (at most
    // `STABILIZE_PASSES`), and return that fixed point. Formatting is cheap
    // next to the parse it already did; the loop runs once for documents
    // that are already canonical, which is most of them.
    let mut current = format_once(module, style);
    for _ in 0..STABILIZE_PASSES {
        let Ok(reparsed) = crate::parser::parse(&current, "fmt-stabilize") else {
            // The output must always re-parse; if it somehow does not, the
            // caller's input is still better served by a deterministic
            // answer than by a rewrite loop.
            return current;
        };
        let next = format_once(&reparsed, style);
        if next == current {
            return current;
        }
        current = next;
    }
    current
}

/// Passes allowed while searching for the formatter's fixed point. Two is
/// enough for the attachment cases measured (an already-canonical document
/// needs one); the cap keeps a hypothetical oscillation bounded.
const STABILIZE_PASSES: usize = 3;

fn format_once(module: &Module, style: Style) -> String {
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
        pending_blank: false,
        flat: false,
        emitted_scopes: std::collections::HashSet::new(),
        out: String::new(),
    };
    f.flat = f.needs_flat_rendering();
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
    /// A blank separator was already emitted by an empty-table trailing
    /// comment block; the next root entry must not add a second one.
    pending_blank: bool,
    /// The tree is deeper than [`FLAT_DEPTH_THRESHOLD`]: every data table
    /// is rendered as a `[dotted.path]` header instead of a nested body
    /// (see the constants above). Chosen once per pass from the tree.
    flat: bool,
    /// Scope identities whose includes/schema/depends_on blocks were
    /// already emitted. Keyed by (profile, path): profiles and value
    /// tables are separate namespaces with the same text.
    emitted_scopes: std::collections::HashSet<(bool, String)>,
    out: String,
}

/// Map every named node back to its name (let bindings and table keys,
/// including profile trees so profile-internal references render).
fn build_names(module: &Module) -> HashMap<NodeId, String> {
    /// Schema trees are detached from value fields, so a reference that
    /// resolved to a schema node had no name and rendered as `<?>` -
    /// unparseable output (local random search, seed 33). Name them by
    /// their declaring key, like value tables.
    fn walk_schema(module: &Module, id: NodeId, names: &mut HashMap<NodeId, String>) {
        let children: Vec<(String, NodeId)> = match module.get(id) {
            Some(Node::SchemaTable { fields, .. }) => {
                fields.iter().map(|(k, v)| (k.clone(), *v)).collect()
            }
            _ => return,
        };
        for (key, child) in children {
            names.entry(child).or_insert(key);
            walk_schema(module, child, names);
        }
    }
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
    if let Some(schema) = module.schema {
        names.entry(schema).or_insert_with(|| "schema".to_string());
        walk_schema(module, schema, &mut names);
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
            // Canonical trivia: a lone CR is a line-ending artifact, and
            // emitting one before the formatter's own "\n" makes the
            // next pass treat it as CRLF and drop it, breaking
            // idempotence. Comments are non-semantic, so drop CRs.
            let text = c.text.replace('\r', "");
            match &c.attachment {
                Attachment::Leading(key) => self
                    .leading
                    .entry(key.clone())
                    .or_default()
                    .push(text.clone()),
                Attachment::Trailing(key) => {
                    // Duplicate keys share one surviving statement (the
                    // LAST one), so every duplicate's trailing comment
                    // lands in the same bucket. Keep every comment; the
                    // split between trailing and leading happens after the
                    // pass, once it is known which comment belongs to the
                    // survivor (LANG-D-002).
                    self.trailing
                        .entry(key.clone())
                        .or_default()
                        .push(text.clone());
                }
                Attachment::TrailingBlock(path) => self
                    .trailing_block
                    .entry(path.clone())
                    .or_default()
                    .push(text.clone()),
                Attachment::TrailingDocument => self.trailing_document.push(text),
            }
        }

        // Duplicate keys: the surviving statement is the LAST duplicate, so
        // only its own trailing comment stays on the line; earlier
        // duplicates' comments become leading lines above it, in source
        // order. Concatenating them (`x = 2 # first # second`) would lose
        // the boundary between two independent edits (LANG-D-002).
        let mut splits: Vec<(String, Vec<String>)> = Vec::new();
        for (key, bucket) in self.trailing.iter_mut() {
            if bucket.len() > 1 {
                let survivor = bucket.pop().expect("len > 1");
                let dropped = std::mem::take(bucket);
                bucket.push(survivor);
                splits.push((key.clone(), dropped));
            }
        }
        for (key, dropped) in splits {
            let lead = self.leading.entry(key).or_default();
            let mut merged = dropped;
            merged.append(lead);
            *lead = merged;
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
            self.emit_leading(name, 0);
            let value = self.render_expr(id);
            self.out.push_str(&format!("let {name} = {value}"));
            self.emit_trailing(name);
            self.out.push('\n');
        }

        // The root schema block, when the document declares one
        // (spec §2.10). Schemas are metadata: position is canonical.
        if let Some(schema) = self.module.schema {
            self.emit_leading("schema", 0);
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
            self.emit_leading(name, 0);
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
        self.emit_table_body_with(id, depth, path, is_root, false);
    }

    /// Emit a table body. `profile` selects the namespace: a profile
    /// body and a value table can share the same path text.
    fn emit_table_body_with(
        &mut self,
        id: NodeId,
        depth: usize,
        path: &str,
        is_root: bool,
        profile: bool,
    ) {
        // Only a REF to a named table is rendered as its name by the
        // caller; a table definition emits its own body.
        if matches!(self.module.get(id), Some(Node::Ref(_)))
            && self.names.contains_key(&self.resolve_ref(id))
        {
            return;
        }
        let resolved = self.resolve_ref(id);
        let fields = match self.module.body_fields.get(&resolved) {
            Some(snapshot) => snapshot.clone(),
            None => match self.module.get(resolved) {
                Some(Node::Table { fields }) => fields.clone(),
                _ => return,
            },
        };
        // Defense in depth: a schema node that survived extraction must
        // never render as data (`schema = null`).
        let fields: indexmap::IndexMap<String, NodeId> = fields
            .into_iter()
            .filter(|(key, node)| {
                !(key == "schema"
                    && matches!(
                        self.module.get(self.resolve_ref(*node)),
                        Some(Node::SchemaTable { .. })
                    ))
                    && !self.is_include_node(*node)
            })
            .collect();
        // Include statements recorded for this scope come first, in
        // source order: under the defaults rule (spec §2.7) their
        // position relative to keys never changes semantics. Root
        // includes emit with root patches (positional application
        // order), so only nested scopes emit here.
        if !path.is_empty() && self.emitted_scopes.insert((profile, path.to_string())) {
            let mut includes: Vec<&IncludeStmt> = self
                .module
                .includes
                .iter()
                .filter(|inc| inc.profile == profile && inc.scope_path.join(".") == path)
                .collect();
            includes.sort_by_key(|inc| inc.line);
            for inc in includes {
                self.emit_include(inc, depth);
            }
            if !profile {
                if let Some(schema) = self.schema_for_path(path) {
                    self.emit_schema_table(schema, depth);
                }
            }
            // Dependency metadata follows the schema (spec/linter.md
            // §2), ahead of the keys it constrains. The scope node is
            // the surviving first table at `path` (or the profile body).
            let scope_id = if profile {
                self.module.profiles.get(path).copied().unwrap_or(resolved)
            } else {
                self.table_id_at_path(path).unwrap_or(resolved)
            };
            self.emit_depends_on(scope_id, depth);
        }
        // A reference to a table is a table here: rendering it as a
        // scalar expression (`key = { ... }`) made the next pass
        // expand it into a block instead, so formatting was not a
        // fixed point (fuzz crash, formatter target 2026-11-15).
        // A DATA field named `schema` is never a table, whatever it resolves
        // to: every table-valued form of that key is schema syntax
        // (spec/grammar.md section 2.10), so emitting a block or `{}` would
        // turn data into metadata on the next parse. A field named `schema`
        // can only hold an alias here (a literal table value would already
        // have parsed as a schema block), and the alias name re-parses as
        // data. Fuzz seed crash-a0f12d89.
        let is_table_node = |f: &Formatter, n: &NodeId, key: &str| f.is_block_table(n, key);
        // At the document root, scalars must be hoisted above tables: a
        // `key = value` line after a table header would otherwise be
        // absorbed by that table (spec §5.1). Inside a block, the original
        // order is preserved.
        let ordered: Vec<(String, NodeId)> = if is_root {
            let mut scalars: Vec<(String, NodeId)> = Vec::new();
            let mut tables: Vec<(String, NodeId)> = Vec::new();
            for (k, v) in &fields {
                if is_table_node(self, v, k) {
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
            .filter(|(k, v)| is_table_node(self, v, k))
            .cloned()
            .collect();

        // A table's own trailing comments are emitted after its last
        // entry. When that entry is a nested non-empty block, the parser
        // attaches those comments to the nested block on the next read
        // (they follow its `}`), so the canonical form places them inside
        // it. Hoist them down now: this is the fixed point the second
        // pass reaches.
        if let Some((last_key, last_id)) = ordered.last() {
            // Resolve aliases first: the last entry can be a `Ref` to a
            // table definition, and the hoist must see the table it names.
            // Reading the raw node skipped the hoist for aliased tables,
            // so the comment stayed after the closing brace and the next
            // parse attached it inside the block (fuzz seed
            // crash-8e989b62).
            if matches!(
                self.module.get(self.resolve_ref(*last_id)),
                Some(Node::Table { fields }) if !fields.is_empty()
            ) && self.trailing_block.contains_key(path)
            {
                let child_path = if path.is_empty() {
                    last_key.clone()
                } else {
                    format!("{path}.{last_key}")
                };
                if let Some(comments) = self.trailing_block.remove(path) {
                    self.trailing_block
                        .entry(child_path)
                        .or_default()
                        .extend(comments);
                }
            }
        }

        // An empty table that is rendered INLINE (`key = {}`) has no block
        // on the next read, so a comment written after it would attach to
        // the enclosing block instead of the table. Hoist such comments to
        // the enclosing block now - that is the fixed point the second pass
        // reaches (fuzz seed crash-962a37a9: indentation style, an empty
        // table followed by a comment). Root tables in brace style keep
        // their `[header]`, which does own a block, so they are excluded.
        let root_header_style = is_root && depth == 0 && self.style == Style::Brace;
        if !root_header_style {
            for (key, node) in &ordered {
                let empty = matches!(
                    self.module.get(self.resolve_ref(*node)),
                    Some(Node::Table { fields }) if fields.is_empty()
                );
                if !empty {
                    continue;
                }
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if let Some(comments) = self.trailing_block.remove(&child_path) {
                    self.trailing_block
                        .entry(path.to_string())
                        .or_default()
                        .extend(comments);
                }
            }
        }

        // Document-level trailing comments: a comment at the very end of
        // the document still attaches to the LAST block (Rule 3 fires
        // before Rule 4), so emit it inside that block. Hoisting to the
        // last block-rendered entry is the fixed point the next pass
        // reaches (fuzz seed crash-962a37a9).
        if path.is_empty() && self.trailing_block.contains_key("") {
            let last_block: Option<&(String, NodeId)> = ordered.iter().rev().find(|(_, node)| {
                let id = self.resolve_ref(*node);
                match self.module.get(id) {
                    Some(Node::Table { fields }) => {
                        self.style == Style::Brace || !fields.is_empty()
                    }
                    _ => false,
                }
            });
            if let Some((last_key, _)) = last_block {
                let child_path = last_key.clone();
                if let Some(comments) = self.trailing_block.remove("") {
                    self.trailing_block
                        .entry(child_path)
                        .or_default()
                        .extend(comments);
                }
            }
        }

        // Scalars-first at the root, source order inside blocks.
        let mut first = true;
        let entry_count = ordered.len();
        for (index, (key, node)) in ordered.iter().enumerate() {
            let child_path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            let is_table = tables.iter().any(|(k, _)| k == key);
            if is_root && !first && !self.pending_blank {
                self.out.push('\n');
            }
            self.pending_blank = false;
            first = false;
            let has_next = index + 1 < entry_count;

            let qkey = child_path.clone();
            self.emit_leading(&qkey, depth);
            if is_table {
                if self.flat && is_root {
                    // A tree deeper than the parser's block budget: emit
                    // dotted headers instead of nesting (see
                    // FLAT_DEPTH_THRESHOLD).
                    self.emit_flat_table(*node, &child_path, key, depth);
                } else {
                    self.emit_table_block(
                        *node,
                        &child_path,
                        key,
                        depth,
                        is_root,
                        has_next,
                        profile,
                    );
                }
            } else {
                let value = self.render_expr(*node);
                let indent = INDENT.repeat(depth);
                self.out.push_str(&format!("{indent}{key} = {value}"));
                self.emit_trailing(&qkey);
                self.out.push('\n');
            }
        }

        if let Some(comments) = self.trailing_block.remove(path) {
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
        let (fields, closed, meta) = match self.module.get(id) {
            Some(Node::SchemaTable {
                fields,
                closed,
                meta,
                ..
            }) => (fields.clone(), *closed, meta.clone()),
            _ => return,
        };
        let check_lines: Vec<String> = if self.module.schema == Some(id) {
            self.module
                .schema_checks
                .get(&id)
                .map(|rules| {
                    rules
                        .iter()
                        .map(|rule| format!("{} = {}", rule.name, rule.source))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if fields.is_empty()
            && !closed
            && meta.version.is_none()
            && meta.min_reader.is_none()
            && meta.min_writer.is_none()
            && check_lines.is_empty()
        {
            let indent = INDENT.repeat(depth);
            self.out.push_str(&format!("{indent}schema = {{}}"));
            self.emit_trailing("schema");
            self.out.push('\n');
            return;
        }
        let mut meta_lines: Vec<std::string::String> = Vec::new();
        for (name, slot) in [
            ("version", &meta.version),
            ("min_reader", &meta.min_reader),
            ("min_writer", &meta.min_writer),
        ] {
            if let Some(v) = slot {
                meta_lines.push(format!("{name} = \"{}\"", escape(v)));
            }
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
                for line in &meta_lines {
                    self.out.push_str(&format!("{inner}{line}\n"));
                }
                for (name, entry) in &fields {
                    let typ = self.render_schema_type(*entry);
                    self.out.push_str(&format!("{inner}{name} = {typ}\n"));
                }
                if !check_lines.is_empty() {
                    // L-P19: predicates are emitted from authored source,
                    // never through the value-expression renderer.
                    self.out.push_str(&format!("{inner}check:\n"));
                    let deeper = INDENT.repeat(depth + 2);
                    for line in &check_lines {
                        self.out.push_str(&format!("{deeper}{line}\n"));
                    }
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
                for line in &meta_lines {
                    self.out.push_str(&format!("{inner}{line}\n"));
                }
                for (name, entry) in &fields {
                    let typ = self.render_schema_type(*entry);
                    self.out.push_str(&format!("{inner}{name} = {typ}\n"));
                }
                if !check_lines.is_empty() {
                    self.out.push_str(&format!("{inner}check {{\n"));
                    let deeper = INDENT.repeat(depth + 2);
                    for line in &check_lines {
                        self.out.push_str(&format!("{deeper}{line}\n"));
                    }
                    self.out.push_str(&format!("{inner}}}\n"));
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
            Some(Node::SchemaTable {
                fields,
                closed,
                meta,
                ..
            }) => {
                let mut parts: Vec<std::string::String> = Vec::new();
                if closed {
                    parts.push("closed = true".to_string());
                }
                // Evolution metadata round-trips first (insertion
                // order is not significant; the parser accepts any
                // position for these reserved keys).
                for (name, slot) in [
                    ("version", &meta.version),
                    ("min_reader", &meta.min_reader),
                    ("min_writer", &meta.min_writer),
                ] {
                    if let Some(v) = slot {
                        parts.push(format!("{name} = \"{}\"", escape(v)));
                    }
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
            // Enum fields accept the same optional arguments as other
            // leaves. Dropping them changed `required`, defaults, and
            // descriptions on the next parse (a formatter must never
            // change meaning).
            for (name, slot) in [
                ("description", field.description),
                ("requires", field.requires),
                ("default", field.default),
            ] {
                if let Some(arg) = slot {
                    out.push_str(&format!(", {name} = {}", self.render_expr(arg)));
                }
            }
            if let Some(removed) = &field.removed_in {
                out.push_str(&format!(", removed_in = \"{}\"", escape(removed)));
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
        // `removed_in` is static text (a version string), not a node.
        if let Some(removed) = &field.removed_in {
            args.push(format!("removed_in = \"{}\"", escape(removed)));
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

    /// True when this field renders as a table *block*. A reference to a
    /// named table stays a `key = other` reference (the next parse resolves
    /// the same name); only an unnamed target is expanded. A DATA field
    /// named `schema` is never a table, whatever it resolves to.
    fn is_block_table(&self, node: &NodeId, key: &str) -> bool {
        if key == "schema" {
            return false;
        }
        match self.module.get(*node) {
            Some(Node::Table { .. }) => true,
            Some(Node::Ref(_)) => {
                let resolved = self.resolve_ref(*node);
                !self.names.contains_key(&resolved)
                    && matches!(self.module.get(resolved), Some(Node::Table { .. }))
            }
            _ => false,
        }
    }

    /// True when the rendered tree would nest deeper than the parser
    /// accepts, counting table blocks only (inline tables, arrays, and
    /// schema bodies do not consume the block budget).
    fn needs_flat_rendering(&self) -> bool {
        if let Some(root) = self.module.root {
            if self.exceeds_safe_nesting(root, FLAT_DEPTH_THRESHOLD) {
                return true;
            }
        }
        self.module
            .profiles
            .values()
            .any(|id| self.exceeds_safe_nesting(*id, FLAT_DEPTH_THRESHOLD))
    }

    fn exceeds_safe_nesting(&self, id: NodeId, remaining: usize) -> bool {
        if remaining == 0 {
            return true;
        }
        let Some(Node::Table { fields }) = self.module.get(self.resolve_ref(id)) else {
            return false;
        };
        fields.iter().any(|(key, child)| {
            self.is_block_table(child, key) && self.exceeds_safe_nesting(*child, remaining - 1)
        })
    }

    /// Emit one table of a deep tree as a `[dotted.path]` header: its
    /// non-table entries at `depth + 1`, then every child table as a
    /// sibling header at `depth`. Sibling headers close each body before
    /// the next one starts, so the parser merges the dotted path into the
    /// already-finished ancestors.
    fn emit_flat_table(&mut self, id: NodeId, full: &str, rel: &str, depth: usize) {
        let id = self.resolve_ref(id);
        let fields = match self.module.get(id) {
            Some(Node::Table { fields }) => fields.clone(),
            _ => return,
        };
        self.emit_leading(full, depth);
        self.out
            .push_str(&format!("{}[{rel}]", INDENT.repeat(depth)));
        self.emit_trailing(full);
        self.out.push('\n');
        // A header body continues at the header's own indentation: an
        // `Indent` token at statement position is E010, so scalars sit on
        // the header's column (the long-standing brace-root form is `[a]`
        // then `k = 0` at column 0). The next sibling header at this level
        // closes the body.
        self.emit_flat_body(id, &fields, full, depth);
        for (key, node) in &fields {
            if !self.is_block_table(node, key) {
                continue;
            }
            let child_full = if full.is_empty() {
                key.clone()
            } else {
                format!("{full}.{key}")
            };
            let child_rel = if rel.is_empty() {
                key.clone()
            } else {
                format!("{rel}.{key}")
            };
            self.emit_flat_table(*node, &child_full, &child_rel, depth);
        }
    }

    /// The body of a header (or of a profile block in flat mode): includes,
    /// schema, dependency metadata, every non-table entry, then this
    /// scope's trailing comments. Child tables are emitted by the caller as
    /// sibling headers, never inside this body.
    fn emit_flat_body(
        &mut self,
        id: NodeId,
        fields: &indexmap::IndexMap<String, NodeId>,
        full: &str,
        depth: usize,
    ) {
        let indent = INDENT.repeat(depth);
        if !full.is_empty() {
            let mut includes: Vec<IncludeStmt> = self
                .module
                .includes
                .iter()
                .filter(|inc| !inc.profile && inc.scope_path.join(".") == full)
                .cloned()
                .collect();
            includes.sort_by_key(|inc| inc.line);
            for inc in &includes {
                self.emit_include(inc, depth);
            }
            if let Some(schema) = self.schema_for_path(full) {
                self.emit_schema_table(schema, depth);
            }
        }
        self.emit_depends_on(id, depth);
        for (key, node) in fields {
            if self.is_block_table(node, key) {
                continue;
            }
            let child_full = if full.is_empty() {
                key.clone()
            } else {
                format!("{full}.{key}")
            };
            self.emit_leading(&child_full, depth);
            let value = self.render_expr(*node);
            self.out.push_str(&format!("{indent}{key} = {value}"));
            self.emit_trailing(&child_full);
            self.out.push('\n');
        }
        if let Some(comments) = self.trailing_block.get(full).cloned() {
            if !self.out.ends_with('\n') {
                self.out.push('\n');
            }
            for text in comments {
                self.out.push_str(&indent);
                self.out.push_str(&text);
                self.out.push('\n');
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_table_block(
        &mut self,
        id: NodeId,
        path: &str,
        key: &str,
        depth: usize,
        is_root: bool,
        has_next: bool,
        profile: bool,
    ) {
        // Resolve the alias first: a field can be a `Ref` to a table
        // definition, and the empty-vs-block decision must look at the
        // table it names. Reading the raw node made an aliased EMPTY table
        // render as a block (`ye {\n}`) that re-parsed as an empty table
        // and then rendered inline (`ye = {}`) on the second pass - fuzz
        // seed crash-f438d19.
        let id = self.resolve_ref(id);
        // A body whose only fields came from an include still owns the
        // include/schema/depends blocks; it must render as a block.
        let has_scope_meta = !path.is_empty()
            && (self
                .module
                .includes
                .iter()
                .any(|inc| !inc.profile && inc.scope_path.join(".") == path)
                || self.schema_for_path(path).is_some()
                || self
                    .table_id_at_path(path)
                    .is_some_and(|scope| self.module.depends_on.contains_key(&scope)));
        let empty = self.table_is_empty(id) && !has_scope_meta;
        let has_non_empty_extras = self
            .module
            .repeated_tables
            .get(&id)
            .is_some_and(|bodies| bodies.iter().any(|body| !self.table_is_empty(*body)));
        if empty && has_non_empty_extras {
            // An empty body contributes nothing; emitting it inline would
            // replace the merged table on the next parse. Emit only the
            // non-empty repeated bodies.
            self.emit_repeated_tables(id, path, key, depth, is_root, profile);
            return;
        }
        match self.style {
            Style::Indent => {
                // Indentation style cannot express a bodyless block, so an
                // empty table is emitted in its inline form (`key = {}`).
                if empty {
                    self.out
                        .push_str(&format!("{}{key} = {{}}", INDENT.repeat(depth)));
                    self.emit_trailing(path);
                    self.out.push('\n');
                    // An empty table can still own trailing comments. At
                    // the document root, a following entry's canonical
                    // blank line goes BEFORE those comments: leaving it
                    // after them makes the next parse re-attach them to
                    // the following key and breaks idempotence.
                    if let Some(comments) = self.trailing_block.remove(path) {
                        if is_root && has_next {
                            self.out.push('\n');
                            self.pending_blank = true;
                        }
                        for text in comments {
                            self.out.push_str(&INDENT.repeat(depth));
                            self.out.push_str(&text);
                            self.out.push('\n');
                        }
                    }
                    self.emit_repeated_tables(id, path, key, depth, is_root, profile);
                    return;
                }
                self.out
                    .push_str(&format!("{}{key}:", INDENT.repeat(depth)));
                self.emit_trailing(path);
                self.out.push('\n');
                self.emit_table_body_with(id, depth + 1, path, false, profile);
                self.emit_repeated_tables(id, path, key, depth, is_root, profile);
            }
            _ => {
                if is_root && depth == 0 {
                    // Top-level tables use `[name]` headers (TOML-ish); a
                    // trailing comment stays on the header line.
                    self.out.push_str(&format!("[{key}]"));
                    self.emit_trailing(path);
                    self.out.push('\n');
                    // Same blank-line rule as the indentation branch: a
                    // following entry's separator must precede the empty
                    // table's trailing comments.
                    if empty
                        && has_next
                        && self
                            .trailing_block
                            .get(path)
                            .is_some_and(|comments| !comments.is_empty())
                    {
                        self.out.push('\n');
                        self.pending_blank = true;
                    }
                    self.emit_table_body_with(id, 0, path, false, profile);
                    self.emit_repeated_tables(id, path, key, depth, is_root, profile);
                } else if empty {
                    // Inline form: a `key {\n}` block followed by a
                    // comment makes the next parse attach that comment
                    // inside the empty block. `key = {}` is stable.
                    self.out
                        .push_str(&format!("{}{key} = {{}}", INDENT.repeat(depth)));
                    self.emit_trailing(path);
                    self.out.push('\n');
                    self.emit_repeated_tables(id, path, key, depth, is_root, profile);
                } else {
                    self.out
                        .push_str(&format!("{}{key} {{", INDENT.repeat(depth)));
                    self.emit_trailing(path);
                    self.out.push('\n');
                    self.emit_table_body_with(id, depth + 1, path, false, profile);
                    self.out.push_str(&format!("{}}}\n", INDENT.repeat(depth)));
                    self.emit_repeated_tables(id, path, key, depth, is_root, profile);
                }
            }
        }
    }

    /// True when a node came from an included file. A resolved module
    /// materializes those values as fields; the formatter must not also
    /// emit them as literals (the include statement recreates them at
    /// Include priority).
    fn is_include_node(&self, id: NodeId) -> bool {
        let Some(file) = self.module.node_files.get(&id) else {
            return false;
        };
        self.module
            .include_pins
            .iter()
            .any(|pin| &pin.stamp == file)
    }

    /// True when a body (its snapshot, for a first body) has no fields of
    /// its own; include-provided fields are recreated by the include.
    fn table_is_empty(&self, id: NodeId) -> bool {
        let id = self.resolve_ref(id);
        let fields = match self.module.body_fields.get(&id) {
            Some(snapshot) => snapshot.clone(),
            None => match self.module.get(id) {
                Some(Node::Table { fields }) => fields.clone(),
                _ => return true,
            },
        };
        fields.values().all(|node| self.is_include_node(*node))
    }

    /// Re-emit each extra body of a repeated table as its own statement
    /// (L-P09 phase 3). Scope metadata (includes, schema, depends_on)
    /// stays with the first body; only fields repeat. Empty extra bodies
    /// are omitted: they contribute no value, and an inline empty table
    /// would replace the merged table on the next parse.
    fn emit_repeated_tables(
        &mut self,
        id: NodeId,
        path: &str,
        key: &str,
        depth: usize,
        is_root: bool,
        profile: bool,
    ) {
        let extras = match self.module.repeated_tables.get(&id) {
            Some(bodies) => bodies.clone(),
            None => return,
        };
        for body in extras {
            if self.table_is_empty(body) {
                continue;
            }
            self.emit_table_block(body, path, key, depth, is_root, false, profile);
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
                self.emit_profile_body(id, name, depth + 1);
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
                    self.emit_profile_body(id, name, depth + 1);
                    self.out.push_str(&format!("{}}}\n", INDENT.repeat(depth)));
                }
            }
        }
    }

    /// A profile's body. In flat mode the body's tables become headers at
    /// the body depth (profiles nest one level, so a deep profile tree
    /// would otherwise exceed the parser's block budget too).
    fn emit_profile_body(&mut self, id: NodeId, name: &str, depth: usize) {
        if !self.flat {
            self.emit_table_body_with(id, depth, name, false, true);
            return;
        }
        let fields = match self.module.get(id) {
            Some(Node::Table { fields }) => fields.clone(),
            _ => return,
        };
        self.emit_flat_body(id, &fields, name, depth);
        for (key, node) in &fields {
            if !self.is_block_table(node, key) {
                continue;
            }
            let child_full = format!("{name}.{key}");
            self.emit_flat_table(*node, &child_full, key, depth);
        }
    }

    /// Emit one `include` statement (spec §2.7) at the given depth.
    fn emit_include(&mut self, inc: &IncludeStmt, depth: usize) {
        self.out.push_str(&INDENT.repeat(depth));
        self.out.push_str(&format!("include {:?}", inc.path));
        if let Some(hash) = &inc.expected_hash {
            self.out.push_str(&format!(" sha256:{hash}"));
        }
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
        self.emit_leading(&key, 0);
        self.out.push_str(&text);
        self.emit_trailing(&key);
        self.out.push('\n');
    }

    /// Emit a statement's leading comments at `depth`. The indentation is
    /// part of the comment's meaning to the PARSER: attachment is scored by
    /// the comment's column, so a nested comment rewritten to column 0 can
    /// re-attach to a different block on the next parse (a formatter must
    /// preserve the column it was written at).
    fn emit_leading(&mut self, key: &str, depth: usize) {
        // Take, not get: a comment bucket is emitted exactly once. Repeated
        // statements share a qualified key (two `@remove("a")` patches), and
        // re-reading the bucket emitted the same comment for each of them,
        // doubling it on the next pass (local random search: identical patch
        // lines).
        if let Some(comments) = self.leading.remove(key) {
            for text in comments {
                self.out.push_str(&INDENT.repeat(depth));
                self.out.push_str(&text);
                self.out.push('\n');
            }
        }
    }

    fn emit_trailing(&mut self, key: &str) {
        // Same one-shot rule as `emit_leading`.
        if let Some(texts) = self.trailing.remove(key) {
            for text in texts {
                self.out.push(' ');
                self.out.push_str(&text);
            }
        }
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    /// Follow `Ref` links to the node that holds the value. The
    /// dependency graph is acyclic (S001 refuses cycles), so this
    /// terminates; the step bound is defense in depth.
    /// The surviving table node at a document path, for scope metadata.
    fn table_id_at_path(&self, path: &str) -> Option<NodeId> {
        let mut node = self.module.root?;
        for segment in path.split('.').filter(|segment| !segment.is_empty()) {
            match self.module.get(self.resolve_ref(node)) {
                Some(Node::Table { fields }) => node = *fields.get(segment)?,
                _ => return None,
            }
        }
        Some(self.resolve_ref(node))
    }

    fn resolve_ref(&self, mut id: NodeId) -> NodeId {
        for _ in 0..1024 {
            match self.module.get(id) {
                Some(Node::Ref(next)) => id = *next,
                _ => return id,
            }
        }
        id
    }

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
                // `secret.env("NAME")` with a name that cannot be a
                // bare identifier has no `env.NAME` spelling. Keep the
                // string form so the name survives a round trip.
                if let Some(Node::Env {
                    name,
                    default: None,
                }) = self.module.get(*locator)
                {
                    if !is_bare_identifier(name) {
                        return format!("secret.env(\"{}\")", escape(name));
                    }
                }
                format!("secret({})", self.render_expr(*locator))
            }
            Some(Node::ContentAddressed { media_type, data }) => {
                format!(
                    "blob({}, {})",
                    self.render_expr(*media_type),
                    self.render_expr(*data)
                )
            }
            // A Ref usually names a key or `let`, but it can point at
            // an unnamed expression node (an inline value that the
            // resolver rewrote). Rendering `<?>` there produced text
            // that does not parse, which the formatter contract
            // forbids; inline the target expression instead. The
            // dependency graph is acyclic (S001 refuses cycles), so
            // this recursion terminates.
            Some(Node::Ref(target)) => match self.names.get(target).cloned() {
                Some(name) => name,
                None => self.render_expr(*target),
            },
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
                format!("{}{}", self.render_expr(*value), unit.spelling())
            }
            Some(Node::Semantic { tag, inner }) => {
                format!("@{} {}", tag.as_str(), self.render_ml(*inner))
            }
            // Unreachable for every node the parser builds (named nodes
            // render by name, literals and containers by structure); the
            // old `<?>` placeholder broke re-parsing for the WHOLE
            // document, so the fallback is a parseable literal.
            _ => "null".into(),
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
            Some(Node::Literal(Value::String(text)))
                // The verbatim form is only unambiguous when a quote at
                // either edge cannot merge with the delimiter (four
                // adjacent quotes re-parse as a different token) and the
                // delimiter itself does not occur.
                if !text.contains("'''")
                    && !text.contains('\r')
                    && !text.starts_with('\'')
                    && !text.ends_with('\'') =>
            {
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

/// Whether a name can be written in the bare `env.NAME` form: an
/// identifier-shaped ASCII name. Anything else keeps the
/// `secret.env("...")` spelling so the value round-trips exactly.
fn is_bare_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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
            // Double-quoted with `\${` escapes: `${` stays literal while
            // \r, \n, and \t keep a spelled form. A single-quoted string
            // cannot carry a bare CR (the parser normalizes it), which
            // broke idempotence for a `${` string containing one.
            format!("\"{}\"", escape_interp_literal(s))
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
