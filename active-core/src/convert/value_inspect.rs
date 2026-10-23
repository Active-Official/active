//! Path-based value inspection (TOOLING INSPECT.001-005).
//!
//! Given an evaluated tree, report one path's value (redacted), kind,
//! taint, size, content-addressed identity, schema contract, and
//! dependency provenance. Secrets never leave this module as material:
//! values render through the canonical redacting JSON mapping, and a
//! secret cone is reported as `"***SECRET***"` with `tainted: true`.

use serde_json::{json, Value as JsonValue};

use crate::ir::{Module, Node, SchemaField, Value};
use crate::types::ActiveType;

/// Inspect one dotted/index path of an evaluated document.
pub fn inspect_path(module: &Module, root: &Value, path: &str) -> Result<JsonValue, String> {
    let cone = crate::convert::project_path(root, path)
        .map_err(|why| format!("cannot inspect `{path}`: {why}"))?;
    let kind = kind_name(cone);
    let tainted = contains_secret(cone);
    let value = crate::convert::value_to_json(cone);
    // Size of the REDACTED rendering (the bytes a consumer actually
    // receives). Secret payloads therefore report the marker length,
    // never the material's length.
    let size = serde_json::to_string(&value)
        .map(|text| text.len())
        .unwrap_or(0);
    let mut dependencies: Vec<String> = crate::graph::resolve_key(module, path)
        .map(|id| crate::graph::why_paths(module, id))
        .unwrap_or_default()
        .into_iter()
        // Internal arena labels (`nodeN`) are noise for a consumer;
        // only named paths answer "where did this come from".
        .filter(|label| !label.starts_with("node"))
        .collect();
    dependencies.dedup();
    let mut report = json!({
        "version": 1,
        "path": path,
        "kind": kind,
        "tainted": tainted,
        "value": value,
        "size_bytes": size,
        "dependencies": dependencies,
    });
    if let Some(schema) = schema_for_path(module, path) {
        report["schema"] = schema;
    }
    if let Some(root) = module.schema {
        if let Some(Node::SchemaTable { meta, .. }) = module.get(root) {
            if meta.version.is_some() || meta.min_reader.is_some() || meta.min_writer.is_some() {
                report["schema_evolution"] = json!({
                    "version": meta.version,
                    "min_reader": meta.min_reader,
                    "min_writer": meta.min_writer,
                });
            }
        }
    }
    if let Value::Blob {
        hash,
        size: blob_size,
        tainted: blob_tainted,
        media_type,
        ..
    } = cone
    {
        report["content_addressed"] = json!({
            "hash": format!("blake3:{}", crate::blob::hash_hex(hash)),
            "size": blob_size,
            "media_type": media_type,
            "tainted": blob_tainted,
        });
    }
    Ok(report)
}

/// Render an inspection report as a compact human tree.
pub fn render_text(report: &JsonValue) -> String {
    let mut out = String::new();
    let path = report["path"].as_str().unwrap_or("");
    out.push_str(&format!(
        "{path}: {} ({} bytes{})\n",
        report["kind"].as_str().unwrap_or("?"),
        report["size_bytes"].as_u64().unwrap_or(0),
        if report["tainted"].as_bool().unwrap_or(false) {
            ", tainted"
        } else {
            ""
        }
    ));
    if let Some(schema) = report.get("schema") {
        out.push_str(&format!(
            "  schema: {}{}\n",
            schema["type"].as_str().unwrap_or("?"),
            if schema["required"].as_bool().unwrap_or(false) {
                " (required)"
            } else {
                ""
            }
        ));
        if let Some(list) = schema.get("constraints").and_then(JsonValue::as_array) {
            for constraint in list {
                if let Some(text) = constraint.as_str() {
                    out.push_str(&format!("  constraint: {text}\n"));
                }
            }
        }
        if let Some(description) = schema.get("description").and_then(JsonValue::as_str) {
            out.push_str(&format!("  description: {description}\n"));
        }
        if let Some(deprecated) = schema.get("deprecated").and_then(JsonValue::as_str) {
            out.push_str(&format!("  deprecated: {deprecated}\n"));
        }
    }
    out.push_str(&format!(
        "  value: {}\n",
        serde_json::to_string(&report["value"]).unwrap_or_else(|_| "null".into())
    ));
    if let Some(blob) = report.get("content_addressed") {
        out.push_str(&format!(
            "  blob: {} {} bytes media={}\n",
            blob["hash"].as_str().unwrap_or("?"),
            blob["size"].as_u64().unwrap_or(0),
            blob["media_type"].as_str().unwrap_or("?")
        ));
    }
    if let Some(deps) = report.get("dependencies").and_then(JsonValue::as_array) {
        for dep in deps {
            if let Some(text) = dep.as_str() {
                out.push_str(&format!("  depends on: {text}\n"));
            }
        }
    }
    out
}

