//! Path-based value inspection (TOOLING INSPECT.001-005).
//!
//! Given an evaluated tree, report one path's value (redacted), kind,
//! taint, size, content-addressed identity, schema contract, and
//! dependency provenance. Secrets never leave this module as material:
//! values render through the canonical redacting JSON mapping, and a
//! secret cone is reported as `"***SECRET***"` with `tainted: true`.

use serde_json::{json, Value as JsonValue};

use crate::ir::{Module, Node, SchemaField, Value};
use crate::secret::{SecretProfile, MARKER};
use crate::types::ActiveType;

/// Inspect one dotted/index path of an evaluated document.
///
/// Runtime environment values are not visible to this wrapper. It does
/// recover computed out-of-tree secrets by re-evaluating the module
/// hermetically for seeds; a document that reads the host environment
/// should use [`inspect_path_with_profile_env`] with the evaluator's
/// seeds and environment.
pub fn inspect_path(module: &Module, root: &Value, path: &str) -> Result<JsonValue, String> {
    inspect_path_with_profile(module, root, path, None)
}

/// Inspect with an explicit profile override (the engine's `--profile`
/// selection). Taint must follow the profile that produced the tree.
///
/// Hermetic seeds only; see [`inspect_path`].
pub fn inspect_path_with_profile(
    module: &Module,
    root: &Value,
    path: &str,
    override_profile: Option<&str>,
) -> Result<JsonValue, String> {
    // Only a `secret(...)` node can bind a value outside the tree; a
    // document without one needs no seed pass. When one exists, recover
    // the seeds hermetically (trusted envelope, so a document that
    // evaluated at production scale still inspects) and fail closed if
    // that is impossible: an unseeded report could return raw material.
    let has_secret = module
        .arena
        .iter()
        .any(|node| matches!(node, crate::ir::Node::Secret { .. }));
    let seeds = if has_secret {
        let mut engine = crate::eval::Engine::hermetic();
        engine.profile_override = override_profile.map(str::to_string);
        engine.budget = crate::Budget::trusted();
        engine
            .evaluate_with_seeds(module)
            .map(|(_, _, seeds)| seeds)
            .map_err(|error| {
                format!(
                    "cannot recover runtime secret seeds ({error}); \
                     use inspect_path_with_profile_env with the evaluator seeds"
                )
            })?
    } else {
        crate::secret::SecretRedactor::new()
    };
    inspect_path_with_profile_env(module, root, path, override_profile, None, Some(&seeds))
}

/// Inspect with the environment snapshot and runtime secret seeds that
/// produced the tree. A secret copied through a `let` binding or a lazy
/// host lookup has no authored literal in the profile, so the evaluator
/// seeds are required for the report to stay redacted.
pub fn inspect_path_with_profile_env(
    module: &Module,
    root: &Value,
    path: &str,
    override_profile: Option<&str>,
    env: Option<&std::collections::HashMap<String, String>>,
    seeds: Option<&crate::secret::SecretRedactor>,
) -> Result<JsonValue, String> {
    let cone = crate::convert::project_path(root, path)
        .map_err(|why| format!("cannot inspect `{path}`: {why}"))?;
    let profile = SecretProfile::with_context(module, override_profile, env, seeds);
    profile.begin_redaction_pass();
    // Runtime secrets live in the evaluated tree, not in the profile's
    // authored literals; seed both so a copy in any inspected value is
    // redacted.
    let redactor = crate::convert::redactor_with_runtime_secrets(&profile, root);
    let kind = kind_name(cone);
    // A media type sourced from material (an env lookup used as a
    // secret locator) is not publishable, even when the blob's data is
    // public; the identity is material as a whole. A path-material blob
    // (a profile selection) is material for the same reason.
    let path_material = profile.path_is_secret(path);
    let media_secret = path_material
        || matches!(
            profile.path_node(path).and_then(|id| module.get(id)),
            Some(Node::ContentAddressed { media_type, .. }) if profile.node_is_secret(*media_type)
        );
    // A value used as a secret locator (or built from one) carries no
    // `Value::Secret` marker, so taint is structural. Render with the
    // profile's path map, then sweep authored literals out of any
    // string that embedded one.
    let (mut value, marked) = render_cone(&profile, &redactor, path, cone);
    let redacted = redactor.redact(&mut value);
    let tainted =
        profile.path_is_secret(path) || contains_secret(cone) || redacted || marked || media_secret;
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
        // only named paths answer "where did this come from". A real
        // key such as `node_id` is not an internal label.
        .filter(|label| !is_internal_label(module, label))
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
        payload,
        ..
    } = cone
    {
        // A media type sourced from material (an env lookup used as a
        // secret locator) is not publishable, even though the blob's
        // data taint flag is separate. A materialised payload that
        // embeds a secret literal is material for the same reason.
        let media_literal = redactor.redact_text(media_type).is_some();
        let payload_literal = crate::convert::blob_payload_is_material(&redactor, payload.as_ref());
        let media_secret = path_material
            || media_literal
            || payload_literal
            || matches!(
                profile.path_node(path).and_then(|id| module.get(id)),
                Some(Node::ContentAddressed { media_type, .. }) if profile.node_is_secret(*media_type)
            );
        report["content_addressed"] = json!({
            "hash": format!("blake3:{}", crate::blob::hash_hex(hash)),
            "size": blob_size,
            // A tainted identity redacts its media type (spec/blob.md
            // §5/§7), matching every other export path.
            "media_type": if *blob_tainted || media_secret { MARKER } else { media_type },
            "tainted": *blob_tainted || media_secret,
        });
    }
    Ok(report)
}

