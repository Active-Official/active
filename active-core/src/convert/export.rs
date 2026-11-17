//! Export: Active value trees to YAML and TOML (C-046,
//! spec/converters.md). JSON export lives in the parent module and is
//! the canonical mapping both formats derive from, so all three
//! agree on shapes and on secret redaction (`***SECRET***`).

use crate::diag::{Diagnostic, Severity};
use crate::ir::Value;

/// Render the evaluated tree as YAML 1.2 (block style) through the
/// canonical JSON model: one mapping, two syntaxes.
///
/// Profile-unaware: identity-only (retained blob payloads are not
/// emitted). Secret-bearing callers use [`super::value_to_yaml_string_with`];
/// explicit materialisation uses [`value_to_yaml_string_materialised`].
pub fn value_to_yaml_string(v: &Value) -> Result<String, String> {
    serde_yaml::to_string(&super::value_to_json(v))
        .map_err(|e| format!("YAML serialization failed: {e}"))
}

/// The explicit materialisation form: emits `payload_b64` for a retained
/// untainted payload. Call only on a profile-redacted tree.
pub fn value_to_yaml_string_materialised(v: &Value) -> Result<String, String> {
    serde_yaml::to_string(&super::value_to_json_materialised(v))
        .map_err(|e| format!("YAML serialization failed: {e}"))
}

/// Render the evaluated tree as a TOML document, with export notes
/// for lossy corners (spec/converters.md §3). Returns the document
/// plus diagnostics (currently only W006 null omissions), or a hard
/// error for shapes TOML cannot hold at all (mixed-type arrays —
/// announced with a path, never silently dropped).
///
/// Profile-unaware: identity-only (retained blob payloads are not
/// emitted). Secret-bearing callers use [`super::value_to_toml_string_with`];
/// explicit materialisation uses [`value_to_toml_string_materialised`].
pub fn value_to_toml_string(v: &Value) -> Result<(String, Vec<Diagnostic>), String> {
    toml_document(v, false)
}

/// The explicit materialisation form of [`value_to_toml_string`].
pub fn value_to_toml_string_materialised(v: &Value) -> Result<(String, Vec<Diagnostic>), String> {
    toml_document(v, true)
}

fn toml_document(v: &Value, include_payloads: bool) -> Result<(String, Vec<Diagnostic>), String> {
    check_toml_homogeneous(v, "")?;
    let mut diags = Vec::new();
    let doc = match build_toml_value(v, "", include_payloads, &mut diags) {
        // The common case: a table at the root.
        Some(toml::Value::Table(map)) => toml::Value::Table(map),
        // Defensive: a scalar/array root wraps as `value = …`
        // (TOML documents are key-value sequences).
        Some(other) => {
            let mut map = toml::map::Map::new();
            map.insert("value".to_string(), other);
            toml::Value::Table(map)
        }
        None => toml::Value::Table(toml::map::Map::new()),
    };
    toml::to_string(&doc).map(|s| (s, diags)).map_err(|e| {
        // Unreachable after the homogeneity check, but never silent.
        format!("TOML serialization failed: {e}")
    })
}

