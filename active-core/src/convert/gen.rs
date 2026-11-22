//! Typed code generation from Active schemas (C-047,
//! spec/converters.md §7). Reads the module's schema tree and emits
//! JSON Schema, an OpenAPI fragment, TypeScript interfaces, Go
//! structs, and Rust serde types. Only static literal constraints
//! render — dynamic bounds still validate at runtime but have no
//! static form (documented, never guessed).

use crate::ir::{Module, Node, NodeId, SchemaField};
use crate::types::ActiveType;
use serde_json::{json, Value as JsonValue};
use std::collections::{HashMap, HashSet};

fn no_schema() -> String {
    "document has no schema block".to_string()
}

fn schema_root(module: &Module) -> Result<NodeId, String> {
    module.schema.ok_or_else(no_schema)
}

// ------------------------------------------------------------------
// Shared shape helpers.
// ------------------------------------------------------------------

/// Static integer bound (plain integers only; units and floats are not
/// integer bounds). A `let` binding resolves through its chain.
fn literal_int(module: &Module, id: Option<NodeId>) -> Option<i64> {
    match static_bound(module, id)? {
        StaticBound::Int(n) => Some(n),
        _ => None,
    }
}

/// Static literal float bound (plain integers widen).
fn literal_float(module: &Module, id: Option<NodeId>) -> Option<f64> {
    match static_bound(module, id)? {
        StaticBound::Int(n) => Some(n as f64),
        StaticBound::Float(f) => Some(f),
        _ => None,
    }
}

/// Static literal string bound.
pub(crate) fn literal_string(module: &Module, id: Option<NodeId>) -> Option<String> {
    super::static_string(module, id)
}

/// Field `description` slot for doc comments across codegen targets.
fn field_description(module: &Module, id: NodeId) -> Option<String> {
    match module.get(id) {
        Some(crate::ir::Node::SchemaField { field }) => literal_string(module, field.description),
        _ => None,
    }
}