/// Render a selected cone, replacing material leaves with the marker.
/// Containers keep their shape so non-secret siblings stay visible;
/// every leaf carries its own path, including array indexes, so the
/// structural taint set applies at the right granularity. The bool
/// reports whether anything was marked, so the report's `tainted` flag
/// never says false on a value that contains markers.
fn render_cone(
    profile: &SecretProfile,
    redactor: &crate::secret::SecretRedactor,
    path: &str,
    value: &Value,
) -> (JsonValue, bool) {
    match value {
        Value::Array(items) => {
            let mut marked = false;
            let rendered = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let (child, hit) =
                        render_cone(profile, redactor, &format!("{path}[{index}]"), item);
                    marked |= hit;
                    child
                })
                .collect();
            (JsonValue::Array(rendered), marked)
        }
        Value::Table(map) => {
            let mut out = serde_json::Map::new();
            let mut marked = false;
            for (key, item) in map {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let (rendered, hit) = render_cone(profile, redactor, &child, item);
                marked |= hit;
                out.insert(key.clone(), rendered);
            }
            (JsonValue::Object(out), marked)
        }
        _ if profile.path_is_secret(path) => (JsonValue::String(MARKER.to_string()), true),
        // A materialised payload can carry a secret even when the
        // blob's own taint flag is clear. The encoded bytes would not
        // match a literal sweep, so the identity is redacted as a whole
        // and the payload is never rendered.
        Value::Blob {
            hash,
            size,
            payload,
            ..
        } => {
            if crate::convert::blob_payload_is_material(redactor, payload.as_ref()) {
                (crate::convert::redacted_blob_json(hash, *size), true)
            } else {
                // Inspect reports are identity-only: a clean payload is
                // used for the taint check, never emitted.
                (
                    crate::convert::value_to_json(&crate::convert::strip_blob_payloads(value)),
                    false,
                )
            }
        }
        _ => (crate::convert::value_to_json(value), false),
    }
}

/// Arena labels are `node` followed by ASCII digits AND resolve to no
/// real path or binding. A genuine key named `node1` stays provenance.
fn is_internal_label(module: &Module, label: &str) -> bool {
    let base = label.split('.').next().unwrap_or(label);
    let base = base.split('[').next().unwrap_or(base);
    if module.resolve_value_path(base).is_some() || module.bindings.contains_key(base) {
        return false;
    }
    base.strip_prefix("node")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
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

/// Schema contract at a dotted path, when a schema covers it. Each
/// segment may carry any number of indices (`arr[0][1].key`): the key
/// selects the field, then every index descends one array level.
fn schema_for_path(module: &Module, path: &str) -> Option<JsonValue> {
    let root = module.schema?;
    let mut current = root;
    for segment in path.split('.') {
        let key_end = segment.find('[').unwrap_or(segment.len());
        let key = &segment[..key_end];
        if !key.is_empty() {
            current = match module.get(current) {
                Some(Node::SchemaTable { fields, .. }) => *fields.get(key)?,
                Some(Node::SchemaArray { element }) => *element,
                _ => return None,
            };
        }
        for _ in segment.matches('[') {
            let Some(Node::SchemaArray { element }) = module.get(current) else {
                return None;
            };
            current = *element;
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
