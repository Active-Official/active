//! Stable schema introspection surface (CHECKLIST C.007,
//! PROPOSAL §3.2): the schema as a queryable contract for agents.
//!
//! `schema_inspect` walks the schema tree and returns versioned JSON
//! describing every field: dotted path, type, required flag, static
//! default, constraints, description, deprecation, removed_in,
//! requires target, enum members, table closure, schema evolution
//! metadata (version, min_reader, min_writer), and evaluation mode.
//! Validation never reads this output; agents query it instead of
//! parsing schema source text.

use super::gen::{literal_default, literal_string};
use crate::ir::{Module, Node, NodeId, SchemaField};
use crate::types::ActiveType;
use serde_json::{json, Map as JsonMap, Value as JsonValue};

/// Introspection document version: a single integer, exact-match.
/// Any shape change (added, removed, or retyped keys) bumps it; additive
/// and breaking changes are not distinguished, so consumers pin the exact
/// value they were built against and reject anything else.
/// v2: schema evolution metadata (schema_version, min_reader,
/// min_writer), per-field removed_in, and top-level mode.
pub const INSPECT_VERSION: u32 = 2;

/// Parser's own nesting ceiling, mirrored here so hand-built arenas can
/// never recurse deeper than parsed documents.
const MAX_DEPTH: usize = 64;

fn schema_root(module: &Module) -> Result<NodeId, String> {
    module
        .schema
        .ok_or_else(|| "document has no schema block".to_string())
}

/// One field entry plus recursion into nested tables and arrays.
/// `depth` mirrors the parser ceiling so hand-built arenas terminate.
fn push_field(
    module: &Module,
    out: &mut Vec<JsonValue>,
    path: String,
    name: String,
    id: NodeId,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    match module.get(id) {
        Some(Node::SchemaTable {
            fields,
            closed,
            meta,
            ..
        }) => {
            out.push(json!({
                "path": path,
                "name": name,
                "kind": "table",
                "type": "Table",
                "required": true,
                "default": JsonValue::Null,
                "constraints": {},
                "description": JsonValue::Null,
                "deprecated": JsonValue::Null,
                "requires": JsonValue::Null,
                "members": JsonValue::Null,
                "member_descriptions": JsonValue::Null,
                "closed": closed,
                "version": meta.version,
                "min_reader": meta.min_reader,
                "min_writer": meta.min_writer,
                "element": JsonValue::Null,
            }));
            for (key, child) in fields {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                push_field(module, out, child_path, key.clone(), *child, depth + 1);
            }
        }
        Some(Node::SchemaArray { element }) => {
            let element_type = match module.get(*element) {
                Some(Node::SchemaField { field }) => stable_type(field),
                Some(Node::SchemaTable { .. }) => "Table".to_string(),
                Some(Node::SchemaArray { .. }) => "Array".to_string(),
                _ => "Any".to_string(),
            };
            out.push(json!({
                "path": path,
                "name": name,
                "kind": "array",
                "type": "Array",
                "required": true,
                "default": JsonValue::Null,
                "constraints": {},
                "description": JsonValue::Null,
                "deprecated": JsonValue::Null,
                "requires": JsonValue::Null,
                "members": JsonValue::Null,
                "member_descriptions": JsonValue::Null,
                "closed": JsonValue::Null,
                "element": element_type,
            }));
            // Constrained element fields (Array<String(min_len = 1)>) carry
            // their own entry so agents see element constraints; nested
            // tables and arrays recurse for their structure.
            match module.get(*element) {
                Some(Node::SchemaField { field }) => out.push(field_entry(
                    module,
                    format!("{path}[]"),
                    name.clone(),
                    field,
                )),
                Some(Node::SchemaTable { .. }) | Some(Node::SchemaArray { .. }) => push_field(
                    module,
                    out,
                    format!("{path}[]"),
                    name.clone(),
                    *element,
                    depth + 1,
                ),
                _ => {}
            }
        }
        Some(Node::SchemaField { field }) => out.push(field_entry(module, path, name, field)),
        _ => {}
    }
}

fn opt_string(value: Option<String>) -> JsonValue {
    value.map(JsonValue::String).unwrap_or(JsonValue::Null)
}

/// Stable type label for the `type` key: `ActiveType::name` is
/// display-oriented (`Enum(red, green)`), but agents need a fixed
/// vocabulary — members live in `members`, not in the label.
fn stable_type(field: &SchemaField) -> String {
    match &field.typ {
        ActiveType::Enum(_) => "Enum".to_string(),
        ActiveType::Blob => "Blob".to_string(),
        other => other.name(),
    }
}