/// Enum member descriptions joined as `"member: description"` pairs for
/// targets with a single description string (JSON Schema `description`).
/// Length mismatches cannot reach here: validation reports them first,
/// and codegen reads the same evaluated nodes.
fn enum_descriptions_text(module: &Module, field: &SchemaField) -> Option<String> {
    let members = match &field.typ {
        ActiveType::Enum(members) => members,
        _ => return None,
    };
    let id = field.enum_descriptions?;
    let texts: Vec<String> = match module.get(id) {
        Some(Node::Literal(crate::ir::Value::Array(items))) => items
            .iter()
            .map(|v| match v {
                crate::ir::Value::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        Some(Node::Array { elements }) => elements
            .iter()
            .map(|e| match module.get(*e) {
                Some(Node::Literal(crate::ir::Value::String(text))) => Some(text.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    if texts.len() != members.len() {
        return None;
    }
    Some(
        members
            .iter()
            .zip(texts.iter())
            .map(|(member, text)| format!("{member}: {text}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// Static literal default as JSON (any literal kind, redacted like
/// every export — secrets never leak through generated defaults).
pub(crate) fn literal_default(module: &Module, field: &SchemaField) -> Option<JsonValue> {
    super::static_default(module, field)
}

/// Required unless a static default exists or the field is Null
/// (absent Null keys validate clean — C-032). Tables and arrays are
/// always required: they carry no default slot.
fn is_required(module: &Module, id: NodeId) -> bool {
    match module.get(id) {
        Some(Node::SchemaField { field }) => {
            !matches!(field.typ, ActiveType::Null) && literal_default(module, field).is_none()
        }
        _ => true,
    }
}

/// Whether the field's presence is conditional (`requires`): a binding
/// target must mark it optional, because the trigger being disabled makes
/// absence valid (the L-P10 T004 excuse / T010 converse).
fn has_requires(module: &Module, id: NodeId) -> bool {
    matches!(
        module.get(id),
        Some(Node::SchemaField { field }) if field.requires.is_some()
    )
}

/// The binding-target optionality: absent is valid for a defaulted, Null,
/// or conditionally-present field.
fn is_optional(module: &Module, id: NodeId) -> bool {
    !is_required(module, id) || has_requires(module, id)
}

fn pascal_case(s: &str) -> String {
    let mut out = String::new();
    for part in s.split(['_', '-']) {
        let mut chars = part.chars();
        match chars.next() {
            None => out.push('_'),
            Some(first) => {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    if out.is_empty() {
        return "Type".to_string();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn sanitize_ident(s: &str, fallback: &str) -> String {
    let clean: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty()
        || clean.starts_with(|c: char| c.is_ascii_digit())
        || !clean
            .chars()
            .all(|c: char| c.is_ascii_alphanumeric() || c == '_')
    {
        return fallback.to_string();
    }
    clean
}

/// Reserve the shared Blob helper names so a user type can never
/// collide with them (`--name ActiveBlob` gets a deterministic suffix
/// instead of a duplicate declaration).
fn reserved_blob_names(has_blob: bool) -> HashSet<String> {
    let mut taken = HashSet::new();
    if has_blob {
        taken.insert("ActiveBlob".to_string());
        taken.insert("ActiveBlobValue".to_string());
    }
    taken
}

/// Codegen pipeline version (CHECKLIST C.008): bump on any output-shape
/// change in any target. Emitters stamp it into every artifact next to
/// the workspace version so agents can detect stale bindings.
/// v2: Blob identity shape (x-active-blob / ActiveBlob) on all targets.
/// v3: Blob types model the `{"$active.blob": {...}}` wire wrapper on
///     Go/Rust/Python (TypeScript already did), so real evaluated
///     documents decode.
/// v4: Blob types carry the optional `payload_b64` materialisation
///     field on every target.
pub const CODEGEN_VERSION: u32 = 4;

/// Single-line generated-file header for text targets (`prefix` is the
/// comment marker: `//` for TS/Go/Rust, `#` for Python).
fn version_header(prefix: &str) -> String {
    format!(
        "{prefix} Generated by Active {} (codegen v{}). Do not edit.\n",
        env!("CARGO_PKG_VERSION"),
        CODEGEN_VERSION
    )
}

/// L-P19: cross-field checks cannot be represented in a generated
/// validator. Emit a value-free notice (count only, no names/sources).
fn check_notice(module: &Module) -> Option<String> {
    let schema = module.schema?;
    let count = module
        .schema_checks
        .get(&schema)
        .map_or(0, |rules| rules.len());
    (count > 0).then(|| {
        format!(
            "{count} cross-field check rule(s) are not representable here; enforce with active check."
        )
    })
}

fn check_notice_line(prefix: &str, module: &Module) -> String {
    check_notice(module)
        .map(|notice| format!("{prefix} {notice}\n"))
        .unwrap_or_default()
}

/// Innermost nominal node through array wrappers: arrays are
/// transparent for naming (the element table/enum owns the name).
fn nominal_id(module: &Module, mut id: NodeId) -> NodeId {
    while let Some(Node::SchemaArray { element }) = module.get(id) {
        id = *element;
    }
    id
}

// ------------------------------------------------------------------
// JSON Schema (Draft 2020-12 compatible subset).
// ------------------------------------------------------------------

/// The schema tree as JSON Schema: objects for tables (closed →
/// `additionalProperties: false`), enums, arrays, constraints from
/// static literals, static defaults, and `deprecated` flags.
pub fn schema_to_json_schema(module: &Module) -> Result<JsonValue, String> {
    let profile = crate::secret::SecretProfile::analyze(module);
    schema_to_json_schema_with_profile(module, &profile)
}

/// JSON Schema under an analyzed profile: a caller with an environment
/// snapshot can redact schema prose that matches a host value.
pub fn schema_to_json_schema_with_profile(
    module: &Module,
    profile: &crate::secret::SecretProfile,
) -> Result<JsonValue, String> {
    let root = schema_root(module)?;
    let mut node = json_schema_node(module, profile, root);
    if let JsonValue::Object(ref mut map) = node {
        // L-P25: declare the dialect the contract is written in. Without
        // it, a draft-07 consumer silently ignores `dependentRequired`
        // (2019-09+) while the pinned validator assumes a newer default.
        map.insert(
            "$schema".to_string(),
            JsonValue::String("http://json-schema.org/draft-07/schema#".to_string()),
        );
        // The validator exempts the document metadata keys at the root of
        // a closed table (T005); a faithful contract allows them too.
        if matches!(
            module.get(root),
            Some(Node::SchemaTable { closed: true, .. })
        ) {
            if let Some(JsonValue::Object(properties)) = map.get_mut("properties") {
                properties
                    .entry("active_mode".to_string())
                    .or_insert_with(|| json!({"type": "string", "enum": ["data", "config"]}));
                properties
                    .entry("active_capabilities".to_string())
                    .or_insert_with(|| json!({"type": "array", "items": {"type": "string"}}));
            }
        }
        map.insert(
            "$comment".to_string(),
            JsonValue::String(match check_notice(module) {
                Some(notice) => format!(
                    "Generated by Active {} (codegen v{}). Do not edit.\n{notice}",
                    env!("CARGO_PKG_VERSION"),
                    CODEGEN_VERSION
                ),
                None => format!(
                    "Generated by Active {} (codegen v{}). Do not edit.",
                    env!("CARGO_PKG_VERSION"),
                    CODEGEN_VERSION
                ),
            }),
        );
    }
    Ok(node)
}

fn default_is_material(profile: &crate::secret::SecretProfile, value: &JsonValue) -> bool {
    match value {
        JsonValue::String(text) => profile.blob_identity_is_material(text),
        JsonValue::Array(items) => items.iter().any(|item| default_is_material(profile, item)),
        JsonValue::Object(map) => map.values().any(|item| default_is_material(profile, item)),
        _ => false,
    }
}

fn redact_default_value(profile: &crate::secret::SecretProfile, value: JsonValue) -> JsonValue {
    match value {
        JsonValue::String(text) => {
            JsonValue::String(profile.redact_authored_text(&text).unwrap_or(text))
        }
        JsonValue::Array(items) => JsonValue::Array(
            items
                .into_iter()
                .map(|item| redact_default_value(profile, item))
                .collect(),
        ),
        JsonValue::Object(map) => JsonValue::Object(
            map.into_iter()
                .map(|(key, item)| (key, redact_default_value(profile, item)))
                .collect(),
        ),
        other => other,
    }
}

/// The JS expression for a static default on this field's zod type. A
/// duration's wire default is the `$active.duration` wrapper; the zod
/// type is a number, so the canonical `ms` magnitude is the default.
/// `None` means the target cannot apply this default (it stays optional).
fn zod_default_expr(field: &SchemaField, value: &JsonValue) -> Option<String> {
    match &field.typ {
        ActiveType::Duration => value
            .get("$active.duration")
            .and_then(|inner| inner.get("ms"))
            .map(|ms| ms.to_string()),
        ActiveType::Bytes | ActiveType::Int | ActiveType::Float => {
            value.as_f64().map(|_| value.to_string())
        }
        ActiveType::String | ActiveType::Enum(_) => value
            .as_str()
            .map(|s| JsonValue::String(s.to_string()).to_string()),
        ActiveType::Bool => value.as_bool().map(|b| b.to_string()),
        ActiveType::Null => Some("null".to_string()),
        ActiveType::Any => Some(value.to_string()),
        ActiveType::Secret | ActiveType::Blob | ActiveType::OneOf | ActiveType::Table { .. } => {
            None
        }
    }
}

fn redact_prose(profile: &crate::secret::SecretProfile, text: String) -> String {
    profile.redact_authored_text(&text).unwrap_or(text)
}

/// Whether any schema field is a disjunction (L-P10): used by targets
/// that cannot represent one honestly and must refuse.
fn module_has_disjunction(module: &Module) -> bool {
    module.arena.iter().any(|node| {
        matches!(
            node,
            Node::SchemaField { field } if !field.one_of.is_empty()
        )
    })
}

/// How a `requires` trigger can be expressed as a JSON Schema condition.
enum TriggerCondition {
    /// A schema matching every value that leaves the trigger disabled;
    /// `None` means no value is disabled (a provided Secret/Blob).
    Enabled(Option<JsonValue>),
    /// A Null-typed trigger is never enabled (present counts as disabled).
    Never,
    /// The trigger's type has no static enabled set: annotate.
    Unknown,
}

/// The condition under which a `requires` trigger is ENABLED
/// (spec/schema-extensions.md §4: present and not false/0/""/empty/null;
/// a provided secret or blob counts as enabled). `Never` and `Unknown` are
/// surfaced so the caller annotates instead of claiming a contract it
/// cannot express.
fn trigger_condition(module: &Module, id: NodeId) -> TriggerCondition {
    let disabled: Option<JsonValue> = match module.get(id) {
        Some(Node::SchemaField { field }) => match &field.typ {
            ActiveType::Null => return TriggerCondition::Never,
            ActiveType::Secret | ActiveType::Blob => None,
            ActiveType::Bool => Some(json!({"enum": [false]})),
            ActiveType::Int | ActiveType::Bytes => Some(json!({"enum": [0]})),
            ActiveType::Float => Some(json!({"enum": [0.0]})),
            ActiveType::String | ActiveType::Enum(_) => Some(json!({"enum": [""]})),
            ActiveType::Duration => Some(json!({"enum": [{ "$active.duration": { "ms": 0 } }]})),
            ActiveType::Table { .. } => Some(json!({"enum": [{}]})),
            // L-P25: enablement is a runtime-type rule, so an Any (or
            // disjunction) trigger is disabled by any zero magnitude. A
            // zero quantity is disabled for every unit, and the unit is a
            // runtime property, so the wire shape (not a per-unit enum)
            // carries that rule. R2/F3: the enum alone missed `0%`.
            ActiveType::Any | ActiveType::OneOf => Some(json!({
                "anyOf": [
                    {"enum": [
                        false, 0, "", [], {}, null,
                        { "$active.duration": { "ms": 0 } },
                    ]},
                    {
                        "type": "object",
                        "required": ["$active.quantity"],
                        "properties": {
                            "$active.quantity": {
                                "type": "object",
                                "required": ["value"],
                                "properties": { "value": { "const": 0 } },
                            },
                        },
                    },
                ],
            })),
        },
        Some(Node::SchemaArray { .. }) => Some(json!({"enum": [[]]})),
        Some(Node::SchemaTable { .. }) => Some(json!({"enum": [{}]})),
        _ => return TriggerCondition::Unknown,
    };
    TriggerCondition::Enabled(disabled)
}

fn json_schema_node(
    module: &Module,
    profile: &crate::secret::SecretProfile,
    id: NodeId,
) -> JsonValue {
    match module.get(id) {
        Some(Node::SchemaTable { fields, closed, .. }) => {
            // A literal `requires` target that names no schema path is a
            // schema typo (T003): evaluation reports it for every document
            // that reaches this table, so the table's contract is
            // unsatisfiable. The comment names the field and target; the
            // root wrapper replaces the comment with the version notice,
            // and the engine's diagnostic carries the detail.
            for (name, field_id) in fields {
                if let Some(Node::SchemaField { field }) = module.get(*field_id) {
                    if let Some(target) = crate::convert::requires_target(module, field) {
                        if !crate::convert::requires_target_exists(module, *field_id, &target) {
                            return json!({
                                "not": {},
                                "$comment": format!(
                                    "unsatisfiable: field '{name}' requires '{target}', which names no schema path"
                                ),
                            });
                        }
                    }
                }
            }
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for (name, field) in fields {
                properties.insert(name.clone(), json_schema_node(module, profile, *field));
                // L-P25: a `requires` field's presence is conditional. The
                // unconditional `required` array would demand it even when
                // the trigger is disabled (the L-P10 T004 excuse); the
                // forward conditional below enforces the enabled case.
                let conditional = matches!(
                    module.get(*field),
                    Some(Node::SchemaField { field }) if field.requires.is_some()
                );
                if is_required(module, *field) && !conditional {
                    required.push(JsonValue::String(name.clone()));
                }
            }
            let mut obj = serde_json::Map::new();
            obj.insert("type".to_string(), JsonValue::String("object".to_string()));
            obj.insert("properties".to_string(), JsonValue::Object(properties));
            if !required.is_empty() {
                obj.insert("required".to_string(), JsonValue::Array(required));
            }
            let mut conditionals: Vec<JsonValue> = Vec::new();
            let mut annotations = serde_json::Map::new();
            let mut computed: Vec<JsonValue> = Vec::new();
            for (name, field_id) in fields {
                let Some(Node::SchemaField { field }) = module.get(*field_id) else {
                    continue;
                };
                if field.requires.is_none() {
                    continue;
                }
                let Some(target) = crate::convert::requires_target(module, field) else {
                    // A target computed at evaluation time (an env read,
                    // a dynamic interpolation) has no static spelling, so
                    // no conditional can be emitted. Annotate the field as
                    // conditional - absent is valid when the trigger is
                    // disabled - and let consumers see the limitation.
                    computed.push(JsonValue::String(name.clone()));
                    continue;
                };
                if !crate::convert::requires_target_exists(module, *field_id, &target) {
                    continue;
                }
                // A direct sibling key is expressible in this object; a
                // dotted or root-absolute target crosses objects and has
                // no static JSON Schema form. Annotate, never lie.
                let Some(target_id) = fields.get(&target).copied() else {
                    annotations.insert(name.clone(), JsonValue::String(target.clone()));
                    continue;
                };
                let enabled = match trigger_condition(module, target_id) {
                    TriggerCondition::Enabled(disabled) => match disabled {
                        Some(disabled) => json!({
                            "required": [target],
                            "properties": { target: { "not": disabled } },
                        }),
                        None => json!({"required": [target]}),
                    },
                    TriggerCondition::Never => {
                        // A Null-typed trigger is always disabled, so a
                        // present dependent field is the T010 violation.
                        conditionals.push(json!({
                            "if": {"required": [name]},
                            "then": false,
                        }));
                        continue;
                    }
                    TriggerCondition::Unknown => {
                        annotations.insert(name.clone(), JsonValue::String(target.clone()));
                        continue;
                    }
                };
                // Forward: trigger present and enabled -> a REQUIRED field
                // is present. A defaulted/Null field stays optional (the
                // generated `required` set and resilient fills agree).
                if is_required(module, *field_id) {
                    conditionals.push(json!({
                        "if": enabled.clone(),
                        "then": {"required": [name]},
                    }));
                }
                // Reverse (T010): a present field requires the trigger
                // present and enabled.
                conditionals.push(json!({
                    "if": {"required": [name]},
                    "then": enabled,
                }));
            }
            if !conditionals.is_empty() {
                obj.insert("allOf".to_string(), JsonValue::Array(conditionals));
            }
            if !annotations.is_empty() {
                obj.insert(
                    "x-active-requires".to_string(),
                    JsonValue::Object(annotations),
                );
            }
            if !computed.is_empty() {
                obj.insert(
                    "x-active-requires-computed".to_string(),
                    JsonValue::Array(computed),
                );
            }
            if *closed {
                obj.insert("additionalProperties".to_string(), JsonValue::Bool(false));
            }
            JsonValue::Object(obj)
        }
        Some(Node::SchemaArray { element }) => {
            json!({"type": "array", "items": json_schema_node(module, profile, *element)})
        }
        Some(Node::SchemaField { field }) => json_schema_field(module, profile, field),
        // A non-schema node in a schema position is not an always-true
        // contract: fail closed (the parser never produces one).
        _ => JsonValue::Bool(false),
    }
}

/// Any static bound argument as the JSON number validation compares
/// against (integers stay integers, a duration bound is its canonical
/// millisecond count). A dynamic bound has no static number; neither
/// does a value the field type cannot order. LANG-B-005.
fn literal_number(module: &Module, id: Option<NodeId>) -> Option<JsonValue> {
    match static_bound(module, id)? {
        StaticBound::Int(n) => Some(JsonValue::from(n)),
        StaticBound::Float(f) => Some(JsonValue::from(f)),
        StaticBound::DurationMs(ms) => Some(JsonValue::from(ms)),
        StaticBound::Incomparable => None,
    }
}

/// A statically known bound argument, classified by the value evaluation
/// produces for it (LANG-B-005). Whether the field's comparison can order
/// it is decided per field type below.
enum StaticBound {
    Int(i64),
    Float(f64),
    DurationMs(u64),
    /// A static value the field type cannot compare (a percent quantity,
    /// a string, a bool, ...). The engine reports T003 for every present
    /// value, so the field's contract is unsatisfiable.
    Incomparable,
}

/// The bound an argument evaluates to, when that is statically known.
/// `None` is an absent or computed bound: codegen cannot see a runtime
/// value, and the schema simply carries no constraint (a documented
/// limitation, not a value verdict). A `let` binding resolves through
/// its `Ref` chain, exactly as evaluation does.
fn static_bound(module: &Module, slot: Option<NodeId>) -> Option<StaticBound> {
    static_bound_depth(module, slot, 0)
}

fn static_bound_depth(module: &Module, slot: Option<NodeId>, depth: usize) -> Option<StaticBound> {
    // Bound a malformed reference cycle; a real binding chain is short.
    if depth > 32 {
        return None;
    }
    Some(match module.get(slot?)? {
        Node::Ref(to) => return static_bound_depth(module, Some(*to), depth + 1),
        Node::Literal(crate::ir::Value::Int(n)) => StaticBound::Int(*n),
        Node::Literal(crate::ir::Value::Float(f)) if f.is_finite() => StaticBound::Float(*f),
        Node::Literal(crate::ir::Value::Float(_)) => StaticBound::Incomparable,
        Node::Literal(crate::ir::Value::DurationMs(ms)) => StaticBound::DurationMs(*ms),
        // Evaluation canonicalizes a unit literal: time units become
        // DurationMs, byte units become Int bytes. Any other unit keeps
        // its Quantity shape, which no numeric bound accepts.
        Node::Quantity { value, unit } => {
            let raw = match module.get(*value)? {
                Node::Literal(crate::ir::Value::Int(n)) => *n as f64,
                Node::Literal(crate::ir::Value::Float(f)) => *f,
                Node::Ref(to) => match module.get(*to)? {
                    Node::Literal(crate::ir::Value::Int(n)) => *n as f64,
                    Node::Literal(crate::ir::Value::Float(f)) => *f,
                    _ => return None,
                },
                _ => return None,
            };
            if let Some(ms) = unit.normalize_time_to_ms(raw) {
                StaticBound::DurationMs(ms)
            } else if let Some(bytes) = unit.normalize_to_bytes(raw) {
                StaticBound::Int(i64::try_from(bytes).unwrap_or(i64::MAX))
            } else {
                StaticBound::Incomparable
            }
        }
        Node::Literal(_) => StaticBound::Incomparable,
        _ => return None,
    })
}

/// Which comparison family a field uses (validate.rs `check_*_bounds`).
#[derive(Clone, Copy)]
enum BoundClass {
    Int,
    Float,
    Bytes,
    Duration,
    /// `min_len`/`max_len`: integers only, and the engine clamps a
    /// negative limit to zero rather than calling it incomparable.
    Length,
    /// Blob `max_size`: a non-negative integer only; any other static
    /// bound is an invalid schema, so every document fails.
    Size,
}

/// The JSON Schema keyword value for one bound, mirroring exactly what
/// the engine compares. `Err` is a static bound the field type cannot
/// order: every present value is rejected (T003), so the whole field
/// contract is unsatisfiable.
fn comparable_bound(
    module: &Module,
    slot: Option<NodeId>,
    class: BoundClass,
) -> Result<Option<JsonValue>, ()> {
    let Some(bound) = static_bound(module, slot) else {
        return Ok(None);
    };
    match (class, bound) {
        (BoundClass::Int, StaticBound::Int(n)) => Ok(Some(JsonValue::from(n))),
        (BoundClass::Int, StaticBound::Float(f)) => Ok(Some(JsonValue::from(f))),
        (BoundClass::Float, StaticBound::Int(n)) => Ok(Some(JsonValue::from(n))),
        (BoundClass::Float, StaticBound::Float(f)) => Ok(Some(JsonValue::from(f))),
        (BoundClass::Bytes, StaticBound::Int(n)) => Ok(Some(JsonValue::from(n))),
        (BoundClass::Duration, StaticBound::DurationMs(ms)) => Ok(Some(JsonValue::from(ms))),
        // A plain non-negative integer reads as milliseconds.
        (BoundClass::Duration, StaticBound::Int(n)) => Ok(Some(JsonValue::from(n.max(0) as u64))),
        (BoundClass::Length, StaticBound::Int(n)) => Ok(Some(JsonValue::from(n.max(0) as u64))),
        (BoundClass::Size, StaticBound::Int(n)) if n >= 0 => Ok(Some(JsonValue::from(n))),
        _ => Err(()),
    }
}

/// The first static bound in the schema tree that no value of its field
/// can satisfy, with a dotted field path. Binding targets refuse such a
/// schema: a generated type that accepts values the Active schema rejects
/// is a lie, and no target can spell "no value" for every consumer.
fn unsatisfiable_bound(module: &Module, field: &SchemaField) -> Option<&'static str> {
    let check = |slot, class, name: &'static str| {
        comparable_bound(module, slot, class)
            .is_err()
            .then_some(name)
    };
    match &field.typ {
        ActiveType::Int => check(field.min, BoundClass::Int, "min")
            .or_else(|| check(field.max, BoundClass::Int, "max")),
        ActiveType::Float => check(field.min, BoundClass::Float, "min")
            .or_else(|| check(field.max, BoundClass::Float, "max")),
        ActiveType::Bytes => check(field.min, BoundClass::Bytes, "min")
            .or_else(|| check(field.max, BoundClass::Bytes, "max")),
        ActiveType::Duration => check(field.min, BoundClass::Duration, "min")
            .or_else(|| check(field.max, BoundClass::Duration, "max")),
        ActiveType::String => check(field.min_len, BoundClass::Length, "min_len")
            .or_else(|| check(field.max_len, BoundClass::Length, "max_len")),
        ActiveType::Blob => check(field.blob_max_size, BoundClass::Size, "max_size"),
        _ => None,
    }
}

fn unsatisfiable_in_tree(
    module: &Module,
    id: NodeId,
    path: &str,
    profile: &crate::secret::SecretProfile,
) -> Option<String> {
    match module.get(id)? {
        Node::SchemaTable { fields, .. } => {
            for (name, child) in fields {
                let child_path = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}.{name}")
                };
                if let Some(found) = unsatisfiable_in_tree(module, *child, &child_path, profile) {
                    return Some(found);
                }
            }
            None
        }
        Node::SchemaArray { element } => unsatisfiable_in_tree(module, *element, path, profile),
        Node::SchemaField { field } => {
            if let Some(bound) = unsatisfiable_bound(module, field) {
                return Some(format!(
                    "schema field '{path}' has an incomparable {bound} bound, which the Active schema rejects for every value"
                ));
            }
            // Static pattern/format/media constraints the engine cannot
            // satisfy are unsatisfiable too. A material value is withheld
            // by the JSON Schema target, so it is not refused here.
            if matches!(field.typ, ActiveType::String) {
                if field.pattern.is_some()
                    && literal_string(module, field.pattern).is_none()
                    && static_bound(module, field.pattern).is_some()
                {
                    return Some(format!(
                        "schema field '{path}' has a non-string pattern, which the Active schema rejects for every value"
                    ));
                }
                match literal_string(module, field.format) {
                    Some(format) if !profile.blob_identity_is_material(&format) => {
                        if format_pattern(&format).is_none() {
                            return Some(format!(
                                "schema field '{path}' names an unknown format, which the Active schema rejects for every value"
                            ));
                        }
                    }
                    None if field.format.is_some()
                        && static_bound(module, field.format).is_some() =>
                    {
                        return Some(format!(
                            "schema field '{path}' has a non-string format, which the Active schema rejects for every value"
                        ));
                    }
                    _ => {}
                }
            }
            if matches!(field.typ, ActiveType::Blob) {
                match literal_string(module, field.blob_media_type) {
                    Some(media) if !profile.blob_identity_is_material(&media) => {}
                    None if field.blob_media_type.is_some()
                        && static_bound(module, field.blob_media_type).is_some() =>
                    {
                        return Some(format!(
                            "schema field '{path}' has a non-string media_type, which the Active schema rejects for every value"
                        ));
                    }
                    _ => {}
                }
            }
            for branch in &field.one_of {
                if let Some(found) = unsatisfiable_in_tree(module, *branch, path, profile) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// Binding targets (TypeScript/Zod, Go, Rust, Python) refuse a schema
/// containing an unsatisfiable field. The JSON Schema target stays
/// available: it can express the constraint exactly (`not: {}`).
fn refuse_unsatisfiable_binding(module: &Module) -> Result<(), String> {
    let profile = crate::secret::SecretProfile::analyze(module);
    match module
        .schema
        .and_then(|root| unsatisfiable_in_tree(module, root, "", &profile))
    {
        Some(reason) => Err(format!(
            "{reason}; no binding target can express a field with no valid value (use the JSON Schema target, which renders it as unsatisfiable)"
        )),
        None => Ok(()),
    }
}

/// A field whose static constraint the engine cannot satisfy with any
/// value it could hold: the generated contract must reject every present
/// value, exactly as evaluation does. `not: {}` is the draft-07 spelling
/// of an unsatisfiable schema; the comment names the cause.
fn unsatisfiable_field(field: &SchemaField, cause: &str) -> JsonValue {
    json!({
        "not": {},
        "$comment": format!(
            "unsatisfiable: {cause} for {} fields (the Active schema rejects every present value)",
            field.typ.name()
        ),
    })
}

/// Add prose to a generated `description` instead of overwriting it.
/// Enum member docs, the field description, and the deprecation reason
/// are all information a consumer needs; the last writer used to win
/// (LANG-B-017).
fn append_description(obj: &mut serde_json::Map<String, JsonValue>, text: String) {
    if text.is_empty() {
        return;
    }
    match obj.get_mut("description") {
        Some(JsonValue::String(existing)) if !existing.is_empty() => {
            existing.push_str(". ");
            existing.push_str(&text);
        }
        _ => {
            obj.insert("description".to_string(), JsonValue::String(text));
        }
    }
}

/// The regex Active enforces for a named `format` (validate.rs). Emitting
/// it as `pattern` makes the contract enforce the format even where a
/// consumer (or the pinned validator) treats `format` as annotation-only.
fn format_pattern(format: &str) -> Option<&'static str> {
    match format {
        "email" => Some(r"^[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)*$"),
        "uri" => Some(r"^[a-zA-Z][a-zA-Z0-9+.-]*:[^\s]+$"),
        "uuid" => {
            Some(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
        }
        "semver" => Some(
            r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$",
        ),
        _ => None,
    }
}

fn json_schema_field(
    module: &Module,
    profile: &crate::secret::SecretProfile,
    field: &SchemaField,
) -> JsonValue {
    let mut obj = serde_json::Map::new();
    match &field.typ {
        ActiveType::Null => {
            obj.insert("type".to_string(), JsonValue::String("null".to_string()));
        }
        ActiveType::Bool => {
            obj.insert("type".to_string(), JsonValue::String("boolean".to_string()));
        }
        ActiveType::Int => {
            let min = comparable_bound(module, field.min, BoundClass::Int);
            let max = comparable_bound(module, field.max, BoundClass::Int);
            if min.is_err() {
                return unsatisfiable_field(field, "incomparable min bound");
            }
            if max.is_err() {
                return unsatisfiable_field(field, "incomparable max bound");
            }
            obj.insert("type".to_string(), JsonValue::String("integer".to_string()));
            // A bound spelled as a float (min = 1.0) is still a bound;
            // `literal_int` alone dropped it (LANG-B-005).
            if let Ok(Some(min)) = min {
                obj.insert("minimum".to_string(), min);
            }
            if let Ok(Some(max)) = max {
                obj.insert("maximum".to_string(), max);
            }
        }
        ActiveType::Float => {
            let min = comparable_bound(module, field.min, BoundClass::Float);
            let max = comparable_bound(module, field.max, BoundClass::Float);
            if min.is_err() {
                return unsatisfiable_field(field, "incomparable min bound");
            }
            if max.is_err() {
                return unsatisfiable_field(field, "incomparable max bound");
            }
            obj.insert("type".to_string(), JsonValue::String("number".to_string()));
            if let Ok(Some(min)) = min {
                obj.insert("minimum".to_string(), min);
            }
            if let Ok(Some(max)) = max {
                obj.insert("maximum".to_string(), max);
            }
        }
        ActiveType::String => {
            let min = comparable_bound(module, field.min_len, BoundClass::Length);
            let max = comparable_bound(module, field.max_len, BoundClass::Length);
            if min.is_err() {
                return unsatisfiable_field(field, "incomparable min_len bound");
            }
            if max.is_err() {
                return unsatisfiable_field(field, "incomparable max_len bound");
            }
            obj.insert("type".to_string(), JsonValue::String("string".to_string()));
            if let Ok(Some(min)) = min {
                obj.insert("minLength".to_string(), min);
            }
            if let Ok(Some(max)) = max {
                obj.insert("maxLength".to_string(), max);
            }
            let pattern = literal_string(module, field.pattern);
            if let Some(pattern) = &pattern {
                // A material pattern cannot be redacted into valid regex
                // syntax; withhold the constraint entirely.
                if !profile.blob_identity_is_material(pattern) {
                    obj.insert(
                        "pattern".to_string(),
                        JsonValue::String(redact_prose(profile, pattern.clone())),
                    );
                }
            } else if field.pattern.is_some() && static_bound(module, field.pattern).is_some() {
                // A static non-string pattern is an invalid schema: every
                // present value is T003.
                return unsatisfiable_field(field, "incomparable pattern bound");
            }
            let format = literal_string(module, field.format);
            if let Some(format) = &format {
                // A format is a closed vocabulary; withhold a material
                // value instead of emitting a marker.
                if !profile.blob_identity_is_material(format) {
                    match format_pattern(format) {
                        Some(pattern) => {
                            // Keep an authored pattern too: AND the two.
                            if let Some(existing) = obj.remove("pattern") {
                                obj.insert(
                                    "allOf".to_string(),
                                    json!([
                                        { "pattern": existing },
                                        { "pattern": pattern },
                                    ]),
                                );
                            } else {
                                obj.insert(
                                    "pattern".to_string(),
                                    JsonValue::String(pattern.to_string()),
                                );
                            }
                        }
                        // The format vocabulary is closed (validate.rs):
                        // a name outside it makes every present string a
                        // T003, so the field is unsatisfiable.
                        None => return unsatisfiable_field(field, "unknown format name"),
                    }
                    obj.insert("format".to_string(), JsonValue::String(format.clone()));
                }
            } else if field.format.is_some() && static_bound(module, field.format).is_some() {
                return unsatisfiable_field(field, "incomparable format bound");
            }
        }
        // Matches the §1 export shape: durations serialize as
        // `{"$active.duration": {"ms": n}}` (LANG-F-001/F-002). The schema
        // describes that wrapper, so a validator accepts exactly the wire
        // form the exporter produces - the bare `{"ms": n}` shape is also a
        // plain table and is no longer the duration contract.
        ActiveType::Duration => {
            let min = comparable_bound(module, field.min, BoundClass::Duration);
            let max = comparable_bound(module, field.max, BoundClass::Duration);
            if min.is_err() {
                return unsatisfiable_field(field, "incomparable min bound");
            }
            if max.is_err() {
                return unsatisfiable_field(field, "incomparable max bound");
            }
            // Bounds live on the exported `ms` integer, in canonical
            // milliseconds (LANG-B-005): a bare object accepted
            // durations the Active schema rejects.
            let mut ms = serde_json::Map::new();
            ms.insert("type".to_string(), JsonValue::String("integer".to_string()));
            // A duration magnitude is a non-negative integer on the wire
            // (LANG-B-001/B-005), so `minimum: 0` is part of the contract
            // even when the schema states no lower bound. Without it a
            // negative `ms` validated as a well-formed duration.
            let floor = min.ok().flatten().unwrap_or_else(|| JsonValue::from(0u64));
            ms.insert("minimum".to_string(), floor);
            if let Ok(Some(max)) = max {
                ms.insert("maximum".to_string(), max);
            }
            let inner = json!({
                "type": "object",
                "properties": { "ms": JsonValue::Object(ms) },
                "required": ["ms"],
                "additionalProperties": false,
            });
            obj.insert("type".to_string(), JsonValue::String("object".to_string()));
            obj.insert(
                "properties".to_string(),
                json!({ "$active.duration": inner }),
            );
            obj.insert("required".to_string(), json!(["$active.duration"]));
            obj.insert("additionalProperties".to_string(), JsonValue::Bool(false));
        }
        // Bytes normalize to integers on export.
        ActiveType::Bytes => {
            let min = comparable_bound(module, field.min, BoundClass::Bytes);
            let max = comparable_bound(module, field.max, BoundClass::Bytes);
            if min.is_err() {
                return unsatisfiable_field(field, "incomparable min bound");
            }
            if max.is_err() {
                return unsatisfiable_field(field, "incomparable max bound");
            }
            obj.insert("type".to_string(), JsonValue::String("integer".to_string()));
            if let Ok(Some(min)) = min {
                obj.insert("minimum".to_string(), min);
            }
            if let Ok(Some(max)) = max {
                obj.insert("maximum".to_string(), max);
            }
        }
        ActiveType::Table { closed } => {
            obj.insert("type".to_string(), JsonValue::String("object".to_string()));
            if *closed {
                obj.insert("additionalProperties".to_string(), JsonValue::Bool(false));
            }
        }
        ActiveType::Enum(members) => {
            // An enum is a string with a closed member list. Omitting
            // `type` made the generated schema typeless, so importing it
            // back lost the whole Enum type (LANG-B-030).
            obj.insert("type".to_string(), JsonValue::String("string".to_string()));
            // A material member cannot be redacted into a valid enum
            // contract; withhold the member list and keep `string`.
            if !members
                .iter()
                .any(|member| profile.blob_identity_is_material(member))
            {
                obj.insert(
                    "enum".to_string(),
                    JsonValue::Array(members.iter().cloned().map(JsonValue::String).collect()),
                );
            }
            // Member descriptions join the field description instead
            // of replacing it (LANG-B-017).
            if let Some(joined) = enum_descriptions_text(module, field) {
                append_description(&mut obj, redact_prose(profile, joined));
            }
        }
        // Secrets export as redacted strings.
        ActiveType::Secret => {
            obj.insert("type".to_string(), JsonValue::String("string".to_string()));
            obj.insert(
                "description".to_string(),
                JsonValue::String("secret value, redacted as ***SECRET*** on export".to_string()),
            );
        }
        // Blobs export as the wire wrapper `{"$active.blob": {…}}`
        // (spec/blob.md §5), so the generated schema models it exactly:
        // the inner identity is closed, and the materialised payload is
        // the only optional key.
        ActiveType::Blob => {
            let max = comparable_bound(module, field.blob_max_size, BoundClass::Size);
            if max.is_err() {
                return unsatisfiable_field(field, "invalid max_size bound");
            }
            // A static non-string media expectation is an invalid schema:
            // validation reports it and every document fails.
            let media = literal_string(module, field.blob_media_type);
            if field.blob_media_type.is_some()
                && media.is_none()
                && static_bound(module, field.blob_media_type).is_some()
            {
                return unsatisfiable_field(field, "invalid media_type bound");
            }
            let mut inner = serde_json::Map::new();
            inner.insert("type".to_string(), JsonValue::String("object".to_string()));
            let mut inner_properties = serde_json::json!({
                "hash": { "type": "string", "pattern": "^blake3:[0-9a-f]{64}$" },
                "size": { "type": "integer", "minimum": 0 },
                "media_type": { "type": "string" },
                "tainted": { "type": "boolean" },
                "payload_b64": { "type": "string" },
            });
            // L-P25: the blob's declared bounds are part of the contract;
            // x-active-* alone is an annotation a validator ignores.
            if let Ok(Some(max)) = &max {
                inner_properties["size"]["maximum"] = max.clone();
            }
            if let Some(media) = &media {
                if !profile.blob_identity_is_material(media) {
                    // A tainted identity redacts its media type on the
                    // wire to the shared marker (spec/blob.md §5), so a
                    // document exported from a valid config can carry
                    // either spelling. Pinning `const` to the authored
                    // type rejected the engine's own output (F2).
                    inner_properties["media_type"]["enum"] = json!([media, crate::secret::MARKER]);
                }
            }
            inner.insert("properties".to_string(), inner_properties);
            inner.insert(
                "required".to_string(),
                serde_json::json!(["hash", "size", "media_type", "tainted"]),
            );
            inner.insert("additionalProperties".to_string(), JsonValue::Bool(false));
            obj.insert("type".to_string(), JsonValue::String("object".to_string()));
            obj.insert(
                "properties".to_string(),
                serde_json::json!({ "$active.blob": JsonValue::Object(inner) }),
            );
            obj.insert("required".to_string(), serde_json::json!(["$active.blob"]));
            obj.insert("additionalProperties".to_string(), JsonValue::Bool(false));
            obj.insert("x-active-blob".to_string(), JsonValue::Bool(true));
            if let Some(media) = media {
                // Withhold a material media type: a marker would be an
                // invalid contract value.
                if !profile.blob_identity_is_material(&media) {
                    obj.insert("x-active-media-type".to_string(), JsonValue::String(media));
                }
            }
            if let Ok(Some(max)) = max {
                obj.insert("x-active-max-size".to_string(), max);
            }
        }
        ActiveType::OneOf => {
            // A disjunction (L-P10): no `type` at this level; each
            // branch renders as a full schema node, so per-branch bounds,
            // patterns, and nested tables survive into the contract.
            // The parser refuses a degenerate disjunction, so the empty
            // case is defensive only (an always-true schema, never an
            // empty `oneOf` that rejects everything).
            if field.one_of.is_empty() {
                // An empty disjunction accepts nothing; the parser refuses
                // it (E020), so this is defensive and fail-closed.
                return JsonValue::Bool(false);
            }
            let branches: Vec<JsonValue> = field
                .one_of
                .iter()
                .map(|branch| json_schema_node(module, profile, *branch))
                .collect();
            // Active accepts a value when ANY branch accepts it (L-P10);
            // `oneOf` would demand exactly one, rejecting an Int value from
            // an Int|Float disjunction. `anyOf` is the faithful keyword.
            obj.insert("anyOf".to_string(), JsonValue::Array(branches));
        }
        // Any imposes no type keyword, so the object stays empty and the
        // shared tail still emits its default/description/deprecated
        // annotations (returning `true` here dropped all three).
        ActiveType::Any => {}
    }
    if let Some(default) = literal_default(module, field) {
        // A material default is not a usable contract value; omit it
        // rather than emit a marker a consumer would fill.
        if !default_is_material(profile, &default) {
            obj.insert(
                "default".to_string(),
                redact_default_value(profile, default),
            );
        }
    }
    if let Some(doc) = literal_string(module, field.description) {
        append_description(&mut obj, redact_prose(profile, doc));
    }
    if let Some(reason) = field.deprecated.as_deref() {
        obj.insert("deprecated".to_string(), JsonValue::Bool(true));
        if !reason.is_empty() {
            append_description(
                &mut obj,
                format!("deprecated: {}", redact_prose(profile, reason.to_string())),
            );
        }
    }
    JsonValue::Object(obj)
}

// ------------------------------------------------------------------
// OpenAPI fragment (3.1: plain JSON Schema under components).
// ------------------------------------------------------------------

/// The JSON Schema wrapped as an OpenAPI 3.1
/// `components.schemas.<name>` fragment.
pub fn schema_to_openapi(module: &Module, name: &str) -> Result<JsonValue, String> {
    let profile = crate::secret::SecretProfile::analyze(module);
    schema_to_openapi_with_profile(module, name, &profile)
}

/// OpenAPI fragment under an analyzed profile.
pub fn schema_to_openapi_with_profile(
    module: &Module,
    name: &str,
    profile: &crate::secret::SecretProfile,
) -> Result<JsonValue, String> {
    let schema = schema_to_json_schema_with_profile(module, profile)?;
    let mut schemas = serde_json::Map::new();
    schemas.insert(sanitize_ident(name, "Config"), schema);
    let mut components = serde_json::Map::new();
    components.insert("schemas".to_string(), JsonValue::Object(schemas));
    let mut out = serde_json::Map::new();
    out.insert(
        "openapi".to_string(),
        JsonValue::String("3.1.0".to_string()),
    );
    out.insert("components".to_string(), JsonValue::Object(components));
    Ok(JsonValue::Object(out))
}

// ------------------------------------------------------------------
// TypeScript interfaces.
// ------------------------------------------------------------------

/// The schema tree as TypeScript `export interface` declarations plus
/// zod runtime schemas (`<Name>Schema`) mirroring every interface with
/// the same constraints (min/max, patterns, formats, enums, defaults).
/// Nested tables become PascalCase interfaces; `?`/`.optional()` marks
/// defaulted/Null fields.
pub fn schema_to_typescript(module: &Module, root_name: &str) -> Result<String, String> {
    let profile = crate::secret::SecretProfile::analyze(module);
    schema_to_typescript_with_profile(module, root_name, &profile)
}

/// TypeScript under an analyzed profile.
pub fn schema_to_typescript_with_profile(
    module: &Module,
    root_name: &str,
    profile: &crate::secret::SecretProfile,
) -> Result<String, String> {
    typescript_with_profile(module, root_name, profile, TsBinding::None)
}

/// Pulumi binding (L-P18): the TypeScript interfaces + zod schema plus a
/// native adapter that reads the Active-exported document from a stack's
/// configuration (`pulumi.Config.getObject`) and validates it.
pub fn schema_to_pulumi(module: &Module, root_name: &str) -> Result<String, String> {
    let profile = crate::secret::SecretProfile::analyze(module);
    schema_to_pulumi_with_profile(module, root_name, &profile)
}

pub fn schema_to_pulumi_with_profile(
    module: &Module,
    root_name: &str,
    profile: &crate::secret::SecretProfile,
) -> Result<String, String> {
    typescript_with_profile(module, root_name, profile, TsBinding::Pulumi)
}

/// CDK binding (L-P18): the TypeScript interfaces + zod schema plus an
/// adapter that reads the Active-exported document from a construct's
/// context (`node.tryGetContext`) and validates it.
pub fn schema_to_cdk(module: &Module, root_name: &str) -> Result<String, String> {
    let profile = crate::secret::SecretProfile::analyze(module);
    schema_to_cdk_with_profile(module, root_name, &profile)
}

pub fn schema_to_cdk_with_profile(
    module: &Module,
    root_name: &str,
    profile: &crate::secret::SecretProfile,
) -> Result<String, String> {
    typescript_with_profile(module, root_name, profile, TsBinding::Cdk)
}

/// Which tool-specific adapter the TypeScript output carries after the
/// interfaces and the zod schema. The adapter is a consumer binding, not
/// an authored-config surface (the rejected L-P18 direction).
#[derive(Clone, Copy, PartialEq, Eq)]
enum TsBinding {
    None,
    Pulumi,
    Cdk,
}

fn typescript_with_profile(
    module: &Module,
    root_name: &str,
    profile: &crate::secret::SecretProfile,
    binding: TsBinding,
) -> Result<String, String> {
    refuse_unsatisfiable_binding(module)?;
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for code generation".to_string());
    }
    let mut gen = TsGen {
        module,
        profile,
        out: format!(
            "{}{}import {{ z }} from \"zod\";\n\n",
            version_header("//"),
            check_notice_line("//", module)
        ),
        names: HashMap::new(),
        taken: HashSet::new(),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    gen.emit_table(&root_name, root);
    gen.out
        .push_str("\n// Runtime validation schemas (zod) mirroring the interfaces above.\n");
    gen.emit_zod_table(&root_name, root);
    append_binding_adapter(&mut gen.out, &root_name, binding);
    Ok(gen.out)
}

fn append_binding_adapter(out: &mut String, root: &str, binding: TsBinding) {
    match binding {
        TsBinding::None => {}
        TsBinding::Pulumi => out.push_str(&format!(
            "\n// Pulumi binding: Active is the source of truth. The exported document\n\
             // lives under the \"active\" object key; the zod schema above validates it\n\
             // before the program sees it. This reads config; it does not author it.\n\
             export interface PulumiConfigLike {{\n  getObject(key: string): unknown;\n}}\n\n\
             export function readPulumiConfig(config: PulumiConfigLike): {root} {{\n  \
             // zod's inferred output can mark required keys optional (its typing\n  \
             // differs across major versions); parse() enforces them at runtime,\n  \
             // so the assertion states the actual post-parse shape.\n  \
             return {root}Schema.parse(config.getObject(\"active\")) as {root};\n}}\n"
        )),
        TsBinding::Cdk => out.push_str(&format!(
            "\n// CDK binding: Active is the source of truth. The exported document lives\n\
             // under the \"active\" context key; the zod schema above validates it before\n\
             // the construct sees it. This reads context; it does not author it.\n\
             export interface CdkContextLike {{\n  tryGetContext(key: string): unknown;\n}}\n\n\
             export function readCdkContext(context: CdkContextLike): {root} {{\n  \
             // zod's inferred output can mark required keys optional (its typing\n  \
             // differs across major versions); parse() enforces them at runtime,\n  \
             // so the assertion states the actual post-parse shape.\n  \
             return {root}Schema.parse(context.tryGetContext(\"active\")) as {root};\n}}\n"
        )),
    }
}

struct TsGen<'a> {
    module: &'a Module,
    profile: &'a crate::secret::SecretProfile,
    out: String,
    /// Type name per node plus every assigned name: same-derived
    /// names for different nodes (`port` vs `Port` tables) gain
    /// suffixes deterministically instead of colliding.
    names: HashMap<NodeId, String>,
    taken: HashSet<String>,
}

impl TsGen<'_> {
    /// Canonical type name for a node under a derived base: stable
    /// across the reference and declaration passes, which compute
    /// the same base independently.
    fn type_name(&mut self, id: NodeId, base: String) -> String {
        if let Some(name) = self.names.get(&id) {
            return name.clone();
        }
        let mut name = base;
        while !self.taken.insert(name.clone()) {
            name.push('_');
        }
        self.names.insert(id, name.clone());
        name
    }

    fn emit_table(&mut self, name: &str, id: NodeId) {
        let Some(Node::SchemaTable { fields, .. }) = self.module.get(id) else {
            return;
        };
        let fields: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.out.push_str(&format!("export interface {name} {{\n"));
        for (key, field) in &fields {
            let optional = is_optional(self.module, *field);
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            let typ = self.ts_type(&tname, *field);
            let mark = if optional { "?" } else { "" };
            if let Some(doc) = field_description(self.module, *field)
                .map(|doc| self.profile.redact_authored_text(&doc).unwrap_or(doc))
            {
                self.out
                    .push_str(&format!("  /** {} */\n", ts_comment(&doc)));
            }
            if let Some(reason) = self.deprecated(*field) {
                if reason.is_empty() {
                    self.out.push_str("  /** @deprecated */\n");
                } else {
                    self.out
                        .push_str(&format!("  /** @deprecated {} */\n", ts_comment(&reason)));
                }
            }
            self.out.push_str(&format!("  {key}{mark}: {typ};\n"));
        }
        self.out.push_str("}\n");
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            self.emit_nested(&tname, *field);
        }
    }

    fn emit_nested(&mut self, name: &str, id: NodeId) {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => self.emit_table(name, id),
            Some(Node::SchemaArray { element }) => self.emit_nested(name, *element),
            _ => {}
        }
    }

    fn ts_type(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            // Named by the caller: emit_table/emit_nested declares
            // the same hint, so references always resolve.
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("{}[]", self.ts_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.ts_field(field),
            _ => "unknown".to_string(),
        }
    }

    /// Inline type for array elements, threaded with the same naming
    /// hint the declaration pass uses — element tables resolve to
    /// the named interface, never to anonymous duplicates.
    fn ts_inline(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("{}[]", self.ts_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.ts_field(field),
            _ => "unknown".to_string(),
        }
    }

    fn ts_field(&self, field: &SchemaField) -> String {
        match &field.typ {
            ActiveType::Null => "null".to_string(),
            ActiveType::Bool => "boolean".to_string(),
            ActiveType::OneOf => {
                if field.one_of.is_empty() {
                    return "unknown".to_string();
                }
                let branches: Vec<String> = field
                    .one_of
                    .iter()
                    .map(|branch| self.ts_node(*branch))
                    .collect();
                branches.join(" | ")
            }
            ActiveType::Int | ActiveType::Float | ActiveType::Duration | ActiveType::Bytes => {
                "number".to_string()
            }
            ActiveType::String | ActiveType::Secret => "string".to_string(),
            ActiveType::Blob => "{ \"$active.blob\": { hash: string; size: number; media_type: string; tainted: boolean; payload_b64?: string } }".to_string(),
            ActiveType::Table { .. } => "Record<string, unknown>".to_string(),
            ActiveType::Enum(members) => {
                // Keep the interface valid when a member is material.
                if members
                    .iter()
                    .any(|member| self.profile.blob_identity_is_material(member))
                {
                    "string".to_string()
                } else {
                    members
                        .iter()
                        .map(|m| format!("{m:?}"))
                        .collect::<Vec<_>>()
                        .join(" | ")
                }
            }
            ActiveType::Any => "unknown".to_string(),
        }
    }

    /// One schema node as a TypeScript type, for disjunction branches.
    fn ts_node(&self, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => self.ts_field(field),
            Some(Node::SchemaArray { element }) => {
                format!("Array<{}>", self.ts_node(*element))
            }
            Some(Node::SchemaTable { .. }) => "Record<string, unknown>".to_string(),
            _ => "unknown".to_string(),
        }
    }

    fn deprecated(&self, id: NodeId) -> Option<String> {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => field
                .deprecated
                .clone()
                .map(|reason| self.profile.redact_authored_text(&reason).unwrap_or(reason)),
            _ => None,
        }
    }

    /// Zod schema const for one table, children first so every
    /// referenced `<Name>Schema` const already exists at module load
    /// (no temporal-dead-zone reads). Names reuse the interface memo,
    /// so refs always resolve to the declared interfaces' schemas.
    /// Closed tables emit `.strict()` so extra keys are rejected exactly
    /// like `additionalProperties: false` in JSON Schema and Active.
    fn emit_zod_table(&mut self, name: &str, id: NodeId) {
        let Some(Node::SchemaTable { fields, closed, .. }) = self.module.get(id) else {
            return;
        };
        let fields: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            match self.module.get(*field) {
                Some(Node::SchemaTable { .. }) => self.emit_zod_table(&tname, *field),
                Some(Node::SchemaArray { element }) => {
                    self.emit_zod_nested(&tname, *element);
                }
                _ => {}
            }
        }
        self.out
            .push_str(&format!("export const {name}Schema = z.object({{\n"));
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            let expr = self.zod_type(&tname, *field);
            self.out.push_str(&format!("  {key}: {expr},\n"));
        }
        if *closed {
            self.out.push_str("}).strict();\n");
        } else {
            self.out.push_str("});\n");
        }
    }

    fn emit_zod_nested(&mut self, hint: &str, id: NodeId) {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => self.emit_zod_table(hint, id),
            Some(Node::SchemaArray { element }) => self.emit_zod_nested(hint, *element),
            _ => {}
        }
    }

    /// Zod expression for a field node: table refs, arrays, or a leaf
    /// with the same static checks the interfaces document.
    fn zod_type(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => {
                format!("{}Schema", self.type_name(id, hint.to_string()))
            }
            Some(Node::SchemaArray { element }) => {
                format!("z.array({})", self.zod_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => {
                let required = is_required(self.module, id) && !has_requires(self.module, id);
                self.zod_leaf(field, required)
            }
            _ => "z.unknown()".to_string(),
        }
    }

    fn zod_inline(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => {
                format!("{}Schema", self.type_name(id, hint.to_string()))
            }
            Some(Node::SchemaArray { element }) => {
                format!("z.array({})", self.zod_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.zod_leaf(field, true),
            _ => "z.unknown()".to_string(),
        }
    }

    /// Leaf zod expression with static checks. `required` is false for
    /// defaulted/Null fields (`.optional()`/`.default()`); array
    /// elements always pass true (element position has no optionality).
    /// One schema node as a zod expression, for disjunction branches.
    fn zod_node(&self, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => self.zod_leaf(field, true),
            Some(Node::SchemaArray { element }) => {
                format!("z.array({})", self.zod_node(*element))
            }
            Some(Node::SchemaTable { .. }) => "z.record(z.string(), z.unknown())".to_string(),
            _ => "z.unknown()".to_string(),
        }
    }

    fn zod_leaf(&self, field: &SchemaField, required: bool) -> String {
        let mut out = match &field.typ {
            ActiveType::Null => "z.null()".to_string(),
            ActiveType::Bool => "z.boolean()".to_string(),
            ActiveType::Int => "z.number().int()".to_string(),
            ActiveType::Float => "z.number()".to_string(),
            ActiveType::Duration | ActiveType::Bytes => "z.number().int()".to_string(),
            ActiveType::Secret => "z.string()".to_string(),
            ActiveType::Blob => {
                // The wire wrapper (spec/blob.md §5). Carry what a zod
                // schema can enforce: the size bound and the accepted
                // media spellings (the authored type and the tainted
                // marker). A struct tag cannot reach inside a Go wrapper,
                // which is why the Go target documents these as
                // JSON-Schema-only.
                let mut size = "z.number().int().min(0)".to_string();
                if let Some(max) = literal_int(self.module, field.blob_max_size) {
                    if max >= 0 {
                        size.push_str(&format!(".max({max})"));
                    }
                }
                let media = match literal_string(self.module, field.blob_media_type) {
                    Some(media) if !self.profile.blob_identity_is_material(&media) => {
                        format!("z.enum([{media:?}, {:?}])", crate::secret::MARKER)
                    }
                    _ => "z.string()".to_string(),
                };
                format!(
                    "z.object({{ \"$active.blob\": z.object({{ hash: z.string(), size: {size}, media_type: {media}, tainted: z.boolean(), payload_b64: z.string().optional() }}) }})"
                )
            }
            ActiveType::Table { .. } => "z.record(z.string(), z.unknown())".to_string(),
            ActiveType::OneOf => {
                if field.one_of.is_empty() {
                    return "z.unknown()".to_string();
                }
                let branches: Vec<String> = field
                    .one_of
                    .iter()
                    .map(|branch| self.zod_node(*branch))
                    .collect();
                format!("z.union([{}])", branches.join(", "))
            }
            ActiveType::Any => "z.unknown()".to_string(),
            ActiveType::Enum(members) => {
                // A material member cannot be redacted into a valid
                // enum; fall back to an unconstrained string.
                if members
                    .iter()
                    .any(|member| self.profile.blob_identity_is_material(member))
                {
                    "z.string()".to_string()
                } else {
                    let items: Vec<String> = members
                        .iter()
                        .map(|m| JsonValue::String(m.clone()).to_string())
                        .collect();
                    format!("z.enum([{}])", items.join(", "))
                }
            }
            ActiveType::String => {
                let mut expr = "z.string()".to_string();
                if let Some(min) = literal_int(self.module, field.min_len) {
                    expr.push_str(&format!(".min({min})"));
                }
                if let Some(max) = literal_int(self.module, field.max_len) {
                    expr.push_str(&format!(".max({max})"));
                }
                if let Some(pattern) = literal_string(self.module, field.pattern) {
                    // Redacting a pattern would emit an invalid regex;
                    // drop the constraint when it is material.
                    if !self.profile.blob_identity_is_material(&pattern) {
                        let quoted = JsonValue::String(pattern).to_string();
                        expr.push_str(&format!(".regex(new RegExp({quoted}))"));
                    }
                }
                // Use Active's own anchored regex for every named format.
                // zod's `.email()`/`.url()`/`.uuid()` are close to, but
                // narrower than, Active's definitions (its email TLD rule
                // rejects `a@b.c`, which Active accepts), so a binding
                // built on them rejects valid documents. One regex source
                // (the same table the JSON Schema pattern uses) keeps the
                // contract exact.
                if let Some(name) = literal_string(self.module, field.format) {
                    if let Some(pattern) = format_pattern(&name) {
                        let quoted = JsonValue::String(pattern.to_string()).to_string();
                        expr.push_str(&format!(".regex(new RegExp({quoted}))"));
                    }
                }
                expr
            }
        };
        match &field.typ {
            ActiveType::Int => {
                if let Some(min) = literal_int(self.module, field.min) {
                    out.push_str(&format!(".min({min})"));
                }
                if let Some(max) = literal_int(self.module, field.max) {
                    out.push_str(&format!(".max({max})"));
                }
            }
            ActiveType::Float => {
                if let Some(min) = literal_float(self.module, field.min) {
                    out.push_str(&format!(".min({min})"));
                }
                if let Some(max) = literal_float(self.module, field.max) {
                    out.push_str(&format!(".max({max})"));
                }
            }
            // Durations and bytes are exported as integer magnitudes
            // (canonical ms / bytes), so their bounds belong on the zod
            // number too - the JSON Schema emitter already carries them
            // (LANG-B-005; the TS target used to drop them).
            ActiveType::Duration | ActiveType::Bytes => {
                if let Some(min) = literal_number(self.module, field.min) {
                    out.push_str(&format!(".min({min})"));
                }
                if let Some(max) = literal_number(self.module, field.max) {
                    out.push_str(&format!(".max({max})"));
                }
            }
            _ => {}
        }
        if !required {
            if let Some(default) = literal_default(self.module, field) {
                if default_is_material(self.profile, &default) {
                    // A material default would emit a marker into the
                    // runtime schema; keep the field optional instead.
                    out.push_str(".optional()");
                } else {
                    let default = redact_default_value(self.profile, default);
                    match zod_default_expr(field, &default) {
                        Some(expr) => out.push_str(&format!(".default({expr})")),
                        None => out.push_str(".optional()"),
                    }
                }
            } else {
                out.push_str(".optional()");
            }
        }
        out
    }
}

// ------------------------------------------------------------------
// Go structs.
// ------------------------------------------------------------------

/// The schema tree as Go structs with `json` tags plus
/// go-playground/validator `validate` tags (required, min/max, email,
/// url, uuid, oneof; enforced by that library, not by this file).
/// Nested tables become PascalCase structs; enums stay strings with
/// their members documented.
pub fn schema_to_go(module: &Module, root_name: &str) -> Result<String, String> {
    let profile = crate::secret::SecretProfile::analyze(module);
    schema_to_go_with_profile(module, root_name, &profile)
}

/// Go structs under an analyzed profile.
pub fn schema_to_go_with_profile(
    module: &Module,
    root_name: &str,
    profile: &crate::secret::SecretProfile,
) -> Result<String, String> {
    refuse_unsatisfiable_binding(module)?;
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for code generation".to_string());
    }
    let has_blob = module.arena.iter().any(|node| {
        matches!(
            node,
            Node::SchemaField {
                field: SchemaField {
                    typ: ActiveType::Blob,
                    ..
                },
            }
        )
    });
    // Go has no union type: a schema disjunction cannot be represented
    // without inventing an interface convention the consumer does not
    // share. Refuse instead of emitting a lossy `interface{}` contract
    // (L-P10). JSON Schema, OpenAPI, TypeScript, and Zod carry it.
    if module_has_disjunction(module) {
        return Err(
            "the Go target cannot represent a schema disjunction (OneOf); generate JSON Schema \
             or TypeScript instead"
                .to_string(),
        );
    }
    let mut gen = GoGen {
        module,
        profile,
        out: format!(
            "{}{}// Validation tags follow go-playground/validator conventions.\n",
            version_header("//"),
            check_notice_line("//", module)
        ),
        names: HashMap::new(),
        taken: reserved_blob_names(has_blob),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    gen.emit_table(&root_name, root);
    if has_blob {
        // Shared wire shape for Blob fields (spec/blob.md §5): the
        // document carries `{"$active.blob": {hash, size, media_type,
        // tainted}}`, so the generated type models that wrapper rather
        // than the inner object (which would never decode real output).
        // Payloads never cross codegen. Aligned through the same run
        // formatter as tables so the output stays gofmt-clean (the
        // equivalence gate checks).
        gen.out.push_str("\ntype ActiveBlob struct {\n");
        let mut run = vec![(
            "ActiveBlobValue".to_string(),
            "ActiveBlobValue".to_string(),
            "`json:\"$active.blob\"`".to_string(),
        )];
        flush_go_run(&mut run, &mut gen.out);
        gen.out.push_str("}\n\ntype ActiveBlobValue struct {\n");
        let mut run = vec![
            (
                "Hash".to_string(),
                "string".to_string(),
                "`json:\"hash\"`".to_string(),
            ),
            (
                "Size".to_string(),
                "uint64".to_string(),
                "`json:\"size\"`".to_string(),
            ),
            (
                "MediaType".to_string(),
                "string".to_string(),
                "`json:\"media_type\"`".to_string(),
            ),
            (
                "Tainted".to_string(),
                "bool".to_string(),
                "`json:\"tainted\"`".to_string(),
            ),
            (
                // Present only on an explicitly materialised,
                // untainted blob (spec/blob.md §5).
                "PayloadB64".to_string(),
                "string".to_string(),
                "`json:\"payload_b64,omitempty\"`".to_string(),
            ),
        ];
        flush_go_run(&mut run, &mut gen.out);
        gen.out.push_str("}\n");
    }
    Ok(gen.out)
}

/// Emit one contiguous run of struct fields with gofmt-style column
/// alignment: comment lines break runs, so each run aligns names and
/// types independently, exactly as gofmt does. Widths use byte length,
/// which equals display width here: schema keys lex as ASCII-only
/// identifiers (lexer.rs), so names and derived type names never
/// contain multi-byte characters.
fn flush_go_run(run: &mut Vec<(String, String, String)>, out: &mut String) {
    if run.is_empty() {
        return;
    }
    let nw = run.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0);
    let tw = run.iter().map(|(_, t, _)| t.len()).max().unwrap_or(0);
    for (n, t, tag) in run.drain(..) {
        out.push_str(&format!("\t{n:<nw$} {t:<tw$} {tag}\n", nw = nw, tw = tw));
    }
}

struct GoGen<'a> {
    module: &'a Module,
    profile: &'a crate::secret::SecretProfile,
    out: String,
    names: HashMap<NodeId, String>,
    taken: HashSet<String>,
}

impl GoGen<'_> {
    fn type_name(&mut self, id: NodeId, base: String) -> String {
        if let Some(name) = self.names.get(&id) {
            return name.clone();
        }
        let mut name = base;
        while !self.taken.insert(name.clone()) {
            name.push('_');
        }
        self.names.insert(id, name.clone());
        name
    }

    fn emit_table(&mut self, name: &str, id: NodeId) {
        let Some(Node::SchemaTable { fields, .. }) = self.module.get(id) else {
            return;
        };
        let fields: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.out.push_str(&format!("type {name} struct {{\n"));
        let mut taken = HashSet::new();
        // Collect the body first so gofmt-style column alignment can be
        // computed per contiguous run of fields (comment lines break runs,
        // exactly as gofmt does).
        enum BodyLine {
            Comment(String),
            Field {
                name: String,
                typ: String,
                tag: String,
            },
        }
        let mut lines: Vec<BodyLine> = Vec::new();
        for (key, field) in &fields {
            let mut field_name = pascal_case(key);
            while !taken.insert(field_name.clone()) {
                field_name.push('_');
            }
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            let typ = self.go_type(&tname, *field);
            if let Some(doc) = field_description(self.module, *field)
                .map(|doc| self.profile.redact_authored_text(&doc).unwrap_or(doc))
            {
                lines.push(BodyLine::Comment(doc));
            }
            if let Some(reason) = self.deprecated(*field) {
                if reason.is_empty() {
                    lines.push(BodyLine::Comment(
                        "Deprecated: no reason given.".to_string(),
                    ));
                } else {
                    lines.push(BodyLine::Comment(format!("Deprecated: {reason}")));
                }
            }
            for note in self.doc(*field) {
                lines.push(BodyLine::Comment(note));
            }
            lines.push(BodyLine::Field {
                name: field_name,
                typ,
                tag: self.go_tag(key, *field),
            });
        }
        let mut run: Vec<(String, String, String)> = Vec::new();
        for line in lines {
            match line {
                BodyLine::Comment(c) => {
                    flush_go_run(&mut run, &mut self.out);
                    self.out.push_str(&format!("\t// {}\n", comment_line(&c)));
                }
                BodyLine::Field { name, typ, tag } => run.push((name, typ, tag)),
            }
        }
        flush_go_run(&mut run, &mut self.out);
        self.out.push_str("}\n");
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            self.emit_nested(&tname, *field);
        }
    }

    fn emit_nested(&mut self, name: &str, id: NodeId) {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => self.emit_table(name, id),
            Some(Node::SchemaArray { element }) => self.emit_nested(name, *element),
            _ => {}
        }
    }

    fn go_type(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("[]{}", self.go_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.go_field(field),
            _ => "interface{}".to_string(),
        }
    }

    fn go_inline(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            // Element tables resolve to the named struct the
            // declaration pass emits under the same hint.
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("[]{}", self.go_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.go_field(field),
            _ => "interface{}".to_string(),
        }
    }

    /// Struct tag with `json` name plus go-playground/validator rules
    /// derived from static constraints. `pattern` has no validator
    /// equivalent and is documented on the field comment instead;
    /// `requires`/`deprecated` are schema-level concerns, not field tags.
    /// Divergence note: validator `required` means non-zero, while Active
    /// required means merely present — so `Bool` fields never carry
    /// `required` (an explicit `false` is a legitimate present value).
    /// The same non-zero reading applies to `0`, `""`, and `0.0` on other
    /// types: presence is authoritative in Active and JSON Schema, and the
    /// tags are a best-effort hint for the validator library.
    fn go_tag(&self, key: &str, id: NodeId) -> String {
        let mut rules: Vec<String> = Vec::new();
        if is_required(self.module, id) && !has_requires(self.module, id) {
            let is_bool = matches!(
                self.module.get(id),
                Some(Node::SchemaField { field }) if matches!(field.typ, ActiveType::Bool)
            );
            if !is_bool {
                rules.push("required".to_string());
            }
        }
        if let Some(Node::SchemaField { field }) = self.module.get(id) {
            match &field.typ {
                ActiveType::Int => {
                    if let Some(min) = literal_int(self.module, field.min) {
                        rules.push(format!("min={min}"));
                    }
                    if let Some(max) = literal_int(self.module, field.max) {
                        rules.push(format!("max={max}"));
                    }
                }
                ActiveType::Float => {
                    if let Some(min) = literal_float(self.module, field.min) {
                        rules.push(format!("min={min}"));
                    }
                    if let Some(max) = literal_float(self.module, field.max) {
                        rules.push(format!("max={max}"));
                    }
                }
                ActiveType::String => {
                    if let Some(min) = literal_int(self.module, field.min_len) {
                        rules.push(format!("min={min}"));
                    }
                    if let Some(max) = literal_int(self.module, field.max_len) {
                        rules.push(format!("max={max}"));
                    }
                    match literal_string(self.module, field.format).as_deref() {
                        Some("email") => rules.push("email".to_string()),
                        Some("uri") => rules.push("url".to_string()),
                        Some("uuid") => rules.push("uuid".to_string()),
                        _ => {}
                    }
                }
                ActiveType::Duration | ActiveType::Bytes => {
                    // Canonical milliseconds / bytes (LANG-B-005): the
                    // same integer the Engine compares, so unit bounds
                    // and binding-spelled bounds reach the validator tag.
                    if let Some(min) = literal_int(self.module, field.min) {
                        rules.push(format!("min={min}"));
                    }
                    if let Some(max) = literal_int(self.module, field.max) {
                        rules.push(format!("max={max}"));
                    }
                }
                // Spaces, commas, or struct-tag delimiters in a member
                // would make oneof ambiguous or truncate the tag; the
                // `// one of:` comment still documents them, and the
                // JSON Schema target stays exact.
                ActiveType::Enum(members)
                    if !members.iter().any(|member| {
                        member.contains([' ', ',', '`', '"', '\\'])
                            || self.profile.blob_identity_is_material(member)
                    }) =>
                {
                    rules.push(format!("oneof={}", members.join(" ")));
                }
                _ => {}
            }
        }
        if rules.is_empty() {
            format!("`json:\"{key}\"`")
        } else {
            format!("`json:\"{key}\" validate:\"{}\"`", rules.join(","))
        }
    }

    fn go_field(&self, field: &SchemaField) -> String {
        match &field.typ {
            ActiveType::Null | ActiveType::Any => "interface{}".to_string(),
            ActiveType::Bool => "bool".to_string(),
            ActiveType::Int | ActiveType::Duration | ActiveType::Bytes => "int64".to_string(),
            ActiveType::Float => "float64".to_string(),
            ActiveType::String | ActiveType::Secret | ActiveType::Enum(_) => "string".to_string(),
            ActiveType::Blob => "ActiveBlob".to_string(),
            ActiveType::Table { .. } => "map[string]interface{}".to_string(),
            // Unreachable: `schema_to_go` refuses a disjunction before
            // generation (Go has no union type).
            ActiveType::OneOf => "interface{}".to_string(),
        }
    }

    /// Field documentation notes: type-specific remarks plus the
    /// static facets with no validator-tag equivalent (`format`,
    /// `pattern`, `requires`) so nothing is silently dropped.
    fn doc(&self, id: NodeId) -> Vec<String> {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => {
                let mut notes = Vec::new();
                match &field.typ {
                    ActiveType::Duration => notes.push("milliseconds".to_string()),
                    ActiveType::Enum(members) => {
                        let joined = members
                            .iter()
                            .map(|member| {
                                self.profile
                                    .redact_authored_text(member)
                                    .unwrap_or_else(|| member.clone())
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        notes.push(format!("one of: {joined}"))
                    }
                    ActiveType::Secret => {
                        notes.push("secret value, redacted as ***SECRET*** on export".to_string())
                    }
                    ActiveType::Blob => {
                        // Validator tags cannot reach inside the wrapper
                        // struct; the JSON Schema and zod targets enforce
                        // these, so say so instead of implying parity.
                        if let Some(max) = literal_int(self.module, field.blob_max_size) {
                            notes.push(format!(
                                "blob max_size {max} (enforced by JSON Schema/zod, not this tag)"
                            ));
                        }
                    }
                    _ => {}
                }
                if let Some(format) = literal_string(self.module, field.format) {
                    let format = self.profile.redact_authored_text(&format).unwrap_or(format);
                    notes.push(format!("format: {format}"));
                }
                if let Some(pattern) = literal_string(self.module, field.pattern) {
                    let pattern = self
                        .profile
                        .redact_authored_text(&pattern)
                        .unwrap_or(pattern);
                    notes.push(format!("pattern {pattern}"));
                }
                if let Some(target) = literal_string(self.module, field.requires) {
                    // A real field reference stays raw; dangling text
                    // is data.
                    let target = if crate::convert::schema_path_exists(self.module, &target) {
                        target
                    } else {
                        self.profile.redact_authored_text(&target).unwrap_or(target)
                    };
                    notes.push(format!("requires: {target}"));
                }
                notes
            }
            _ => Vec::new(),
        }
    }

    fn deprecated(&self, id: NodeId) -> Option<String> {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => field
                .deprecated
                .clone()
                .map(|reason| self.profile.redact_authored_text(&reason).unwrap_or(reason)),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------
// Rust serde types.
// ------------------------------------------------------------------

const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
];

/// The schema tree as Rust serde types: structs with `rename` tags,
/// real enums for `Enum`, `Option` for defaulted/Null fields.
pub fn schema_to_rust(module: &Module, root_name: &str) -> Result<String, String> {
    let profile = crate::secret::SecretProfile::analyze(module);
    schema_to_rust_with_profile(module, root_name, &profile)
}

/// Rust types under an analyzed profile.
pub fn schema_to_rust_with_profile(
    module: &Module,
    root_name: &str,
    profile: &crate::secret::SecretProfile,
) -> Result<String, String> {
    refuse_unsatisfiable_binding(module)?;
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for code generation".to_string());
    }
    let has_blob = module.arena.iter().any(|node| {
        matches!(
            node,
            Node::SchemaField {
                field: SchemaField {
                    typ: ActiveType::Blob,
                    ..
                },
            }
        )
    });
    // Rust targets generate named structs, not enums per disjunction;
    // an untagged union would need a generated enum plus serde
    // annotations that the current renderer does not model. Refuse
    // rather than emit a field typed `serde_json::Value` (L-P10).
    if module_has_disjunction(module) {
        return Err(
            "the Rust target cannot represent a schema disjunction (OneOf); generate JSON \
             Schema or TypeScript instead"
                .to_string(),
        );
    }
    let mut gen = RustGen {
        module,
        profile,
        out: format!(
            "{}{}\
            // Requires: serde = {{ version = \"1\", features = [\"derive\"] }}, serde_json = \"1\"\nuse serde::{{Deserialize, Serialize}};\n\n",
            version_header("//"),
            check_notice_line("//", module)
        ),
        names: HashMap::new(),
        taken: reserved_blob_names(has_blob),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    gen.emit_table(&root_name, root);
    if has_blob {
        // Wire shape for Blob fields (spec/blob.md §5): evaluated
        // documents carry `{"$active.blob": {...}}`, so the struct
        // renames its single field to that key. The inner identity
        // type stays payload-free.
        gen.out.push_str(
            "\n#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct ActiveBlob {\n    #[serde(rename = \"$active.blob\")]\n    pub active_blob: ActiveBlobValue,\n}\n\n#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct ActiveBlobValue {\n    pub hash: String,\n    pub size: u64,\n    pub media_type: String,\n    pub tainted: bool,\n    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n    pub payload_b64: Option<String>,\n}\n",
        );
    }
    Ok(gen.out)
}

struct RustGen<'a> {
    module: &'a Module,
    profile: &'a crate::secret::SecretProfile,
    out: String,
    names: HashMap<NodeId, String>,
    taken: HashSet<String>,
}

impl RustGen<'_> {
    fn type_name(&mut self, id: NodeId, base: String) -> String {
        if let Some(name) = self.names.get(&id) {
            return name.clone();
        }
        let mut name = base;
        while !self.taken.insert(name.clone()) {
            name.push('_');
        }
        self.names.insert(id, name.clone());
        name
    }

    fn emit_table(&mut self, name: &str, id: NodeId) {
        let Some(Node::SchemaTable { fields, .. }) = self.module.get(id) else {
            return;
        };
        let fields: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.out
            .push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct ");
        self.out.push_str(name);
        self.out.push_str(" {\n");
        let mut taken = HashSet::new();
        for (key, field) in &fields {
            let optional = is_optional(self.module, *field);
            let raw = sanitize_ident(key, "field");
            let (field_name, mut renamed) = if RUST_KEYWORDS.contains(&raw.as_str()) {
                (format!("r#{raw}"), true)
            } else {
                (raw.clone(), raw != *key)
            };
            let mut field_name = field_name;
            while !taken.insert(field_name.clone()) {
                field_name.push('_');
                renamed = true;
            }
            let inner = {
                let nominal = nominal_id(self.module, *field);
                let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
                self.rust_type(&tname, *field)
            };
            let typ = if optional {
                format!("Option<{inner}>")
            } else {
                inner
            };
            if let Some(reason) = self.deprecated(*field) {
                if reason.is_empty() {
                    self.out.push_str("\t#[deprecated]\n");
                } else {
                    self.out
                        .push_str(&format!("\t#[deprecated(note = {reason:?})]\n"));
                }
            }
            // Raw identifiers (`r#type`) never match their key
            // textually; serde_derive strips the prefix, so an
            // explicit rename keeps the mapping exact either way.
            if renamed {
                self.out
                    .push_str(&format!("\t#[serde(rename = {key:?})]\n"));
            }
            self.out.push_str(&format!("\tpub {field_name}: {typ},\n"));
        }
        self.out.push_str("}\n");
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            self.emit_nested(&tname, *field);
        }
    }

    fn emit_nested(&mut self, name: &str, id: NodeId) {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => self.emit_table(name, id),
            Some(Node::SchemaArray { element }) => self.emit_nested(name, *element),
            Some(Node::SchemaField { field }) => {
                if let ActiveType::Enum(members) = &field.typ {
                    // A material member cannot be redacted into a valid
                    // wire value, and a `***SECRET***` variant would
                    // accept the marker instead of the real document.
                    // Degrade the field to `String` on every target.
                    if members
                        .iter()
                        .any(|member| self.profile.blob_identity_is_material(member))
                    {
                        self.out.push_str(&format!("pub type {name} = String;\n"));
                    } else {
                        self.emit_enum(name, members);
                    }
                }
            }
            _ => {}
        }
    }

    fn emit_enum(&mut self, name: &str, members: &[String]) {
        self.out
            .push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\npub enum ");
        self.out.push_str(name);
        self.out.push_str(" {\n");
        let mut taken = HashSet::new();
        for member in members {
            // A material member cannot be emitted as a wire value;
            // derive the variant and the rename from the redacted text.
            let member = self
                .profile
                .redact_authored_text(member)
                .unwrap_or_else(|| member.clone());
            let mut variant = pascal_case(&sanitize_ident(&member, "Value"));
            while !taken.insert(variant.clone()) {
                variant.push('_');
            }
            if variant != member {
                self.out
                    .push_str(&format!("\t#[serde(rename = {member:?})]\n"));
            }
            self.out.push_str(&format!("\t{variant},\n"));
        }
        self.out.push_str("}\n");
    }

    fn rust_type(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("Vec<{}>", self.rust_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => match &field.typ {
                ActiveType::Null => "()".to_string(),
                ActiveType::Bool => "bool".to_string(),
                ActiveType::Int => "i64".to_string(),
                ActiveType::Float => "f64".to_string(),
                ActiveType::String | ActiveType::Secret => "String".to_string(),
                // Shared identity shape (spec/blob.md §5), emitted once
                // below as `ActiveBlob`: hash, size, media type, taint.
                ActiveType::Blob => "ActiveBlob".to_string(),
                ActiveType::Duration | ActiveType::Bytes => "u64".to_string(),
                ActiveType::Table { .. } => {
                    "std::collections::HashMap<String, serde_json::Value>".to_string()
                }
                ActiveType::Enum(_) => hint.to_string(),
                ActiveType::Any => "serde_json::Value".to_string(),
                // Unreachable: `schema_to_rust` refuses a disjunction
                // before generation (no generated untagged enum yet).
                ActiveType::OneOf => "serde_json::Value".to_string(),
            },
            _ => "serde_json::Value".to_string(),
        }
    }

    /// Element types under the declaration hint: element tables and
    /// enums resolve to the named items `emit_nested` declares.
    fn rust_inline(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("Vec<{}>", self.rust_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => match &field.typ {
                ActiveType::Enum(_) => hint.to_string(),
                _ => self.rust_type(hint, id),
            },
            _ => "serde_json::Value".to_string(),
        }
    }

    fn deprecated(&self, id: NodeId) -> Option<String> {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => field
                .deprecated
                .clone()
                .map(|reason| self.profile.redact_authored_text(&reason).unwrap_or(reason)),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------
// Python TypedDicts (stdlib typing only; floor Python 3.11 for
// Required/NotRequired). Descriptions render as comments, formats
// and defaults as comments, requires as a comment: the types carry
// shape, Active carries verdicts.
// ----------------------------------------------------------------

/// The schema tree as Python `TypedDict` classes: nested tables become
/// nested classes, enums become `Literal`, optional fields use
/// `NotRequired`. Descriptions, formats, defaults, and `requires`
/// render as comments above each field.
pub fn schema_to_python(module: &Module, root_name: &str) -> Result<String, String> {
    let profile = crate::secret::SecretProfile::analyze(module);
    schema_to_python_with_profile(module, root_name, &profile)
}

/// Python types under an analyzed profile.
pub fn schema_to_python_with_profile(
    module: &Module,
    root_name: &str,
    profile: &crate::secret::SecretProfile,
) -> Result<String, String> {
    refuse_unsatisfiable_binding(module)?;
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for code generation".to_string());
    }
    let has_blob = module.arena.iter().any(|node| {
        matches!(
            node,
            Node::SchemaField {
                field: SchemaField {
                    typ: ActiveType::Blob,
                    ..
                },
            }
        )
    });
    let mut gen = PythonGen {
        module,
        profile,
        out: format!(
            "{}{}from typing import {}\n\n\n",
            version_header("#"),
            check_notice_line("#", module),
            if module_has_disjunction(module) {
                "Any, Literal, NotRequired, TypedDict, Union"
            } else {
                "Any, Literal, NotRequired, TypedDict"
            }
        ),
        names: HashMap::new(),
        taken: reserved_blob_names(has_blob),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    // Wire shape for Blob fields (spec/blob.md §5): class syntax cannot
    // spell `$active.blob`, so both TypedDicts use the functional form.
    // The wrapper comes FIRST: annotations resolve at class creation.
    if has_blob {
        gen.out.push_str(
            "ActiveBlobValue = TypedDict(\"ActiveBlobValue\", {\"hash\": str, \"size\": int, \"media_type\": str, \"tainted\": bool, \"payload_b64\": NotRequired[str]})\nActiveBlob = TypedDict(\"ActiveBlob\", {\"$active.blob\": ActiveBlobValue})\n\n\n",
        );
    }
    gen.emit_table(&root_name, root);
    Ok(gen.out)
}

struct PythonGen<'a> {
    module: &'a Module,
    profile: &'a crate::secret::SecretProfile,
    out: String,
    names: HashMap<NodeId, String>,
    taken: HashSet<String>,
}

/// Python string literal with escaping (members and defaults come from
/// schema authors, not a fixed dictionary: quotes, backslashes, and
/// control characters must not break the emitted module).
fn py_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Python keywords plus builtins that must not become bare field names.
const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield", "match", "case", "type",
];

