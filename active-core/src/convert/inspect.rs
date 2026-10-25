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
    profile: &crate::secret::SecretProfile,
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
            // Nested table metadata is authored text too.
            let redact_meta = |value: &Option<String>| {
                value
                    .as_ref()
                    .map(|text| {
                        JsonValue::String(
                            profile
                                .redact_authored_text(text)
                                .unwrap_or_else(|| text.clone()),
                        )
                    })
                    .unwrap_or(JsonValue::Null)
            };
            let version = redact_meta(&meta.version);
            let min_reader = redact_meta(&meta.min_reader);
            let min_writer = redact_meta(&meta.min_writer);
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
                "version": version,
                "min_reader": min_reader,
                "min_writer": min_writer,
                "element": JsonValue::Null,
            }));
            for (key, child) in fields {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                push_field(
                    module,
                    profile,
                    out,
                    child_path,
                    key.clone(),
                    *child,
                    depth + 1,
                );
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
                    profile,
                    format!("{path}[]"),
                    name.clone(),
                    field,
                )),
                Some(Node::SchemaTable { .. }) | Some(Node::SchemaArray { .. }) => push_field(
                    module,
                    profile,
                    out,
                    format!("{path}[]"),
                    name.clone(),
                    *element,
                    depth + 1,
                ),
                _ => {}
            }
        }
        Some(Node::SchemaField { field }) => {
            out.push(field_entry(module, profile, path, name, field))
        }
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

fn field_entry(
    module: &Module,
    profile: &crate::secret::SecretProfile,
    path: String,
    name: String,
    field: &SchemaField,
) -> JsonValue {
    let redact = |text: String| -> String { profile.redact_authored_text(&text).unwrap_or(text) };
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
                constraints.insert("pattern".to_string(), JsonValue::String(redact(pattern)));
            }
            if let Some(format) = literal_string(module, field.format) {
                constraints.insert("format".to_string(), JsonValue::String(redact(format)));
            }
        }
        ActiveType::Enum(options) => {
            members = JsonValue::Array(
                options
                    .iter()
                    .map(|member| JsonValue::String(redact(member.clone())))
                    .collect(),
            );
            let texts = string_list(module, field.enum_descriptions);
            if texts.len() == options.len() {
                member_descriptions = JsonValue::Array(
                    texts
                        .into_iter()
                        .map(|text| JsonValue::String(redact(text)))
                        .collect(),
                );
            }
        }
        ActiveType::Table { closed: is_closed } => {
            closed = JsonValue::Bool(*is_closed);
        }
        ActiveType::Blob => {
            if let Some(media) = literal_string(module, field.blob_media_type) {
                constraints.insert("media_type".to_string(), JsonValue::String(redact(media)));
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
    let mut entry = json!({
        "path": path,
        "name": name,
        "kind": "field",
        "type": stable_type(field),
        "required": required,
        "default": literal_default(module, field).unwrap_or(JsonValue::Null),
        "constraints": JsonValue::Object(constraints),
        "description": opt_string(literal_string(module, field.description).map(redact)),
        "deprecated": opt_string(field.deprecated.clone().map(redact)),
        "removed_in": opt_string(field.removed_in.clone().map(redact)),
        "requires": opt_string(literal_string(module, field.requires).map(|target| {
            // A real field reference stays raw so the contract
            // resolves; dangling text is data and redacts.
            if super::schema_path_exists(module, &target) {
                target
            } else {
                redact(target)
            }
        })),
        "members": members,
        "member_descriptions": member_descriptions,
        "closed": closed,
        "element": JsonValue::Null,
    });
    // Redact the value-bearing slot only: `path`, `name`, `kind`, and
    // `type` are structure and must stay queryable.
    profile.redact_json(&mut entry["default"]);
    entry
}

/// Inspect a parsed module's schema block: versioned JSON with one entry
/// per field in source order (depth-first, dotted paths). Schema
/// evolution metadata (version, min_reader, min_writer) rides at the
/// top level so agents discover the contract without parsing source.
/// Errors only when the document has no schema block.
pub fn schema_inspect(module: &Module) -> Result<JsonValue, String> {
    schema_inspect_with_profile(module, None)
}

/// Schema inspection under an explicit profile override.
pub fn schema_inspect_with_profile(
    module: &Module,
    override_profile: Option<&str>,
) -> Result<JsonValue, String> {
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for inspection".to_string());
    }
    let profile = crate::secret::SecretProfile::analyze_with_profile(module, override_profile);
    let mut fields = Vec::new();
    push_field(
        module,
        &profile,
        &mut fields,
        String::new(),
        String::new(),
        root,
        0,
    );
    // The root table entry itself carries no field data; drop it so the
    // output is exactly the field list agents query.
    fields.retain(|entry| !entry["path"].as_str().unwrap_or("").is_empty());
    let redact_meta = |value: Option<String>| {
        value
            .map(|text| JsonValue::String(profile.redact_authored_text(&text).unwrap_or(text)))
            .unwrap_or(JsonValue::Null)
    };
    let (schema_version, min_reader, min_writer) = match module.get(root) {
        Some(Node::SchemaTable { meta, .. }) => (
            redact_meta(meta.version.clone()),
            redact_meta(meta.min_reader.clone()),
            redact_meta(meta.min_writer.clone()),
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