/// Enum description texts, strict like the generators: any non-string
/// element voids the whole list (length then cannot match members, so
/// the entry reports Null instead of a silently filtered subset).
fn string_list(module: &Module, id: Option<NodeId>) -> Vec<String> {
    let texts: Option<Vec<String>> = match id.and_then(|node| module.get(node)) {
        Some(Node::Literal(crate::ir::Value::Array(items))) => items
            .iter()
            .map(|v| match v {
                crate::ir::Value::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        Some(Node::Array { elements }) => elements
            .iter()
            .map(|e| match module.get(*e) {
                Some(Node::Literal(crate::ir::Value::String(text))) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        _ => return Vec::new(),
    };
    texts.unwrap_or_default()
}

fn field_entry(module: &Module, path: String, name: String, field: &SchemaField) -> JsonValue {
    let mut constraints = JsonMap::new();
    let mut members = JsonValue::Null;
    let mut member_descriptions = JsonValue::Null;
    let mut closed = JsonValue::Null;
    match &field.typ {
        ActiveType::Int | ActiveType::Float => {
            if let Some(Node::Literal(v)) = field.min.and_then(|id| module.get(id)) {
                constraints.insert("min".to_string(), super::value_to_json(v));
            }
            if let Some(Node::Literal(v)) = field.max.and_then(|id| module.get(id)) {
                constraints.insert("max".to_string(), super::value_to_json(v));
            }
        }
        ActiveType::String => {
            if let Some(Node::Literal(v)) = field.min_len.and_then(|id| module.get(id)) {
                constraints.insert("min_len".to_string(), super::value_to_json(v));
            }
            if let Some(Node::Literal(v)) = field.max_len.and_then(|id| module.get(id)) {
                constraints.insert("max_len".to_string(), super::value_to_json(v));
            }
            if let Some(pattern) = literal_string(module, field.pattern) {
                constraints.insert("pattern".to_string(), JsonValue::String(pattern));
            }
            if let Some(format) = literal_string(module, field.format) {
                constraints.insert("format".to_string(), JsonValue::String(format));
            }
        }
        ActiveType::Enum(options) => {
            members = JsonValue::Array(options.iter().cloned().map(JsonValue::String).collect());
            let texts = string_list(module, field.enum_descriptions);
            if texts.len() == options.len() {
                member_descriptions =
                    JsonValue::Array(texts.into_iter().map(JsonValue::String).collect());
            }
        }
        ActiveType::Table { closed: is_closed } => {
            closed = JsonValue::Bool(*is_closed);
        }
        ActiveType::Blob => {
            if let Some(media) = literal_string(module, field.blob_media_type) {
                constraints.insert("media_type".to_string(), JsonValue::String(media));
            }
            if let Some(Node::Literal(v)) = field.blob_max_size.and_then(|id| module.get(id)) {
                constraints.insert("max_size".to_string(), super::value_to_json(v));
            }
        }
        _ => {}
    }
    // NodeId-keyed lookup needs the field's own id; the caller passes the
    // entry path only, so required-ness is recomputed from the same rule
    // the generators use (static default or Null excuses).
    let required =
        !matches!(field.typ, ActiveType::Null) && literal_default(module, field).is_none();
    json!({
        "path": path,
        "name": name,
        "kind": "field",
        "type": stable_type(field),
        "required": required,
        "default": literal_default(module, field).unwrap_or(JsonValue::Null),
        "constraints": JsonValue::Object(constraints),
        "description": opt_string(literal_string(module, field.description)),
        "deprecated": opt_string(field.deprecated.clone()),
        "removed_in": opt_string(field.removed_in.clone()),
        "requires": opt_string(literal_string(module, field.requires)),
        "members": members,
        "member_descriptions": member_descriptions,
        "closed": closed,
        "element": JsonValue::Null,
    })
}

/// Inspect a parsed module's schema block: versioned JSON with one entry
/// per field in source order (depth-first, dotted paths). Schema
/// evolution metadata (version, min_reader, min_writer) rides at the
/// top level so agents discover the contract without parsing source.
/// Errors only when the document has no schema block.
pub fn schema_inspect(module: &Module) -> Result<JsonValue, String> {
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for inspection".to_string());
    }
    let mut fields = Vec::new();
    push_field(module, &mut fields, String::new(), String::new(), root, 0);
    // The root table entry itself carries no field data; drop it so the
    // output is exactly the field list agents query.
    fields.retain(|entry| !entry["path"].as_str().unwrap_or("").is_empty());
    let (schema_version, min_reader, min_writer) = match module.get(root) {
        Some(Node::SchemaTable { meta, .. }) => (
            meta.version
                .clone()
                .map(JsonValue::String)
                .unwrap_or(JsonValue::Null),
            meta.min_reader
                .clone()
                .map(JsonValue::String)
                .unwrap_or(JsonValue::Null),
            meta.min_writer
                .clone()
                .map(JsonValue::String)
                .unwrap_or(JsonValue::Null),
        ),
        _ => (JsonValue::Null, JsonValue::Null, JsonValue::Null),
    };
    Ok(json!({
        "version": INSPECT_VERSION,
        "schema_version": schema_version,
        "min_reader": min_reader,
        "min_writer": min_writer,
        "mode": match module.mode {
            crate::ir::Mode::Data => "data",
            crate::ir::Mode::Config => "config",
        },
        "fields": fields,
    }))
}
