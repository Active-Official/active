//! Documentation generation: schema + intent + comments →
//! Markdown, searchable HTML, and man pages (C-048, spec/docgen.md).
//! Secret values render redacted in every format (C-036).

use super::gen::literal_string;
use crate::ir::{AnnotationKind, Attachment, Module, Node, NodeId, Value};
use crate::types::{ActiveType, Unit};
use std::collections::HashMap;

/// One documented configuration item: a dotted value path with its
/// type, value, and prose.
#[derive(Debug, Clone)]
pub struct DocItem {
    pub path: String,
    pub depth: usize,
    pub section: String,
    pub is_table: bool,
    pub type_text: String,
    pub required: bool,
    pub default_text: Option<String>,
    pub value_text: String,
    pub description: String,
    pub constraints: Vec<String>,
    pub deprecated: Option<String>,
}

/// Collect documentation items depth-first in source order from the
/// evaluated value tree, enriched with schema data, annotations,
/// and attached comments.
pub fn collect_items(module: &Module, value: &Value) -> Vec<DocItem> {
    // Node per dotted path (tables win ties, like resolution) for
    // annotations and comment matching.
    let mut by_path: HashMap<String, NodeId> = HashMap::new();
    for (name, id) in &module.bindings {
        by_path.insert(name.clone(), *id);
    }
    for (id, path) in module.value_paths() {
        by_path.insert(path, id);
    }
    let mut comments: HashMap<String, Vec<String>> = HashMap::new();
    // Keyed by path only (no file): intended — an included schema's
    // contract comments and the document's own notes on the same key
    // render together. Attachment itself already matched (line, file)
    // at parse time, so same-path entries from different files are
    // genuinely about the same key.
    for trivia in &module.trivia {
        let key = match &trivia.attachment {
            Attachment::Leading(key) | Attachment::Trailing(key) => key.clone(),
            _ => continue,
        };
        let text = strip_comment(&trivia.text);
        // Annotation payloads (`# @intent: …`) already render from
        // the parsed annotations — echoing their source comments
        // would duplicate them verbatim.
        if text.starts_with('@') {
            continue;
        }
        comments.entry(key).or_default().push(text);
    }
    let mut items = Vec::new();
    if let Value::Table(map) = value {
        for (key, val) in map {
            walk_value(module, val, key, 0, &by_path, &comments, &mut items);
        }
    }
    items
}

/// One value plus, for containers, its descendants: tables recurse
/// by key, arrays by index (`items[0]`). Sections group by the
/// first segment (`General` for root-level scalars).
fn walk_value(
    module: &Module,
    val: &Value,
    path: &str,
    depth: usize,
    by_path: &HashMap<String, NodeId>,
    comments: &HashMap<String, Vec<String>>,
    items: &mut Vec<DocItem>,
) {
    let section = if depth == 0 && !matches!(val, Value::Table(_) | Value::Array(_)) {
        "General".to_string()
    } else {
        path.split('.')
            .next()
            .unwrap_or(path)
            .split('[')
            .next()
            .unwrap_or(path)
            .to_string()
    };
    let node = by_path.get(path).copied();
    let schema = schema_node_at(module, path);
    let is_table = matches!(val, Value::Table(_));
    let (type_text, required, default_text, constraints, deprecated) =
        describe_schema(module, schema, val);
    let mut description_parts: Vec<String> = Vec::new();
    // Schema `description` metadata leads: the contract prose lives with
    // the contract, and docgen is its renderer (spec/schema-extensions
    // §2). Validation never reads it; rendering always surfaces it.
    if let Some(text) = schema_description(module, schema) {
        description_parts.push(text);
    }
    if let Some(id) = node {
        if let Some(anns) = module.annotations.get(&id) {
            for ann in anns {
                match &ann.kind {
                    AnnotationKind::Intent(text) => description_parts.push(text.clone()),
                    AnnotationKind::Constraint { source, .. } => {
                        description_parts.push(format!("Constraint: `{source}`."));
                    }
                    AnnotationKind::AiImmutable(true) => {
                        description_parts.push("Agent-immutable.".to_string());
                    }
                    AnnotationKind::AiImmutable(false) => {}
                }
            }
        }
    }
    if let Some(lines) = comments.get(path) {
        description_parts.extend(lines.iter().cloned());
    }
    // Bare `#` markers contribute nothing; drop empties so they
    // never widen paragraph spacing.
    description_parts.retain(|part| !part.trim().is_empty());
    items.push(DocItem {
        path: path.to_string(),
        depth,
        section,
        is_table,
        type_text,
        required,
        default_text,
        value_text: render_value(val),
        description: description_parts.join("\n\n"),
        constraints,
        deprecated,
    });
    match val {
        Value::Table(nested) => {
            for (key, child) in nested {
                walk_value(
                    module,
                    child,
                    &format!("{path}.{key}"),
                    depth + 1,
                    by_path,
                    comments,
                    items,
                );
            }
        }
        Value::Array(elements) => {
            for (i, element) in elements.iter().enumerate() {
                walk_value(
                    module,
                    element,
                    &format!("{path}[{i}]"),
                    depth + 1,
                    by_path,
                    comments,
                    items,
                );
            }
        }
        _ => {}
    }
}

