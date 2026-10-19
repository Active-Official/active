//! Conversion between Active Value and JSON / simple map representations

use crate::ir::Value;
use serde_json::{json, Value as JsonValue};

pub mod doc;
pub mod export;
pub mod gen;
pub mod import;

pub use doc::{collect_items, doc_to_html, doc_to_man, doc_to_markdown, DocItem};

pub use export::{value_to_toml_string, value_to_yaml_string};
pub use gen::{
    schema_to_go, schema_to_json_schema, schema_to_openapi, schema_to_rust, schema_to_typescript,
};
pub use import::{
    json_text_to_active, json_value_to_active, toml_text_to_active, yaml_text_to_active,
};

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
    }
}

pub fn value_to_pretty_json(v: &Value) -> String {
    serde_json::to_string_pretty(&value_to_json(v)).unwrap_or_else(|_| "{}".into())
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
        Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(value_to_toml_literal).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Table(_) => "{...}".into(),
    }
}
