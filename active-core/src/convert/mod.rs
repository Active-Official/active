//! Conversion between Active Value and JSON / simple map representations

use crate::ir::Value;
use crate::secret::{SecretProfile, SecretRedactor};
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
pub use doc::{
    collect_items, collect_items_with_profile, doc_to_html, doc_to_man, doc_to_markdown, DocItem,
};
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
pub use value_inspect::{
    inspect_path, inspect_path_with_profile, inspect_path_with_profile_env,
    render_text as render_inspection_text,
};

/// Profile-unaware JSON mapping. A materialised blob payload is emitted
/// as-is; callers handling secret material must use [`value_to_json_with`]
/// (or [`redact_value`] first), which sweeps authored and runtime
/// secrets.
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
            if !*tainted {
                if let Some(bytes) = payload {
                    inner.insert(
                        "payload_b64".to_string(),
                        json!(base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            bytes
                        )),
                    );
                }
            }
            json!({ "$active.blob": inner })
        }
    }
}

/// Seed a redactor with the authored literals plus every runtime secret
/// reachable in the evaluated tree. A runtime secret (an env read
/// wrapped by `secret()`) has no authored literal to find, and a
/// profile-selected plain value is material only by its path, so both
/// forms must be collected here; their copies in a plain string, a blob
/// media type, or materialised payload bytes would otherwise render
/// verbatim.
pub fn redactor_with_runtime_secrets(profile: &SecretProfile, value: &Value) -> SecretRedactor {
    fn seed(profile: &SecretProfile, redactor: &mut SecretRedactor, path: &str, value: &Value) {
        match value {
            Value::Secret(text) => redactor.insert(text.clone()),
            // Seed a plain string only when its node is globally
            // material. A profile selection can place a BENIGN value at
            // a material path; seeding that would hide the same value
            // in its own data slot. A selected env read is in `nodes`
            // (its env name is material), so it still seeds.
            Value::String(text)
                if profile
                    .value_node_at(path)
                    .is_some_and(|id| profile.node_is_secret(id)) =>
            {
                redactor.insert(text.clone());
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    seed(profile, redactor, &format!("{path}[{index}]"), item);
                }
            }
            Value::Table(map) => {
                for (key, item) in map {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    seed(profile, redactor, &child, item);
                }
            }
            _ => {}
        }
    }
    let mut redactor = profile.literals().clone();
    seed(profile, &mut redactor, "", value);
    redactor
}

/// True when a blob payload's raw bytes carry a secret. The bytes are
/// swept before base64 encoding, which cannot be matched against a
/// literal again.
pub(crate) fn blob_payload_is_material(
    redactor: &SecretRedactor,
    profile: &SecretProfile,
    payload: Option<&Vec<u8>>,
) -> bool {
    payload.is_some_and(|bytes| {
        // A short material value is not in the substring redactor; an
        // exact UTF-8 payload match still counts.
        if let Ok(text) = std::str::from_utf8(bytes) {
            if profile.is_material_text(text) {
                return true;
            }
        }
        redactor
            .redact_text(&String::from_utf8_lossy(bytes))
            .is_some()
    })
}

/// Identity shape for a blob whose identity must be withheld: the
/// payload is absent and the media type is the marker, wrapped exactly
/// like every other export (spec/blob.md §5).
pub(crate) fn redacted_blob_json(hash: &[u8; 32], size: u64) -> JsonValue {
    json!({ "$active.blob": {
        "hash": format!("blake3:{}", crate::blob::hash_hex(hash)),
        "size": size,
        "media_type": crate::secret::MARKER,
        "tainted": true,
    }})
}

/// Profile-unaware pretty JSON. Secret-bearing callers must use
/// [`value_to_pretty_json_with`].
pub fn value_to_pretty_json(v: &Value) -> String {
    serde_json::to_string_pretty(&value_to_json(v)).unwrap_or_else(|_| "{}".into())
}