/// TOML arrays hold one type: reject mixed shapes up front with
/// their path (Nulls omit, so they never count). Durations and
/// quantities export as reserved wrapper tables (LANG-B-018/B-019),
/// so they rank with tables; secrets and strings are TOML strings.
fn check_toml_homogeneous(v: &Value, path: &str) -> Result<(), String> {
    let label = if path.is_empty() { "(root)" } else { path };
    match v {
        Value::Array(items) => {
            let mut kinds = std::collections::HashSet::new();
            for (i, item) in items.iter().enumerate() {
                let item_path = format!("{path}[{i}]");
                check_toml_homogeneous(item, &item_path)?;
                if !matches!(item, Value::Null) {
                    kinds.insert(toml_kind(item));
                }
            }
            if kinds.len() > 1 {
                return Err(format!(
                    "mixed-type array at '{label}' cannot be represented in TOML"
                ));
            }
            Ok(())
        }
        Value::Table(map) => {
            for (key, val) in map {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                check_toml_homogeneous(val, &child)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn toml_kind(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int(_) => 2,
        Value::Float(_) => 3,
        Value::String(_) | Value::Secret(_) => 4,
        // Durations and quantities export as reserved wrapper tables, and
        // blobs as identity tables: one TOML kind.
        Value::DurationMs(_) | Value::Quantity(_, _) | Value::Blob { .. } => 6,
        Value::Array(_) => 5,
        Value::Table(_) => 6,
    }
}

/// TOML node for a value, or None for omitted Nulls (with a W006
/// record each — announced, never silent).
fn build_toml_value(
    v: &Value,
    path: &str,
    include_payloads: bool,
    diags: &mut Vec<Diagnostic>,
) -> Option<toml::Value> {
    let at = |key: &str| {
        if path.is_empty() {
            key.to_string()
        } else {
            format!("{path}.{key}")
        }
    };
    match v {
        // TOML has no null: omit the key and say so.
        Value::Null => {
            diags.push(null_omitted(if path.is_empty() { "(root)" } else { path }));
            None
        }
        Value::Bool(b) => Some(toml::Value::Boolean(*b)),
        Value::Int(i) => Some(toml::Value::Integer(*i)),
        Value::Float(f) => Some(toml::Value::Float(*f)),
        Value::String(s) => Some(toml::Value::String(s.clone())),
        // TOML has no duration/quantity types. Emit the SAME reserved wire
        // wrapper JSON uses (`{"$active.duration": {"ms": N}}`,
        // `{"$active.quantity": {"unit": ..., "value": N}}`), so export ->
        // import -> evaluate is a fixed point instead of the old one-way
        // magnitude string (LANG-B-018, LANG-B-019). The wrapper is explicit,
        // so the importer never guesses a type from a bare string.
        Value::DurationMs(ms) => {
            let mut inner = toml::map::Map::new();
            inner.insert(
                "ms".to_string(),
                toml::Value::Integer(i64::try_from(*ms).unwrap_or(i64::MAX)),
            );
            let mut map = toml::map::Map::new();
            map.insert("$active.duration".to_string(), toml::Value::Table(inner));
            Some(toml::Value::Table(map))
        }
        Value::Quantity(n, unit) => {
            let mut inner = toml::map::Map::new();
            inner.insert("unit".to_string(), toml::Value::String(unit.suffix()));
            inner.insert("value".to_string(), toml::Value::Float(*n));
            let mut map = toml::map::Map::new();
            map.insert("$active.quantity".to_string(), toml::Value::Table(inner));
            Some(toml::Value::Table(map))
        }
        Value::Secret(_) => Some(toml::Value::String("***SECRET***".to_string())),
        // Identity table: hash, size, media type, taint. Payload bytes
        // ride along only when the value carries them (explicit
        // materialisation demand); identity-only values omit the key.
        Value::Blob {
            hash,
            size,
            tainted,
            media_type,
            payload,
        } => {
            // Same wire wrapper as JSON/YAML: `{"$active.blob": {…}}`
            // (spec/blob.md §5), so one identity shape holds across
            // every converter. A tainted identity redacts its media
            // type.
            let mut inner = toml::map::Map::new();
            inner.insert(
                "hash".to_string(),
                toml::Value::String(format!("blake3:{}", crate::blob::hash_hex(hash))),
            );
            inner.insert(
                "size".to_string(),
                toml::Value::Integer(i64::try_from(*size).unwrap_or(i64::MAX)),
            );
            inner.insert(
                "media_type".to_string(),
                toml::Value::String(
                    crate::secret::exposed_media_type(*tainted, media_type).to_string(),
                ),
            );
            inner.insert("tainted".to_string(), toml::Value::Boolean(*tainted));
            if include_payloads && !*tainted {
                if let Some(bytes) = payload {
                    inner.insert(
                        "payload_b64".to_string(),
                        toml::Value::String(base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            bytes,
                        )),
                    );
                }
            }
            let mut map = toml::map::Map::new();
            map.insert("$active.blob".to_string(), toml::Value::Table(inner));
            Some(toml::Value::Table(map))
        }
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let item_path = format!("{path}[{i}]");
                if let Some(toml_item) = build_toml_value(item, &item_path, include_payloads, diags)
                {
                    out.push(toml_item);
                }
            }
            Some(toml::Value::Array(out))
        }
        Value::Table(map) => {
            let mut out = toml::map::Map::with_capacity(map.len());
            for (key, val) in map {
                if let Some(toml_val) = build_toml_value(val, &at(key), include_payloads, diags) {
                    // Same reserved-tag escape as JSON (LANG-F-001/F-002).
                    out.insert(crate::ir::wire_key(key), toml_val);
                }
            }
            Some(toml::Value::Table(out))
        }
    }
}

fn null_omitted(path: &str) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        code: "W006".to_string(),
        message: format!("null has no TOML representation; omitted '{path}'"),
        path: Some(path.to_string()),
        span: None,
        repair: None,
        confidence: 1.0,
        file: None,
    }
}