fn kind_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "Null",
        Value::Bool(_) => "Bool",
        Value::Int(_) => "Int",
        Value::Float(_) => "Float",
        Value::String(_) => "String",
        Value::DurationMs(_) => "Duration",
        Value::Quantity(_, _) => "Quantity",
        Value::Array(_) => "Array",
        Value::Table(_) => "Table",
        Value::Secret(_) => "Secret",
        Value::Blob { .. } => "Blob",
    }
}

fn contains_secret(value: &Value) -> bool {
    match value {
        Value::Secret(_) => true,
        Value::Blob { tainted, .. } => *tainted,
        Value::Array(items) => items.iter().any(contains_secret),
        Value::Table(map) => map.values().any(contains_secret),
        _ => false,
    }
}

/// Schema contract at a dotted path, when a schema covers it.
fn schema_for_path(module: &Module, path: &str) -> Option<JsonValue> {
    let root = module.schema?;
    let mut current = root;
    for segment in path.split('.') {
        let key = segment.split('[').next().unwrap_or(segment);
        current = match module.get(current) {
            Some(Node::SchemaTable { fields, .. }) => *fields.get(key)?,
            Some(Node::SchemaArray { element }) => *element,
            _ => return None,
        };
        // Indexed segments (a[0]) descend through the element schema.
        if segment.contains('[') {
            if let Some(Node::SchemaArray { element }) = module.get(current) {
                current = *element;
            }
        }
    }
    match module.get(current) {
        Some(Node::SchemaField { field }) => Some(schema_field_json(module, field)),
        Some(Node::SchemaTable { closed, .. }) => Some(json!({
            "type": if *closed { "Table(closed)" } else { "Table" },
            "required": false,
            "constraints": [],
        })),
        _ => None,
    }
}

fn schema_field_json(module: &Module, field: &SchemaField) -> JsonValue {
    let mut constraints: Vec<String> = Vec::new();
    if let Some(value) = literal_text(module, field.min) {
        constraints.push(format!("min {value}"));
    }
    if let Some(value) = literal_text(module, field.max) {
        constraints.push(format!("max {value}"));
    }
    if let Some(len) = literal_text(module, field.min_len) {
        constraints.push(format!("min_len {len}"));
    }
    if let Some(len) = literal_text(module, field.max_len) {
        constraints.push(format!("max_len {len}"));
    }
    if let Some(pattern) = literal_string(module, field.pattern) {
        constraints.push(format!("pattern {pattern}"));
    }
    if let Some(format) = literal_string(module, field.format) {
        constraints.push(format!("format {format}"));
    }
    if let Some(media) = literal_string(module, field.blob_media_type) {
        constraints.push(format!("media_type {media}"));
    }
    if let Some(size) = literal_text(module, field.blob_max_size) {
        constraints.push(format!("max_size {size}"));
    }
    if let Some(requires) = literal_string(module, field.requires) {
        constraints.push(format!("requires {requires}"));
    }
    let mut out = json!({
        "type": type_name(&field.typ),
        "required": field.default.is_none() && !matches!(field.typ, ActiveType::Null),
        "constraints": constraints,
    });
    if let Some(description) = literal_string(module, field.description) {
        out["description"] = json!(description);
    }
    if let Some(deprecated) = &field.deprecated {
        out["deprecated"] = json!(deprecated);
    }
    if let Some(removed_in) = &field.removed_in {
        out["removed_in"] = json!(removed_in);
    }
    out
}

fn type_name(typ: &ActiveType) -> String {
    typ.name()
}

fn literal_text(module: &Module, node: Option<crate::ir::NodeId>) -> Option<String> {
    match node.and_then(|id| module.get(id)) {
        Some(Node::Literal(Value::Int(i))) => Some(i.to_string()),
        Some(Node::Literal(Value::Float(f))) => Some(f.to_string()),
        _ => None,
    }
}

fn literal_string(module: &Module, node: Option<crate::ir::NodeId>) -> Option<String> {
    match node.and_then(|id| module.get(id)) {
        Some(Node::Literal(Value::String(text))) => Some(text.clone()),
        _ => None,
    }
}