/// Deep copy with every structurally material leaf replaced by a
/// `Secret` value (which every exporter renders as the marker).
/// Containers keep their shape; a benign sibling stays visible, while
/// a material scalar of any kind is hidden. A string that merely
/// contains a known secret substring is redacted in place.
///
/// Seed the profile first when a secret may be bound outside the
/// exported tree: `Engine::evaluate_with_seeds` returns the evaluator's
/// material node values, `SecretProfile::absorb_secrets` merges them,
/// and `absorb_environment` covers material environment values. The
/// CLI, FFI, WASM, LSP, inspect, and replay front ends all do this.
pub fn redact_value(profile: &SecretProfile, value: &Value) -> Value {
    profile.begin_redaction_pass();
    // Runtime secrets live in the evaluated tree; authored literals
    // live in the profile. Copies need both.
    let redactor = redactor_with_runtime_secrets(profile, value);
    let mut blob_material: std::collections::HashMap<[u8; 32], bool> =
        std::collections::HashMap::new();
    fn walk(
        profile: &SecretProfile,
        redactor: &SecretRedactor,
        blob_material: &mut std::collections::HashMap<[u8; 32], bool>,
        path: &str,
        value: &Value,
    ) -> Value {
        match value {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        walk(
                            profile,
                            redactor,
                            blob_material,
                            &format!("{path}[{index}]"),
                            item,
                        )
                    })
                    .collect(),
            ),
            Value::Table(map) => {
                let mut out = indexmap::IndexMap::new();
                for (key, item) in map {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    out.insert(
                        key.clone(),
                        walk(profile, redactor, blob_material, &child, item),
                    );
                }
                Value::Table(out)
            }
            // A material (or already tainted) blob keeps its identity
            // shape: only the media type is redacted and the payload is
            // dropped. Exports must always carry `$active.blob`.
            Value::Blob {
                hash,
                size,
                media_type,
                tainted,
                payload,
            } => {
                // A secret can appear in identity metadata or inside a
                // payload: sweep both. Payload materiality is memoized
                // by content hash, so an aliased blob is scanned once.
                let media_redacted = redactor.redact_text(media_type);
                let media_exact = profile.is_material_text(media_type);
                let payload_secret = if payload.is_some() {
                    match blob_material.get(hash) {
                        Some(known) => *known,
                        None => {
                            let material =
                                blob_payload_is_material(redactor, profile, payload.as_ref());
                            blob_material.insert(*hash, material);
                            material
                        }
                    }
                } else {
                    false
                };
                if *tainted
                    || profile.path_is_secret(path)
                    || media_redacted.is_some()
                    || media_exact
                    || payload_secret
                {
                    Value::Blob {
                        hash: *hash,
                        size: *size,
                        media_type: crate::secret::MARKER.to_string(),
                        tainted: true,
                        payload: None,
                    }
                } else {
                    Value::Blob {
                        hash: *hash,
                        size: *size,
                        media_type: media_type.clone(),
                        tainted: false,
                        payload: payload.clone(),
                    }
                }
            }
            _ if profile.path_is_secret(path) => Value::Secret(String::new()),
            // A whole value identical to a material value (at any
            // length) is material too; short values never enter the
            // substring redactor.
            Value::String(text) if profile.is_material_text(text) => Value::Secret(String::new()),
            Value::String(text) => match redactor.redact_text(text) {
                Some(redacted) => Value::String(redacted),
                None => value.clone(),
            },
            _ => value.clone(),
        }
    }
    walk(profile, &redactor, &mut blob_material, "", value)
}

/// Drop retained blob payloads, keeping identity and taint. Content
/// hashes (audit, replay) use this so `--materialise-blobs` cannot
/// change the identity of a document.
pub fn strip_blob_payloads(value: &Value) -> Value {
    match value {
        Value::Blob {
            hash,
            size,
            media_type,
            tainted,
            ..
        } => Value::Blob {
            hash: *hash,
            size: *size,
            media_type: media_type.clone(),
            tainted: *tainted,
            payload: None,
        },
        Value::Array(items) => Value::Array(items.iter().map(strip_blob_payloads).collect()),
        Value::Table(map) => Value::Table(
            map.iter()
                .map(|(key, item)| (key.clone(), strip_blob_payloads(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// JSON export with structural-material redaction.
pub fn value_to_json_with(profile: &SecretProfile, value: &Value) -> JsonValue {
    value_to_json(&redact_value(profile, value))
}

/// Pretty JSON export with structural-material redaction.
pub fn value_to_pretty_json_with(profile: &SecretProfile, value: &Value) -> String {
    serde_json::to_string_pretty(&value_to_json_with(profile, value))
        .unwrap_or_else(|_| "{}".into())
}

/// YAML export with structural-material redaction.
pub fn value_to_yaml_string_with(profile: &SecretProfile, value: &Value) -> Result<String, String> {
    export::value_to_yaml_string(&redact_value(profile, value))
}

/// TOML export with structural-material redaction.
pub fn value_to_toml_string_with(
    profile: &SecretProfile,
    value: &Value,
) -> Result<(String, Vec<crate::diag::Diagnostic>), String> {
    export::value_to_toml_string(&redact_value(profile, value))
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
            tainted,
            ..
        } => {
            let media =
                serde_json::to_string(crate::secret::exposed_media_type(*tainted, media_type))
                    .unwrap_or_else(|_| "\"\"".to_string());
            format!(
                "{{ \"$active.blob\" = {{ hash = \"blake3:{}\", size = {size}, media_type = {media}, tainted = {tainted} }} }}",
                crate::blob::hash_hex(hash),
            )
        }
        Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(value_to_toml_literal).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Table(_) => "{...}".into(),
    }
}
