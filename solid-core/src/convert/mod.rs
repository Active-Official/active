//! Conversion between SOLID Value and JSON / simple map representations

use crate::ir::Value;
use serde_json::{json, Value as JsonValue};

pub fn value_to_json(v: &Value) -> JsonValue {
    match v {
        Value::Null => JsonValue::Null,
        Value::Bool(b) => json!(b),
        Value::Int(i) => json!(i),
        Value::Float(f) => json!(f),
        Value::String(s) => json!(s),
        Value::DurationMs(ms) => json!({ "ms": ms }),
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

/// Extremely simple TOML-ish dump (not a full TOML serializer)
/// TODO(solid C-046): replace with a full bidirectional TOML/YAML/JSON
/// converter with uniform secret redaction.
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
        Value::Secret(_) => "\"***SECRET***\"".into(),
        Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(value_to_toml_literal).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Table(_) => "{...}".into(),
    }
}
