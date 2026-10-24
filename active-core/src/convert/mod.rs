//! Conversion between Active Value and JSON / simple map representations

use crate::ir::Value;
use serde_json::{json, Value as JsonValue};

pub mod capability;
pub mod compat;
pub mod doc;
pub mod explain;
pub mod export;
pub mod gen;
pub mod import;
pub mod inspect;
pub mod project;
pub mod repair;
pub mod suggest;
pub mod value_inspect;

pub use capability::{
    profile as sandbox_profile, profiles as sandbox_profiles, refused as refused_capabilities,
};
pub use compat::{compare as compare_schemas, CompatFinding};
pub use doc::{collect_items, doc_to_html, doc_to_man, doc_to_markdown, DocItem};
pub use project::project as project_path;
pub use repair::{
    repair as repair_document, repair_with_engine, PlannedStep, RepairOutcome, StepStatus,
};

pub use explain::{explain_all, explain_code, explain_structured, Explanation};
pub use export::{value_to_toml_string, value_to_yaml_string};
pub use gen::{
    schema_to_go, schema_to_json_schema, schema_to_openapi, schema_to_python, schema_to_rust,
    schema_to_typescript, CODEGEN_VERSION,
};
pub use import::{
    json_text_to_active, json_value_to_active, jsonschema_to_active, toml_text_to_active,
    yaml_text_to_active,
};
pub use inspect::{schema_inspect, INSPECT_VERSION};
pub use suggest::{suggest_patches, suggest_patches_structured, PatchKind, PatchSuggestion};
pub use value_inspect::{inspect_path, render_text as render_inspection_text};

pub fn value_to_json(v: &Value) -> JsonValue {
    match v {
        Value::Null => JsonValue::Null,
        Value::Bool(b) => json!(b),
        Value::Int(i) => json!(i),
        Value::Float(f) => json!(f),
        Value::String(s) => json!(s),
        Value::DurationMs(ms) => json!({ "ms": ms }),
        Value::Quantity(n, unit) => json!({ unit.suffix(): n }),
        Value::Array(arr) => JsonValue::Array(arr.iter().map(value_to_json).collect()),
        Value::Table(map) => {
            let mut obj = serde_json::Map::new();
            for (k, val) in map {
                obj.insert(k.clone(), value_to_json(val));
            }
            JsonValue::Object(obj)
        }
        Value::Secret(_) => json!("***SECRET***"),
        // Identity shape, same contract as the Serialize impl:
        // payload bytes serialize ONLY when the value carries them,
        // which happens exclusively under explicit materialisation
        // (engine.materialise_blobs) on an untainted blob.
        Value::Blob {
            hash,
            size,
            tainted,
            media_type,
            payload,
        } => {
            let mut inner = serde_json::Map::new();
            inner.insert(
                "hash".to_string(),
                json!(format!("blake3:{}", crate::blob::hash_hex(hash))),
            );
            inner.insert("size".to_string(), json!(size));
            inner.insert(
                "media_type".to_string(),
                // A tainted identity may carry a material media type;
                // fail closed and redact it (spec/blob.md §5).
                json!(if *tainted {
                    crate::secret::MARKER
                } else {
                    media_type
                }),
            );
            inner.insert("tainted".to_string(), json!(tainted));
            if let Some(bytes) = payload {
                inner.insert(
                    "payload_b64".to_string(),
                    json!(base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        bytes
                    )),
                );
            }
            json!({ "$active.blob": inner })
        }
    }
}

pub fn value_to_pretty_json(v: &Value) -> String {
    serde_json::to_string_pretty(&value_to_json(v)).unwrap_or_else(|_| "{}".into())
}

/// Enforce a materialisation demand (spec/blob.md §5, `--materialise-blobs`):
/// every blob in the tree must carry its payload. Tainted blobs fail
/// with B003 (never emitted), payload-less untainted blobs fail with
/// B005 (fail closed instead of silently emitting identity). Called by
/// entry points after evaluation when the caller demanded materialised
/// output; identity-only paths never call this.
pub fn require_materialised(v: &Value) -> Result<(), crate::eval::EvalError> {
    let mut stack = vec![v];
    while let Some(node) = stack.pop() {
        match node {
            Value::Array(items) => stack.extend(items.iter()),
            Value::Table(map) => stack.extend(map.values()),
            Value::Blob { tainted: true, .. } => {
                return Err(crate::eval::EvalError::BlobError {
                    code: crate::blob::code::TAINTED_MATERIALISE,
                    message: "tainted blob payloads are never emitted".to_string(),
                    span: None,
                    file: None,
                });
            }
            Value::Blob { payload: None, .. } => {
                return Err(crate::eval::EvalError::BlobError {
                    code: crate::blob::code::NO_PAYLOAD,
                    message: "blob payload was not retained; re-evaluate from source with materialisation enabled".to_string(),
                    span: None,
                    file: None,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Active brace-style emission (`key = value` lines plus `[table]`
/// headers): doubles as the unit round-trip path (every suffix from
/// `Unit::suffix()` re-parses — see `tests/unit_roundtrip.rs`), so it
/// stays exactly as is. Real TOML interchange lives in
/// `export::value_to_toml_string`.
pub fn value_to_simple_toml(v: &Value, prefix: &str) -> String {
    let mut out = String::new();
    if let Value::Table(map) = v {
        for (k, val) in map {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{}.{}", prefix, k)
            };
            match val {
                Value::Table(_) => {
                    out.push_str(&format!("\n[{}]\n", path));
                    out.push_str(&value_to_simple_toml(val, &path));
                }
                _ => {
                    out.push_str(&format!("{} = {}\n", k, value_to_toml_literal(val)));
                }
            }
        }
    }
    out
}

fn value_to_toml_literal(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        Value::DurationMs(ms) => format!("{}ms", ms),
        Value::Quantity(n, unit) => unit.format_amount(*n),
        Value::Secret(_) => "\"***SECRET***\"".into(),
        // Identity reference: TOML carries the blob identity, never
        // payload bytes (same contract as JSON).
        Value::Blob {
            hash,
            size,
            media_type,
            ..
        } => format!(
            "\"$active.blob(blake3:{}, size={size}, media={media_type})\"",
            crate::blob::hash_hex(hash)
        ),
        Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(value_to_toml_literal).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Table(_) => "{...}".into(),
    }
}