/// Comment-safe single line for `//` and `#` comments: a newline in a
/// description would otherwise inject an uncommented line into the output.
fn comment_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Comment-safe text for `/** */` blocks: folded to one line with the
/// block terminator neutralized, so `*/` in a description cannot close
/// the comment early.
fn ts_comment(s: &str) -> String {
    comment_line(s).replace("*/", "* /")
}

impl PythonGen<'_> {
    fn type_name(&mut self, id: NodeId, base: String) -> String {
        if let Some(name) = self.names.get(&id) {
            return name.clone();
        }
        let mut name = base;
        while !self.taken.insert(name.clone()) {
            name.push('_');
        }
        self.names.insert(id, name.clone());
        name
    }

    fn emit_table(&mut self, name: &str, id: NodeId) {
        let Some(Node::SchemaTable { fields, .. }) = self.module.get(id) else {
            return;
        };
        let fields: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.out.push_str(&format!("class {name}(TypedDict):\n"));
        if fields.is_empty() {
            self.out.push_str("    pass\n");
        }
        let mut taken_names = std::collections::HashSet::new();
        for (key, field) in &fields {
            let mut field_name = sanitize_ident(&key.to_lowercase(), "field");
            if PYTHON_KEYWORDS.contains(&field_name.as_str()) {
                field_name.push('_');
            }
            while !taken_names.insert(field_name.clone()) {
                field_name.push('_');
            }
            if field_name != *key {
                self.out
                    .push_str(&format!("    # key: {}\n", comment_line(key)));
            }
            if let Some(doc) = field_description(self.module, *field)
                .map(|doc| self.profile.redact_authored_text(&doc).unwrap_or(doc))
            {
                self.out
                    .push_str(&format!("    # {}\n", comment_line(&doc)));
            }
            if let Some(crate::ir::Node::SchemaField { field: f }) = self.module.get(*field) {
                if let Some(format) = literal_string(self.module, f.format) {
                    let format = self.profile.redact_authored_text(&format).unwrap_or(format);
                    self.out
                        .push_str(&format!("    # format: {}\n", comment_line(&format)));
                }
                if let Some(pattern) = literal_string(self.module, f.pattern) {
                    let pattern = self
                        .profile
                        .redact_authored_text(&pattern)
                        .unwrap_or(pattern);
                    self.out
                        .push_str(&format!("    # pattern: {}\n", comment_line(&pattern)));
                }
                if let Some(default) = literal_default(self.module, f) {
                    self.out.push_str(&format!(
                        "    # default: {}\n",
                        comment_line(
                            &self
                                .profile
                                .redact_authored_text(&default.to_string())
                                .unwrap_or_else(|| default.to_string()),
                        )
                    ));
                }
                if let Some(target) = literal_string(self.module, f.requires) {
                    let target = if crate::convert::schema_path_exists(self.module, &target) {
                        target
                    } else {
                        self.profile.redact_authored_text(&target).unwrap_or(target)
                    };
                    self.out
                        .push_str(&format!("    # requires: {}\n", comment_line(&target)));
                }
                if let Some(reason) = f.deprecated.clone() {
                    if reason.is_empty() {
                        self.out.push_str("    # Deprecated\n");
                    } else {
                        let reason = self.profile.redact_authored_text(&reason).unwrap_or(reason);
                        self.out
                            .push_str(&format!("    # Deprecated: {}\n", comment_line(&reason)));
                    }
                }
            }
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            let mut typ = self.py_type(&tname, *field);
            if is_optional(self.module, *field) {
                typ = format!("NotRequired[{typ}]");
            }
            self.out.push_str(&format!("    {field_name}: {typ}\n"));
        }
        self.out.push('\n');
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            self.emit_nested(&tname, *field);
        }
    }

    fn emit_nested(&mut self, name: &str, id: NodeId) {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => self.emit_table(name, id),
            Some(Node::SchemaArray { element }) => self.emit_nested(name, *element),
            _ => {}
        }
    }

    fn py_type(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => format!("\"{hint}\""),
            Some(Node::SchemaArray { element }) => {
                format!("list[{}]", self.py_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.py_field(field),
            _ => "Any".to_string(),
        }
    }

    fn py_inline(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => format!("\"{hint}\""),
            Some(Node::SchemaArray { element }) => {
                format!("list[{}]", self.py_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.py_field(field),
            _ => "Any".to_string(),
        }
    }

    /// One schema node as a Python type, for disjunction branches.
    fn py_node(&self, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => self.py_field(field),
            Some(Node::SchemaArray { element }) => {
                format!("list[{}]", self.py_node(*element))
            }
            Some(Node::SchemaTable { .. }) => "dict[str, Any]".to_string(),
            _ => "Any".to_string(),
        }
    }

    fn py_field(&self, field: &SchemaField) -> String {
        match &field.typ {
            ActiveType::Null => "None".to_string(),
            ActiveType::Bool => "bool".to_string(),
            ActiveType::Int | ActiveType::Duration | ActiveType::Bytes => "int".to_string(),
            ActiveType::Float => "float".to_string(),
            ActiveType::String | ActiveType::Secret => "str".to_string(),
            // Shared identity shape (spec/blob.md §5), emitted once
            // below as an `ActiveBlob` TypedDict.
            ActiveType::Blob => "ActiveBlob".to_string(),
            ActiveType::Enum(members) => {
                // A material member cannot be redacted into a valid
                // Literal; fall back to an unconstrained string.
                if members
                    .iter()
                    .any(|member| self.profile.blob_identity_is_material(member))
                {
                    "str".to_string()
                } else {
                    let quoted: Vec<String> = members.iter().map(|m| py_string(m)).collect();
                    format!("Literal[{}]", quoted.join(", "))
                }
            }
            ActiveType::Table { .. } => "dict[str, Any]".to_string(),
            ActiveType::OneOf => {
                if field.one_of.is_empty() {
                    return "Any".to_string();
                }
                let branches: Vec<String> = field
                    .one_of
                    .iter()
                    .map(|branch| self.py_node(*branch))
                    .collect();
                format!("Union[{}]", branches.join(", "))
            }
            ActiveType::Any => "Any".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Terraform (HCL) bindings (L-P18)
//
// One variable per root field. The HCL type system cannot preserve extra
// object attributes (verified against Terraform 1.9.8: object conversion
// discards them), so a structured table is refused rather than silently
// truncated. Scalars, scalar arrays, bare/empty tables, bounds,
// descriptions, enums, formats, defaults, and sensitivity are represented
// exactly; everything else is a hard error naming the field.
// ---------------------------------------------------------------------------

/// Terraform `variables.tf` from the schema.
pub fn schema_to_terraform(module: &Module, root_name: &str) -> Result<String, String> {
    let profile = crate::secret::SecretProfile::analyze(module);
    schema_to_terraform_with_profile(module, root_name, &profile)
}

pub fn schema_to_terraform_with_profile(
    module: &Module,
    root_name: &str,
    profile: &crate::secret::SecretProfile,
) -> Result<String, String> {
    refuse_unsatisfiable_binding(module)?;
    let root = schema_root(module)?;
    let Some(Node::SchemaTable { fields, closed, .. }) = module.get(root) else {
        return Err("schema block must be a table for code generation".to_string());
    };
    let mut out = version_header("#");
    out.push_str(&check_notice_line("#", module));
    out.push_str(&format!(
        "# Terraform variables for the Active schema `{}`. Regenerate from the\n# schema; the schema is the source of truth.\n# Duration values are canonical milliseconds; byte values are canonical bytes.\n",
        comment_line(&sanitize_ident(root_name, "Config"))
    ));
    if *closed {
        out.push_str("# The root table is closed in Active, but Terraform does not reject\n# undeclared variables in a tfvars file; extra root keys are not enforceable here.\n");
    }
    out.push('\n');
    for (name, id) in fields {
        tf_variable(&mut out, module, profile, name, *id, name)?;
    }
    Ok(out)
}

fn hcl_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    // A hyphen is subtraction in HCL (`var.my-var` reads as an
    // expression), so only letters, digits, and underscore.
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// HCL quoted-string escaping. HCL supports `\\ \" \n \r \t` and `\uNNNN`
/// and treats `${`/`%{` as template interpolation/directives, so those
/// sequences are doubled (`$${`/`%%{`), which HCL renders as the literal
/// text. Other control characters become `\u00NN`, never a raw byte.
fn hcl_string(text: &str) -> String {
    let mut out = String::from("\"");
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            // Double the sigil only when it starts a template sequence;
            // `$${` and `%%{` are HCL's literal escapes.
            '$' if chars.peek() == Some(&'{') => out.push_str("$$"),
            '%' if chars.peek() == Some(&'{') => out.push_str("%%"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Canonical attribute alignment inside an HCL block (terraform fmt).
fn tf_attrs(out: &mut String, indent: &str, attrs: &[(&str, String)]) {
    let width = attrs.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    for (name, value) in attrs {
        out.push_str(indent);
        out.push_str(name);
        out.push_str(&" ".repeat(width - name.len()));
        out.push_str(" = ");
        out.push_str(value);
        out.push('\n');
    }
}

fn tf_variable(
    out: &mut String,
    module: &Module,
    profile: &crate::secret::SecretProfile,
    name: &str,
    id: NodeId,
    path: &str,
) -> Result<(), String> {
    if !hcl_ident(name) {
        return Err(format!(
            "field '{path}' is not a valid Terraform variable name (letters, digits, and '_', not starting with a digit)"
        ));
    }
    if tf_requires_any(module, id) {
        return Err(format!(
            "field '{path}' uses `requires`; Terraform variables cannot express conditional presence (validation sees one variable at a time)"
        ));
    }
    let typ = tf_type(module, id, path)?;
    let mut attrs: Vec<(&str, String)> = vec![("type", typ)];
    let field = match module.get(id) {
        Some(Node::SchemaField { field }) => Some(field),
        _ => None,
    };
    if let Some(field) = field {
        if let Some(doc) = field_description(module, id)
            .map(|doc| profile.redact_authored_text(&doc).unwrap_or(doc))
        {
            attrs.push(("description", hcl_string(&doc)));
        }
        if let Some(reason) = &field.deprecated {
            let reason = profile
                .redact_authored_text(reason)
                .unwrap_or_else(|| reason.clone());
            out.push_str(&format!("# Deprecated: {}\n", comment_line(&reason)));
        }
        if let Some(default) = tf_default(module, profile, id, path)? {
            attrs.push(("default", default));
        } else if matches!(field.typ, ActiveType::Null) {
            // A Null-typed field is optional in Active (absent validates
            // clean); a Terraform variable without a default is required.
            attrs.push(("default", "null".to_string()));
        }
    }
    let nullable = matches!(
        module.get(id),
        Some(Node::SchemaField { field })
            if matches!(field.typ, ActiveType::Null | ActiveType::Any)
    );
    attrs.push(("nullable", nullable.to_string()));
    if tf_contains_secret(module, id) {
        attrs.push(("sensitive", "true".to_string()));
    }
    out.push_str(&format!("variable \"{name}\" {{\n"));
    tf_attrs(out, "  ", &attrs);
    for (condition, message) in tf_conditions(module, profile, id, &format!("var.{name}"), path)? {
        out.push_str("\n  validation {\n");
        tf_attrs(
            out,
            "    ",
            &[
                ("condition", condition),
                ("error_message", hcl_string(&message)),
            ],
        );
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
    Ok(())
}

fn tf_type(module: &Module, id: NodeId, path: &str) -> Result<String, String> {
    match module.get(id) {
        Some(Node::SchemaTable { fields, .. }) => {
            if fields.is_empty() {
                // An empty table is any table; a closed empty table is an
                // empty map (validated separately).
                Ok("map(any)".to_string())
            } else {
                Err(format!(
                    "field '{path}' is a structured table; Terraform object conversion discards attributes outside the type, so the binding would silently drop keys (close and flatten, or use JSON Schema)"
                ))
            }
        }
        Some(Node::SchemaArray { element }) => {
            let element_type = tf_element_type(module, *element, path)?;
            Ok(format!("list({element_type})"))
        }
        Some(Node::SchemaField { field }) => match &field.typ {
            ActiveType::Null | ActiveType::Any => Ok("any".to_string()),
            ActiveType::Bool => Ok("bool".to_string()),
            ActiveType::Int | ActiveType::Float | ActiveType::Duration | ActiveType::Bytes => {
                Ok("number".to_string())
            }
            ActiveType::String | ActiveType::Secret | ActiveType::Enum(_) => {
                Ok("string".to_string())
            }
            ActiveType::Blob => Err(format!(
                "field '{path}' is a Blob; Terraform has no representation for the wire identity wrapper"
            )),
            ActiveType::Table { .. } => Ok("map(any)".to_string()),
            ActiveType::OneOf => Err(format!(
                "field '{path}' uses OneOf; Terraform variables cannot express a union"
            )),
        },
        _ => Err(format!("field '{path}' is not a schema node")),
    }
}

fn tf_element_type(module: &Module, id: NodeId, path: &str) -> Result<String, String> {
    match module.get(id) {
        Some(Node::SchemaTable { fields, .. }) if fields.is_empty() => Ok("map(any)".to_string()),
        Some(Node::SchemaTable { .. }) => Err(format!(
            "field '{path}' is an array of structured tables; Terraform object conversion drops extra attributes and cannot validate element fields"
        )),
        Some(Node::SchemaArray { .. }) => Err(format!(
            "field '{path}' is a nested array; Terraform element constraints cannot be expressed faithfully here"
        )),
        Some(Node::SchemaField { field }) => match &field.typ {
            ActiveType::Blob => Err(format!(
                "field '{path}' is an array of Blobs; Terraform has no representation for the wire identity wrapper"
            )),
            ActiveType::OneOf => Err(format!(
                "field '{path}' is an array of unions; Terraform variables cannot express a union"
            )),
            ActiveType::Table { .. } => Ok("map(any)".to_string()),
            _ => tf_type(module, id, path),
        },
        _ => Err(format!("field '{path}' is not a schema element")),
    }
}

fn tf_requires_any(module: &Module, id: NodeId) -> bool {
    match module.get(id) {
        Some(Node::SchemaTable { fields, .. }) => {
            fields.values().any(|child| tf_requires_any(module, *child))
        }
        Some(Node::SchemaArray { element }) => tf_requires_any(module, *element),
        Some(Node::SchemaField { field }) => {
            field.requires.is_some() || field.one_of.iter().any(|b| tf_requires_any(module, *b))
        }
        _ => false,
    }
}

fn tf_contains_secret(module: &Module, id: NodeId) -> bool {
    match module.get(id) {
        Some(Node::SchemaTable { fields, .. }) => fields
            .values()
            .any(|child| tf_contains_secret(module, *child)),
        Some(Node::SchemaArray { element }) => tf_contains_secret(module, *element),
        Some(Node::SchemaField { field }) => {
            matches!(field.typ, ActiveType::Secret)
                || field.one_of.iter().any(|b| tf_contains_secret(module, *b))
        }
        _ => false,
    }
}

/// A static JSON number bound, or a refusal for a computed one. An
/// absent slot is fine; a present slot that is not statically known is
/// not (the generated contract would silently drop the constraint).
fn tf_number_bound(
    module: &Module,
    slot: Option<NodeId>,
    path: &str,
    which: &str,
) -> Result<Option<JsonValue>, String> {
    let Some(id) = slot else {
        return Ok(None);
    };
    literal_number(module, Some(id)).map(Some).ok_or_else(|| {
        format!("field '{path}' has a computed {which} bound; Terraform needs a static bound")
    })
}

fn tf_int_bound(
    module: &Module,
    slot: Option<NodeId>,
    path: &str,
    which: &str,
) -> Result<Option<i64>, String> {
    let Some(id) = slot else {
        return Ok(None);
    };
    literal_int(module, Some(id)).map(Some).ok_or_else(|| {
        format!("field '{path}' has a computed {which} bound; Terraform needs a static bound")
    })
}

/// Validation (condition, message) pairs for one root variable. Element
/// constraints become `alltrue([for v in var.x : ...])`.
fn tf_conditions(
    module: &Module,
    profile: &crate::secret::SecretProfile,
    id: NodeId,
    expr: &str,
    path: &str,
) -> Result<Vec<(String, String)>, String> {
    match module.get(id) {
        Some(Node::SchemaArray { element }) => {
            // An array of empty closed tables: every element must be an
            // empty map. Other table elements are refused by the type.
            if let Some(Node::SchemaTable { fields, closed, .. }) = module.get(*element) {
                if fields.is_empty() && *closed {
                    return Ok(vec![(
                        format!("alltrue([for v in {expr} : length(v) == 0])"),
                        format!("{path}: a closed table element has no keys"),
                    )]);
                }
                return Ok(Vec::new());
            }
            let inner =
                tf_scalar_conditions(module, profile, *element, "v", &format!("{path} element"))?;
            if inner.is_empty() {
                return Ok(Vec::new());
            }
            let joined = inner
                .iter()
                .map(|(condition, _)| condition.clone())
                .collect::<Vec<_>>()
                .join(" && ");
            Ok(vec![(
                format!("alltrue([for v in {expr} : {joined}])"),
                format!("{path}: an element violates the schema constraints"),
            )])
        }
        Some(Node::SchemaField { field }) => match &field.typ {
            ActiveType::Table { closed } if *closed => Ok(vec![(
                format!("length({expr}) == 0"),
                format!("{path}: a closed table has no keys"),
            )]),
            _ => tf_scalar_conditions(module, profile, id, expr, path),
        },
        Some(Node::SchemaTable { fields, closed, .. }) => {
            if fields.is_empty() && *closed {
                Ok(vec![(
                    format!("length({expr}) == 0"),
                    format!("{path}: a closed table has no keys"),
                )])
            } else {
                Ok(Vec::new())
            }
        }
        _ => Err(format!("field '{path}' is not a schema node")),
    }
}

fn tf_scalar_conditions(
    module: &Module,
    profile: &crate::secret::SecretProfile,
    id: NodeId,
    expr: &str,
    path: &str,
) -> Result<Vec<(String, String)>, String> {
    let Some(Node::SchemaField { field }) = module.get(id) else {
        return Ok(Vec::new());
    };
    let mut out: Vec<(String, String)> = Vec::new();
    let push_range = |out: &mut Vec<(String, String)>,
                      expr: &str,
                      path: &str,
                      min: Option<JsonValue>,
                      max: Option<JsonValue>| {
        if let Some(value) = min {
            out.push((
                format!("{expr} >= {value}"),
                format!("{path}: below the schema minimum"),
            ));
        }
        if let Some(value) = max {
            out.push((
                format!("{expr} <= {value}"),
                format!("{path}: above the schema maximum"),
            ));
        }
    };
    match &field.typ {
        ActiveType::Int => {
            out.push((
                format!("floor({expr}) == {expr}"),
                format!("{path}: an Int field must be a whole number"),
            ));
            push_range(
                &mut out,
                expr,
                path,
                tf_number_bound(module, field.min, path, "minimum")?,
                tf_number_bound(module, field.max, path, "maximum")?,
            );
        }
        ActiveType::Float => {
            push_range(
                &mut out,
                expr,
                path,
                tf_number_bound(module, field.min, path, "minimum")?,
                tf_number_bound(module, field.max, path, "maximum")?,
            );
        }
        ActiveType::Duration | ActiveType::Bytes => {
            // Canonical milliseconds / bytes are integers on the wire.
            out.push((
                format!("floor({expr}) == {expr}"),
                format!(
                    "{path}: a canonical {} magnitude must be a whole number",
                    if matches!(field.typ, ActiveType::Duration) {
                        "millisecond"
                    } else {
                        "byte"
                    }
                ),
            ));
            push_range(
                &mut out,
                expr,
                path,
                tf_number_bound(module, field.min, path, "minimum")?,
                tf_number_bound(module, field.max, path, "maximum")?,
            );
        }
        ActiveType::String => {
            if let Some(min) = tf_int_bound(module, field.min_len, path, "minimum length")? {
                out.push((
                    format!("length({expr}) >= {min}"),
                    format!("{path}: shorter than the schema minimum length"),
                ));
            }
            if let Some(max) = tf_int_bound(module, field.max_len, path, "maximum length")? {
                out.push((
                    format!("length({expr}) <= {max}"),
                    format!("{path}: longer than the schema maximum length"),
                ));
            }
            if field.pattern.is_some() {
                let Some(pattern) = literal_string(module, field.pattern) else {
                    return Err(format!(
                        "field '{path}' has a computed pattern; Terraform needs a static pattern"
                    ));
                };
                if profile.blob_identity_is_material(&pattern) {
                    return Err(format!(
                        "field '{path}' has a material pattern; it cannot be emitted"
                    ));
                }
                out.push((
                    format!("can(regex({}, {expr}))", hcl_string(&pattern)),
                    format!("{path}: does not match the schema pattern"),
                ));
            }
            if field.format.is_some() {
                let Some(format) = literal_string(module, field.format) else {
                    return Err(format!(
                        "field '{path}' has a computed format; Terraform needs a static format"
                    ));
                };
                if profile.blob_identity_is_material(&format) {
                    return Err(format!(
                        "field '{path}' has a material format; it cannot be emitted"
                    ));
                }
                match format_pattern(&format) {
                    Some(pattern) => out.push((
                        format!("can(regex({}, {expr}))", hcl_string(pattern)),
                        format!("{path}: does not match the schema format"),
                    )),
                    None => {
                        return Err(format!(
                            "field '{path}' names an unknown format; Terraform cannot express it"
                        ))
                    }
                }
            }
        }
        ActiveType::Enum(members) => {
            let mut rendered = Vec::new();
            for member in members {
                if profile.blob_identity_is_material(member) {
                    return Err(format!(
                        "field '{path}' has a material enum member; it cannot be emitted"
                    ));
                }
                rendered.push(hcl_string(member));
            }
            out.push((
                format!("contains([{}], {expr})", rendered.join(", ")),
                format!("{path}: not a member of the schema enum"),
            ));
        }
        ActiveType::Null => {
            out.push((
                format!("{expr} == null"),
                format!("{path}: a Null field accepts only null"),
            ));
        }
        _ => {}
    }
    Ok(out)
}

fn tf_default(
    module: &Module,
    profile: &crate::secret::SecretProfile,
    id: NodeId,
    path: &str,
) -> Result<Option<String>, String> {
    let Some(Node::SchemaField { field }) = module.get(id) else {
        return Ok(None);
    };
    if field.default.is_none() {
        return Ok(None);
    }
    let Some(value) = literal_default(module, field) else {
        return Err(format!(
            "field '{path}' has a computed default; Terraform needs a static default"
        ));
    };
    if default_is_material(profile, &value) {
        return Err(format!(
            "field '{path}' has a material default; it cannot be emitted"
        ));
    }
    tf_default_value(module, id, &value, path).map(Some)
}

fn tf_default_value(
    module: &Module,
    id: NodeId,
    value: &JsonValue,
    path: &str,
) -> Result<String, String> {
    match module.get(id) {
        Some(Node::SchemaField { field }) => match &field.typ {
            ActiveType::Duration => value
                .get("$active.duration")
                .and_then(|inner| inner.get("ms"))
                .map(|ms| ms.to_string())
                .ok_or_else(|| format!("field '{path}' default is not a duration")),
            ActiveType::Bytes | ActiveType::Int | ActiveType::Float => value
                .as_f64()
                .map(|_| value.to_string())
                .ok_or_else(|| format!("field '{path}' default is not a number")),
            ActiveType::String | ActiveType::Enum(_) => value
                .as_str()
                .map(hcl_string)
                .ok_or_else(|| format!("field '{path}' default is not a string")),
            ActiveType::Bool => value
                .as_bool()
                .map(|b| b.to_string())
                .ok_or_else(|| format!("field '{path}' default is not a boolean")),
            ActiveType::Null => Ok("null".to_string()),
            ActiveType::Secret => Err(format!(
                "field '{path}' is a Secret; a secret default is never emitted"
            )),
            ActiveType::Any | ActiveType::Table { .. } => Ok(json_to_hcl(value)),
            ActiveType::Blob | ActiveType::OneOf => {
                Err(format!("field '{path}' cannot carry a Terraform default"))
            }
        },
        Some(Node::SchemaArray { element }) => {
            let Some(items) = value.as_array() else {
                return Err(format!("field '{path}' default is not an array"));
            };
            let mut rendered = Vec::new();
            for item in items {
                rendered.push(tf_default_value(module, *element, item, path)?);
            }
            Ok(format!("[{}]", rendered.join(", ")))
        }
        Some(Node::SchemaTable { .. }) => Ok(json_to_hcl(value)),
        _ => Err(format!("field '{path}' default cannot be rendered")),
    }
}

/// Generic JSON-to-HCL value rendering for `Any` and bare-table defaults.
fn json_to_hcl(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::String(s) => hcl_string(s),
        JsonValue::Array(items) => {
            let rendered: Vec<String> = items.iter().map(json_to_hcl).collect();
            format!("[{}]", rendered.join(", "))
        }
        JsonValue::Object(map) => {
            let rendered: Vec<String> = map
                .iter()
                .map(|(key, item)| format!("{} = {}", hcl_string(key), json_to_hcl(item)))
                .collect();
            format!("{{ {} }}", rendered.join(", "))
        }
    }
}