/// Schema `description` metadata for an item, if the schema node is a
/// field carrying one. Tables and arrays carry no description slot;
/// their prose comes from comments and annotations as before.
fn schema_description(module: &Module, schema: Option<NodeId>) -> Option<String> {
    match module.get(schema?) {
        Some(Node::SchemaField { field }) => literal_string(module, field.description),
        _ => None,
    }
}

/// Schema data for an item: (type text, required, default,
/// constraints, deprecated). Falls back to the value kind without a
/// schema; dynamic constraint expressions are skipped (validation
/// still enforces them).
fn describe_schema(
    module: &Module,
    schema: Option<NodeId>,
    val: &Value,
) -> (String, bool, Option<String>, Vec<String>, Option<String>) {
    let fallback = (value_kind_name(val), true, None, Vec::new(), None);
    let Some(id) = schema else {
        return fallback;
    };
    match module.get(id) {
        Some(Node::SchemaTable { closed, .. }) => (
            "Table".to_string(),
            true,
            None,
            if *closed {
                vec!["closed table".to_string()]
            } else {
                Vec::new()
            },
            None,
        ),
        Some(Node::SchemaArray { element }) => {
            let inner = match module.get(*element) {
                Some(Node::SchemaField { field }) => field.typ.name(),
                Some(Node::SchemaTable { .. }) => "Table".to_string(),
                Some(Node::SchemaArray { .. }) => "Array".to_string(),
                _ => "Any".to_string(),
            };
            (format!("Array<{inner}>"), true, None, Vec::new(), None)
        }
        Some(Node::SchemaField { field }) => {
            let mut constraints = Vec::new();
            match &field.typ {
                ActiveType::Int => {
                    push_bound(module, field.min, "min", &mut constraints);
                    push_bound(module, field.max, "max", &mut constraints);
                }
                ActiveType::Float => {
                    push_bound(module, field.min, "min", &mut constraints);
                    push_bound(module, field.max, "max", &mut constraints);
                }
                ActiveType::String => {
                    push_bound(module, field.min_len, "min_len", &mut constraints);
                    push_bound(module, field.max_len, "max_len", &mut constraints);
                    if let Some(pattern) = literal_string(module, field.pattern) {
                        constraints.push(format!("pattern `{pattern}`"));
                    }
                }
                _ => {}
            }
            let default_text = match field.default.and_then(|d| module.get(d)) {
                Some(Node::Literal(value)) => Some(render_value(value)),
                _ => None,
            };
            let required = !matches!(field.typ, ActiveType::Null) && default_text.is_none();
            (
                field.typ.name(),
                required,
                default_text,
                constraints,
                field.deprecated.clone(),
            )
        }
        _ => fallback,
    }
}

/// Schema node for a dotted value path: table segments index
/// `SchemaTable` fields; every index in a `key[i][j]` segment descends
/// one array level, so `matrix[0][1].cell` resolves to `cell`'s field.
fn schema_node_at(module: &Module, path: &str) -> Option<NodeId> {
    let mut id = module.schema?;
    for segment in path.split('.') {
        let key_end = segment.find('[').unwrap_or(segment.len());
        let key = &segment[..key_end];
        if !key.is_empty() {
            id = match module.get(id) {
                Some(Node::SchemaTable { fields, .. }) => *fields.get(key)?,
                Some(Node::SchemaArray { element }) => *element,
                _ => return None,
            };
        }
        for _ in segment.matches('[') {
            let Some(Node::SchemaArray { element }) = module.get(id) else {
                return None;
            };
            id = *element;
        }
    }
    Some(id)
}

fn push_bound(module: &Module, bound: Option<NodeId>, name: &str, constraints: &mut Vec<String>) {
    let rendered = match bound.and_then(|id| module.get(id)) {
        Some(Node::Literal(crate::ir::Value::Int(i))) => Some(i.to_string()),
        Some(Node::Literal(crate::ir::Value::Float(f))) => Some(f.to_string()),
        _ => None,
    };
    if let Some(text) = rendered {
        constraints.push(format!("{name} {text}"));
    }
}

fn value_kind_name(val: &Value) -> String {
    match val {
        Value::Null => "Null".to_string(),
        Value::Bool(_) => "Bool".to_string(),
        Value::Int(_) => "Int".to_string(),
        Value::Float(_) => "Float".to_string(),
        Value::String(_) => "String".to_string(),
        Value::DurationMs(_) => "Duration".to_string(),
        Value::Quantity(_, _) => "Quantity".to_string(),
        Value::Array(_) => "Array".to_string(),
        Value::Table(map) => format!("Table({})", map.len()),
        Value::Secret(_) => "Secret".to_string(),
        Value::Blob { .. } => "Blob".to_string(),
    }
}

/// Short safe rendering for documentation values: scalars inline,
/// composites by kind, secrets redacted.
fn render_value(val: &Value) -> String {
    match val {
        Value::Secret(_) => "***SECRET***".to_string(),
        Value::String(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::DurationMs(ms) => format!("{ms} ms"),
        Value::Quantity(n, unit) => match unit {
            Unit::Percent => format!("{}%", n * 100.0),
            _ => format!("{n} {}", unit.suffix()),
        },
        Value::Array(items) => format!("array[{}]", items.len()),
        Value::Table(map) => format!("table{{{}}}", map.len()),
        // Identity only: docs never carry payload bytes.
        Value::Blob { hash, size, .. } => {
            format!("blob(blake3:{}, size={size})", crate::blob::hash_hex(hash))
        }
    }
}

fn strip_comment(text: &str) -> String {
    let stripped = text.trim_start_matches('#');
    stripped.strip_prefix(' ').unwrap_or(stripped).to_string()
}

// ------------------------------------------------------------------
// Markdown.
// ------------------------------------------------------------------

/// Reference page: `# Title`, one `## table` section per top-level
/// table (`## General` for root scalars), `### path` items with a
/// type/required/default/value line, description paragraphs,
/// constraint bullets, and deprecation quotes.
pub fn doc_to_markdown(title: &str, items: &[DocItem]) -> String {
    let mut out = format!("# {title}\n\n");
    let mut section: Option<&str> = None;
    for item in items {
        if section != Some(item.section.as_str()) {
            out.push_str(&format!("## {}\n\n", item.section));
            section = Some(item.section.as_str());
        }
        out.push_str(&format!("### {}\n\n", item.path));
        let mut line = format!("**Type:** {}", item.type_text);
        line.push_str(&format!(
            " · **Required:** {}",
            if item.required { "yes" } else { "no" }
        ));
        if let Some(default) = &item.default_text {
            line.push_str(&format!(" · **Default:** `{default}`"));
        }
        line.push_str(&format!(" · **Value:** `{}`", item.value_text));
        out.push_str(&line);
        out.push_str("\n\n");
        if !item.description.is_empty() {
            out.push_str(&item.description);
            out.push_str("\n\n");
        }
        for constraint in &item.constraints {
            out.push_str(&format!("- {constraint}\n"));
        }
        if !item.constraints.is_empty() {
            out.push('\n');
        }
        if let Some(reason) = &item.deprecated {
            if reason.is_empty() {
                out.push_str("> Deprecated.\n\n");
            } else {
                out.push_str(&format!("> Deprecated: {reason}\n\n"));
            }
        }
    }
    out
}

// ------------------------------------------------------------------
// HTML (single self-contained searchable page).
// ------------------------------------------------------------------

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// One page with inline CSS and a filter box: typing hides
/// non-matching `.field` blocks (substring, case-insensitive).
pub fn doc_to_html(title: &str, items: &[DocItem]) -> String {
    let mut out = String::new();
    out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str(&format!("<title>{}</title>\n", escape_html(title)));
    out.push_str(
        "<style>body{font-family:sans-serif;max-width:60em;margin:2em auto;padding:0 1em}\
        .field{border-top:1px solid #ccc;padding:.5em 0}code{background:#f4f4f4;padding:0 .3em}\
        #q{width:100%;font-size:1.1em;padding:.3em;margin:1em 0}</style>\n",
    );
    out.push_str("</head>\n<body>\n");
    out.push_str(&format!("<h1>{}</h1>\n", escape_html(title)));
    out.push_str(
        "<input type=\"search\" id=\"q\" placeholder=\"Filter fields…\" oninput=\"\
        const q=this.value.toLowerCase();\
        document.querySelectorAll('.field').forEach(el=>{\
        el.style.display=el.textContent.toLowerCase().includes(q)?'':'none'})\">\n",
    );
    let mut section: Option<&str> = None;
    for item in items {
        if section != Some(item.section.as_str()) {
            out.push_str(&format!("<h2>{}</h2>\n", escape_html(&item.section)));
            section = Some(item.section.as_str());
        }
        out.push_str("<div class=\"field\">\n");
        out.push_str(&format!(
            "<h3><code>{}</code></h3>\n",
            escape_html(&item.path)
        ));
        out.push_str("<p>");
        out.push_str(&format!(
            "<code>{}</code> · {} · <code>{}</code>",
            escape_html(&item.type_text),
            if item.required {
                "required"
            } else {
                "optional"
            },
            escape_html(&item.value_text),
        ));
        if let Some(default) = &item.default_text {
            out.push_str(&format!(" · default <code>{}</code>", escape_html(default)));
        }
        out.push_str("</p>\n");
        for para in item.description.split("\n\n") {
            if !para.is_empty() {
                out.push_str(&format!("<p>{}</p>\n", escape_html(para)));
            }
        }
        if !item.constraints.is_empty() {
            out.push_str("<ul>\n");
            for constraint in &item.constraints {
                out.push_str(&format!(
                    "<li><code>{}</code></li>\n",
                    escape_html(constraint)
                ));
            }
            out.push_str("</ul>\n");
        }
        if let Some(reason) = &item.deprecated {
            if reason.is_empty() {
                out.push_str("<p><strong>Deprecated.</strong></p>\n");
            } else {
                out.push_str(&format!(
                    "<p><strong>Deprecated:</strong> {}</p>\n",
                    escape_html(reason)
                ));
            }
        }
        out.push_str("</div>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}

// ------------------------------------------------------------------
// Man page.
// ------------------------------------------------------------------

/// Escape one output line for man(7): backslashes doubled, and a
/// `\&` guard before leading `'` or `.` (which would otherwise open
/// a request or macro).
fn escape_man_line(line: &str) -> String {
    let mut out = line.replace('\\', "\\\\");
    if out.starts_with('\'') || out.starts_with('.') {
        out.insert_str(0, "\\&");
    }
    out
}

/// Man page: `.TH` header, `.SH NAME` summary, one `.TP` entry per
/// item with type/required/value/description body lines.
pub fn doc_to_man(title: &str, items: &[DocItem]) -> String {
    let mut out = format!(".TH \"{}\" 5\n", title.to_uppercase().replace('"', ""));
    out.push_str(".SH NAME\n");
    out.push_str(&escape_man_line(&format!(
        "{title} \\- configuration reference"
    )));
    out.push('\n');
    out.push_str(".SH FIELDS\n");
    for item in items {
        out.push_str(".TP\n");
        out.push_str(&escape_man_line(&item.path));
        out.push('\n');
        let mut body = format!(
            "Type: {}. Required: {}. Value: {}.",
            item.type_text,
            if item.required { "yes" } else { "no" },
            item.value_text
        );
        if let Some(default) = &item.default_text {
            body.push_str(&format!(" Default: {default}."));
        }
        if !item.constraints.is_empty() {
            body.push_str(&format!(" Constraints: {}.", item.constraints.join("; ")));
        }
        if let Some(reason) = &item.deprecated {
            if reason.is_empty() {
                body.push_str(" Deprecated.");
            } else {
                body.push_str(&format!(" Deprecated: {reason}."));
            }
        }
        for line in body.split('\n') {
            out.push_str(&escape_man_line(line));
            out.push('\n');
        }
        for para in item.description.split("\n\n") {
            if para.is_empty() {
                continue;
            }
            for line in para.split('\n') {
                out.push_str(&escape_man_line(line));
                out.push('\n');
            }
            out.push_str(".br\n");
        }
    }
    out
}
