//! L-P25: the generated JSON Schema IS the contract.
//!
//! For every schema-bearing file in the repository corpus, this test:
//!   1. renders the schema back to Active source (the value contract; the
//!      evolution metadata that the JSON Schema does not model is omitted
//!      and counted),
//!   2. generates a base value plus boundary/omission/extra/requires
//!      candidates (the L-P66 deterministic family discipline),
//!   3. evaluates each candidate through the shipped engine (resilient:
//!      the generated `required`/`default` set follows the static-default
//!      rule, matching resilient fills), and
//!   4. validates the candidate's wire JSON against the generated schema
//!      with the pinned Python `jsonschema==4.26.0` batch check (the same
//!      validator the codegen/output-schema gates use; no Rust dependency,
//!      so its `serde_json` feature choices cannot perturb the evaluator).
//!
//! A verdict disagreement is a generator bug, never a reason to loosen the
//! Active schema. Classes the JSON Schema cannot express statically
//! (dotted/root-absolute `requires`, clear limits) are counted and
//! printed; a bare skip fails the test.

use active_core::convert::gen::schema_to_json_schema;
use active_core::ir::{Module, Node, NodeId, SchemaField, Value};
use active_core::types::ActiveType;
use active_core::{parse, parse_file, Engine, ParseOptions, Severity};
use serde_json::{json, Value as JsonValue};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Deterministic family RNG (the L-P66 xorshift64* convention).
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

// ---------------------------------------------------------------------------
// Literal / schema rendering (the value contract, without evolution
// metadata: `version`, `min_reader`, `min_writer`, `removed_in`, and
// `deprecated` are not part of the value-shape contract).
// ---------------------------------------------------------------------------

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Active's double-quoted escapes are `\n \t \r \\ \" \0 \$`; any other
/// control character has no source form, so a schema carrying one is
/// skipped (counted) rather than rendered into an unparseable document.
fn representable(text: &str) -> bool {
    text.chars()
        .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t' | '\0'))
}

/// A round-trip-safe float literal: Display's shortest form, never an
/// exponent (Active has no exponent syntax), always with a decimal point
/// (so it lexes as Float). `.6` formatting silently changed a bound.
fn float_source(value: f64) -> String {
    let mut text = format!("{value}");
    if text.contains(['e', 'E']) {
        text = format!("{value:.17}");
    }
    if !text.contains('.') {
        text.push_str(".0");
    }
    text
}

/// A source literal for a schema argument or a generated value. A float is
/// rendered with a decimal point (and never exponent notation: Active has
/// no exponent syntax). A `Ref` follows its binding chain, so a schema
/// that spells an argument through `let` renders as the literal it names.
fn literal(module: &Module, id: NodeId) -> Option<String> {
    literal_depth(module, id, 0)
}

fn literal_depth(module: &Module, id: NodeId, depth: usize) -> Option<String> {
    // Bound a malformed reference cycle; a real binding chain is short.
    if depth > 32 {
        return None;
    }
    match module.get(id)? {
        Node::Literal(Value::Null) => Some("null".to_string()),
        Node::Literal(Value::Bool(b)) => Some(b.to_string()),
        Node::Literal(Value::Int(n)) => Some(n.to_string()),
        Node::Literal(Value::String(s)) => representable(s).then(|| quote(s)),
        Node::Literal(Value::DurationMs(ms)) => Some(format!("{ms}ms")),
        Node::Literal(Value::Quantity(v, unit)) => Some(unit.format_amount(*v)),
        Node::Literal(Value::Float(f)) => Some(float_source(*f)),
        // A unit literal parses as a Quantity node, not a Literal value;
        // render its authored amount and unit (the percent amount is the
        // internal fraction, which `format_amount` scales back).
        Node::Quantity { value, unit } => {
            let amount = match module.get(*value)? {
                Node::Literal(Value::Int(n)) => *n as f64,
                Node::Literal(Value::Float(f)) => *f,
                _ => return None,
            };
            Some(unit.format_amount(amount))
        }
        Node::Ref(to) => literal_depth(module, *to, depth + 1),
        Node::Table { fields } => {
            let mut parts = Vec::new();
            for (name, child) in fields {
                parts.push(format!(
                    "{name} = {}",
                    literal_depth(module, *child, depth + 1)?
                ));
            }
            Some(format!("{{ {} }}", parts.join(", ")))
        }
        Node::Array { elements } => {
            let mut parts = Vec::new();
            for element in elements {
                parts.push(literal_depth(module, *element, depth + 1)?);
            }
            Some(format!("[{}]", parts.join(", ")))
        }
        _ => None,
    }
}

fn render_table_body(
    module: &Module,
    fields: &indexmap::IndexMap<String, NodeId>,
    closed: bool,
) -> Option<String> {
    let mut parts = Vec::new();
    if closed {
        parts.push("closed = true".to_string());
    }
    for (name, child) in fields {
        parts.push(format!("{name} = {}", render_node(module, *child)?));
    }
    Some(format!("{{ {} }}", parts.join(", ")))
}

fn render_node(module: &Module, id: NodeId) -> Option<String> {
    match module.get(id)? {
        Node::SchemaTable { fields, closed, .. } => Some(format!(
            "Table {}",
            render_table_body(module, fields, *closed)?
        )),
        Node::SchemaArray { element } => Some(format!("Array<{}>", render_node(module, *element)?)),
        Node::SchemaField { field } => render_field(module, field),
        _ => None,
    }
}

fn render_field(module: &Module, field: &SchemaField) -> Option<String> {
    let mut args: Vec<String> = Vec::new();
    for (name, slot) in [
        ("min", field.min),
        ("max", field.max),
        ("min_len", field.min_len),
        ("max_len", field.max_len),
        ("pattern", field.pattern),
        ("format", field.format),
        ("default", field.default),
        ("requires", field.requires),
        ("media_type", field.blob_media_type),
        ("max_size", field.blob_max_size),
    ] {
        if let Some(id) = slot {
            args.push(format!("{name} = {}", literal(module, id)?));
        }
    }
    if !field.one_of.is_empty() {
        let mut parts: Vec<String> = field
            .one_of
            .iter()
            .map(|branch| render_node(module, *branch))
            .collect::<Option<Vec<_>>>()?;
        parts.extend(args);
        return Some(format!("OneOf({})", parts.join(", ")));
    }
    let base = match &field.typ {
        ActiveType::Null => "Null".to_string(),
        ActiveType::Bool => "Bool".to_string(),
        ActiveType::Int => "Int".to_string(),
        ActiveType::Float => "Float".to_string(),
        ActiveType::String => "String".to_string(),
        ActiveType::Duration => "Duration".to_string(),
        ActiveType::Bytes => "Bytes".to_string(),
        ActiveType::Secret => "Secret".to_string(),
        ActiveType::Blob => {
            return Some(if args.is_empty() {
                "Blob".to_string()
            } else {
                format!("Blob({})", args.join(", "))
            });
        }
        ActiveType::Any => "Any".to_string(),
        ActiveType::Table { .. } => "Table".to_string(),
        ActiveType::Enum(members) => {
            if !members.iter().all(|member| representable(member)) {
                return None;
            }
            let mut parts: Vec<String> = members.iter().map(|member| quote(member)).collect();
            parts.extend(args);
            return Some(format!("Enum({})", parts.join(", ")));
        }
        ActiveType::OneOf => return None,
    };
    if args.is_empty() {
        Some(base)
    } else {
        Some(format!("{base}({})", args.join(", ")))
    }
}

fn schema_source(module: &Module) -> Option<String> {
    let root = module.schema?;
    match module.get(root)? {
        Node::SchemaTable { fields, closed, .. } => Some(format!(
            "schema = {}",
            render_table_body(module, fields, *closed)?
        )),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Generated values. A value carries both its Active source and its wire
// JSON, so a fatal evaluation still has a JSON form to validate.
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Gen {
    Scalar { source: String, json: JsonValue },
    Table(Vec<(String, Gen)>),
    Array(Vec<Gen>),
}

fn scalar(source: impl Into<String>, json: JsonValue) -> Gen {
    Gen::Scalar {
        source: source.into(),
        json,
    }
}

fn int(value: i64) -> Gen {
    scalar(value.to_string(), json!(value))
}

fn float(value: f64) -> Gen {
    scalar(float_source(value), json!(value))
}

fn text(value: &str) -> Gen {
    scalar(quote(value), json!(value))
}

fn boolean(value: bool) -> Gen {
    scalar(value.to_string(), json!(value))
}

fn null() -> Gen {
    scalar("null", JsonValue::Null)
}

fn duration(ms: u64) -> Gen {
    scalar(format!("{ms}ms"), json!({"$active.duration": {"ms": ms}}))
}

fn secret() -> Gen {
    scalar("secret(\"x\")", json!("***SECRET***"))
}

const BLOB_HASH: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn blob(media: &str, size: usize) -> Gen {
    let payload = "x".repeat(size);
    scalar(
        format!("blob({}, {})", quote(media), quote(&payload)),
        json!({"$active.blob": {
            "hash": BLOB_HASH,
            "size": size,
            "media_type": media,
            "tainted": false,
        }}),
    )
}

/// A structurally tainted identity (its data is a secret locator). The
/// wire redacts the media type to the shared marker (spec/blob.md §5),
/// so the generated contract must accept that spelling too.
fn blob_tainted(media: &str, size: usize) -> Gen {
    let payload = "x".repeat(size);
    scalar(
        format!("blob({}, secret({}))", quote(media), quote(&payload)),
        json!({"$active.blob": {
            "hash": BLOB_HASH,
            "size": size,
            "media_type": "***SECRET***",
            "tainted": true,
        }}),
    )
}

/// A zero quantity (`0%`): evaluation keeps the unit, and every zero
/// magnitude is disabled as a `requires` trigger.
fn quantity_zero() -> Gen {
    scalar(
        "0%",
        json!({"$active.quantity": {"unit": "%", "value": 0.0}}),
    )
}

/// A non-zero quantity (`1%`): enabled as a `requires` trigger. The wire
/// carries the internal fraction (spec/converters.md §4).
fn quantity_one() -> Gen {
    scalar(
        "1%",
        json!({"$active.quantity": {"unit": "%", "value": 0.01}}),
    )
}

fn table_empty() -> Gen {
    Gen::Table(Vec::new())
}

fn array_empty() -> Gen {
    Gen::Array(Vec::new())
}

fn render_value(value: &Gen) -> String {
    match value {
        Gen::Scalar { source, .. } => source.clone(),
        Gen::Table(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(name, child)| format!("{name} = {}", render_value(child)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Gen::Array(items) => {
            let parts: Vec<String> = items.iter().map(render_value).collect();
            format!("[{}]", parts.join(", "))
        }
    }
}

fn json_of(value: &Gen) -> JsonValue {
    match value {
        Gen::Scalar { json, .. } => json.clone(),
        Gen::Table(fields) => {
            let mut map = serde_json::Map::new();
            for (name, child) in fields {
                map.insert(name.clone(), json_of(child));
            }
            JsonValue::Object(map)
        }
        Gen::Array(items) => JsonValue::Array(items.iter().map(json_of).collect()),
    }
}

fn document(schema: &str, value: &Gen) -> String {
    let mut out = format!("{schema}\n");
    match value {
        Gen::Table(fields) => {
            for (name, child) in fields {
                out.push_str(&format!("{name} = {}\n", render_value(child)));
            }
        }
        other => out.push_str(&format!("value = {}\n", render_value(other))),
    }
    out
}

// ---------------------------------------------------------------------------
// Base values and boundary replacements.
// ---------------------------------------------------------------------------

fn bound_number(module: &Module, slot: Option<NodeId>) -> Option<f64> {
    let mut id = slot?;
    // A `let` binding resolves through its `Ref` chain, as evaluation
    // does; a bound spelled through a binding is static.
    for _ in 0..32 {
        match module.get(id)? {
            Node::Ref(to) => id = *to,
            Node::Literal(Value::Int(n)) => return Some(*n as f64),
            Node::Literal(Value::Float(f)) => return Some(*f),
            Node::Literal(Value::DurationMs(ms)) => return Some(*ms as f64),
            // L-P25/F2: a bound written with a unit (`1s`, `1KiB`) is a
            // Quantity node; normalize exactly as the generator does, or
            // the boundary candidates miss min-1/min/min+1 for the unit
            // classes.
            Node::Quantity { value, unit } => {
                let raw = match module.get(*value)? {
                    Node::Literal(Value::Int(n)) => *n as f64,
                    Node::Literal(Value::Float(f)) => *f,
                    Node::Ref(to) => match module.get(*to)? {
                        Node::Literal(Value::Int(n)) => *n as f64,
                        Node::Literal(Value::Float(f)) => *f,
                        _ => return None,
                    },
                    _ => return None,
                };
                return if let Some(ms) = unit.normalize_time_to_ms(raw) {
                    Some(ms as f64)
                } else if let Some(bytes) = unit.normalize_to_bytes(raw) {
                    Some(bytes as f64)
                } else {
                    Some(raw)
                };
            }
            _ => return None,
        }
    }
    None
}

fn int_bound(module: &Module, slot: Option<NodeId>) -> Option<i64> {
    let value = bound_number(module, slot)?;
    (value.is_finite() && value.fract() == 0.0).then_some(value as i64)
}

fn float_bound(module: &Module, slot: Option<NodeId>) -> Option<f64> {
    bound_number(module, slot)
}

fn string_bound(module: &Module, slot: Option<NodeId>) -> Option<usize> {
    int_bound(module, slot).and_then(|n| usize::try_from(n).ok())
}

fn format_sample(format: &str) -> Option<&'static str> {
    match format {
        "email" => Some("user@example.com"),
        "uri" => Some("https://example.com/path"),
        "uuid" => Some("00000000-0000-4000-8000-000000000000"),
        "semver" => Some("1.0.0"),
        _ => None,
    }
}

fn base_value(module: &Module, id: NodeId, rng: &mut Rng) -> Option<Gen> {
    match module.get(id)? {
        Node::SchemaTable { fields, .. } => {
            let mut out = Vec::new();
            for (name, child) in fields {
                out.push((name.clone(), base_value(module, *child, rng)?));
            }
            Some(Gen::Table(out))
        }
        Node::SchemaArray { element } => Some(Gen::Array(vec![base_value(module, *element, rng)?])),
        Node::SchemaField { field } => {
            if !field.one_of.is_empty() {
                return base_value(module, field.one_of[0], rng);
            }
            Some(match &field.typ {
                ActiveType::Null => null(),
                ActiveType::Bool => boolean(true),
                ActiveType::Int => {
                    let value = int_bound(module, field.min)
                        .or_else(|| int_bound(module, field.max))
                        .unwrap_or(0);
                    int(value)
                }
                ActiveType::Float => {
                    let value = float_bound(module, field.min)
                        .or_else(|| float_bound(module, field.max))
                        .unwrap_or(1.0);
                    float(value)
                }
                ActiveType::String => {
                    let min = string_bound(module, field.min_len).unwrap_or(1);
                    let max = string_bound(module, field.max_len);
                    let format = field
                        .format
                        .and_then(|id| literal(module, id))
                        .map(|text| text.trim_matches('"').to_string());
                    if let Some(sample) = format.as_deref().and_then(format_sample) {
                        text(sample)
                    } else {
                        let len = max
                            .map(|max| min.max(1).min(max.max(1)))
                            .unwrap_or(min.max(1));
                        text(&"x".repeat(len.max(1)))
                    }
                }
                ActiveType::Duration => {
                    let ms = int_bound(module, field.min)
                        .or_else(|| int_bound(module, field.max))
                        .unwrap_or(1000)
                        .max(0) as u64;
                    duration(ms)
                }
                ActiveType::Bytes => {
                    let value = int_bound(module, field.min)
                        .or_else(|| int_bound(module, field.max))
                        .unwrap_or(1)
                        .max(0);
                    int(value)
                }
                ActiveType::Secret => secret(),
                ActiveType::Blob => {
                    let media = field
                        .blob_media_type
                        .and_then(|id| module.get(id))
                        .and_then(|node| match node {
                            Node::Literal(Value::String(text)) => Some(text.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| "text/plain".to_string());
                    let max = field
                        .blob_max_size
                        .and_then(|id| int_bound(module, Some(id)))
                        .and_then(|n| usize::try_from(n).ok());
                    let size = max.map(|max| 2.min(max)).unwrap_or(2);
                    blob(&media, size)
                }
                ActiveType::Table { .. } => table_empty(),
                ActiveType::Any => null(),
                ActiveType::Enum(members) => {
                    let index = if members.is_empty() {
                        0
                    } else {
                        rng.below(members.len() as u64) as usize
                    };
                    text(members.get(index).map(String::as_str).unwrap_or(""))
                }
                ActiveType::OneOf => return None,
            })
        }
        _ => None,
    }
}

/// Boundary and off-type replacements for one leaf field.
fn replacements(module: &Module, field: &SchemaField) -> Vec<Gen> {
    let mut out: Vec<Gen> = Vec::new();
    if !field.one_of.is_empty() {
        for branch in &field.one_of {
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
            if let Some(value) = base_value(module, *branch, &mut rng) {
                out.push(value);
            }
        }
        return dedupe(out);
    }
    match &field.typ {
        ActiveType::Int => {
            let min = int_bound(module, field.min);
            let max = int_bound(module, field.max);
            let values: Vec<i64> = match (min, max) {
                (Some(min), Some(max)) => vec![
                    min.saturating_sub(1),
                    min,
                    min.saturating_add(1),
                    max.saturating_sub(1),
                    max,
                    max.saturating_add(1),
                ],
                (Some(min), None) => vec![min.saturating_sub(1), min, min.saturating_add(1)],
                (None, Some(max)) => vec![max.saturating_sub(1), max, max.saturating_add(1)],
                (None, None) => vec![-1, 0, 1],
            };
            out.extend(values.into_iter().map(int));
            out.push(boolean(true));
        }
        ActiveType::Float => {
            let min = float_bound(module, field.min);
            let max = float_bound(module, field.max);
            let values: Vec<f64> = match (min, max) {
                (Some(min), Some(max)) => vec![min - 0.5, min, max, max + 0.5],
                (Some(min), None) => vec![min - 0.5, min, min + 0.5],
                (None, Some(max)) => vec![max - 0.5, max, max + 0.5],
                (None, None) => vec![-1.5, 0.0, 1.5],
            };
            out.extend(
                values
                    .into_iter()
                    .filter(|value| value.is_finite())
                    .map(float),
            );
            out.push(boolean(true));
        }
        ActiveType::String => {
            let min = string_bound(module, field.min_len);
            let max = string_bound(module, field.max_len);
            let mut lengths: Vec<usize> = Vec::new();
            if let Some(min) = min {
                lengths.push(min.saturating_sub(1));
                lengths.push(min);
                lengths.push(min.saturating_add(1));
            }
            if let Some(max) = max {
                lengths.push(max.saturating_sub(1));
                lengths.push(max);
                lengths.push(max.saturating_add(1));
            }
            if lengths.is_empty() {
                lengths.push(1);
            }
            for len in lengths {
                out.push(text(&"x".repeat(len)));
            }
            out.push(text(""));
            out.push(int(1));
            out.push(boolean(true));
            if let Some(format) = field
                .format
                .and_then(|id| literal(module, id))
                .map(|text| text.trim_matches('"').to_string())
            {
                if let Some(sample) = format_sample(&format) {
                    out.push(text(sample));
                }
                if format != "semver" {
                    out.push(text("not-a-format-value"));
                }
            }
        }
        ActiveType::Duration => {
            let min = int_bound(module, field.min);
            let max = int_bound(module, field.max);
            let values: Vec<i64> = match (min, max) {
                (Some(min), Some(max)) => vec![
                    min.saturating_sub(1),
                    min,
                    min.saturating_add(1),
                    max.saturating_sub(1),
                    max,
                    max.saturating_add(1),
                ],
                (Some(min), None) => vec![min.saturating_sub(1), min, min.saturating_add(1)],
                (None, Some(max)) => vec![max.saturating_sub(1), max, max.saturating_add(1)],
                (None, None) => vec![0, 1, 1000],
            };
            out.extend(values.into_iter().map(|ms| duration(ms.max(0) as u64)));
            out.push(boolean(true));
        }
        ActiveType::Bytes => {
            let min = int_bound(module, field.min);
            let max = int_bound(module, field.max);
            let values: Vec<i64> = match (min, max) {
                (Some(min), Some(max)) => vec![
                    min.saturating_sub(1),
                    min,
                    min.saturating_add(1),
                    max.saturating_sub(1),
                    max,
                    max.saturating_add(1),
                ],
                (Some(min), None) => vec![min.saturating_sub(1), min, min.saturating_add(1)],
                (None, Some(max)) => vec![max.saturating_sub(1), max, max.saturating_add(1)],
                (None, None) => vec![0, 1],
            };
            out.extend(values.into_iter().map(int));
            out.push(boolean(true));
        }
        ActiveType::Bool => {
            out.push(boolean(false));
            out.push(boolean(true));
            out.push(int(1));
        }
        ActiveType::Null => {
            out.push(null());
            out.push(int(1));
        }
        ActiveType::Secret => {
            out.push(secret());
            out.push(int(1));
        }
        ActiveType::Blob => {
            let media = field
                .blob_media_type
                .and_then(|id| literal(module, id))
                .map(|text| text.trim_matches('"').to_string())
                .unwrap_or_else(|| "text/plain".to_string());
            out.push(blob(&media, 2));
            // L-P25/F2 (review 2): a tainted identity exports the media
            // marker, not the authored type; both spellings are valid
            // wire values for the field.
            out.push(blob_tainted(&media, 2));
            // L-P25/F3: the validating blob bounds (size maximum, media
            // const) must be exercised, not just the happy path.
            let max = field
                .blob_max_size
                .and_then(|id| int_bound(module, Some(id)))
                .and_then(|value| usize::try_from(value).ok());
            if let Some(max) = max {
                out.push(blob(&media, max.saturating_add(1)));
            }
            let wrong = if media == "text/plain" {
                "application/json"
            } else {
                "text/plain"
            };
            out.push(blob(wrong, 1));
            out.push(int(1));
        }
        ActiveType::Table { .. } => {
            out.push(table_empty());
            out.push(int(1));
        }
        ActiveType::Any => {
            out.push(null());
            out.push(int(1));
        }
        ActiveType::Enum(members) => {
            for member in members {
                out.push(text(member));
            }
            out.push(text("__not_a_member__"));
            out.push(int(1));
        }
        ActiveType::OneOf => {}
    }
    dedupe(out)
}

fn dedupe(values: Vec<Gen>) -> Vec<Gen> {
    let mut out: Vec<Gen> = Vec::new();
    for value in values {
        let source = render_value(&value);
        if !out.iter().any(|existing| render_value(existing) == source) {
            out.push(value);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Paths and mutation helpers.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    Key(String),
    Index(usize),
}

type PathSegments = Vec<Seg>;

#[derive(Clone)]
enum Kind {
    Field(Box<SchemaField>),
    Table,
    Array,
}

struct Site {
    path: PathSegments,
    kind: Kind,
}

struct RequiresPair {
    table: PathSegments,
    field: String,
    target: String,
    target_id: NodeId,
}

/// Everything the corpus walk discovers in one schema tree. Grouping the
/// collectors keeps the recursive walker readable and its arguments small.
struct Discovery {
    sites: Vec<Site>,
    tables: Vec<PathSegments>,
    fields: Vec<PathSegments>,
    requires: Vec<RequiresPair>,
    computed_requires: usize,
}

fn join(path: &PathSegments, seg: Seg) -> PathSegments {
    let mut out = path.clone();
    out.push(seg);
    out
}

fn collect(module: &Module, id: NodeId, path: &PathSegments, found: &mut Discovery) {
    if let Some(Node::SchemaTable {
        fields: entries, ..
    }) = module.get(id)
    {
        found.tables.push(path.clone());
        for (name, child) in entries {
            let child_path = join(path, Seg::Key(name.clone()));
            found.fields.push(child_path.clone());
            match module.get(*child) {
                Some(Node::SchemaField { field }) => {
                    if field.requires.is_some() {
                        match active_core::convert::requires_target(module, field) {
                            Some(target) => {
                                if entries.contains_key(&target) {
                                    if let Some(target_id) = entries.get(&target).copied() {
                                        found.requires.push(RequiresPair {
                                            table: path.clone(),
                                            field: name.clone(),
                                            target,
                                            target_id,
                                        });
                                    }
                                }
                            }
                            // A target computed at evaluation time cannot
                            // be paired statically; the schema is skipped
                            // and counted (the generator annotates it).
                            None => found.computed_requires += 1,
                        }
                    }
                    found.sites.push(Site {
                        path: child_path.clone(),
                        kind: Kind::Field(Box::new(field.clone())),
                    });
                    // A disjunction's branch tables are schema positions
                    // too; the generator emits conditionals and bounds
                    // inside them, so the value oracle must see them.
                    for branch in &field.one_of {
                        match module.get(*branch) {
                            Some(Node::SchemaTable { .. }) => {
                                found.sites.push(Site {
                                    path: child_path.clone(),
                                    kind: Kind::Table,
                                });
                                collect(module, *branch, &child_path, found);
                            }
                            Some(Node::SchemaArray { .. }) => {
                                found.sites.push(Site {
                                    path: child_path.clone(),
                                    kind: Kind::Array,
                                });
                                collect_element(
                                    module,
                                    *branch,
                                    &join(&child_path, Seg::Index(0)),
                                    found,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                Some(Node::SchemaArray { .. }) => {
                    found.sites.push(Site {
                        path: child_path.clone(),
                        kind: Kind::Array,
                    });
                    if let Some(Node::SchemaArray { element }) = module.get(*child) {
                        collect_element(module, *element, &join(&child_path, Seg::Index(0)), found);
                    }
                }
                Some(Node::SchemaTable { .. }) => {
                    found.sites.push(Site {
                        path: child_path.clone(),
                        kind: Kind::Table,
                    });
                    collect(module, *child, &child_path, found);
                }
                _ => {}
            }
        }
    }
}

fn collect_element(module: &Module, id: NodeId, path: &PathSegments, found: &mut Discovery) {
    match module.get(id) {
        Some(Node::SchemaField { field }) => {
            found.sites.push(Site {
                path: path.clone(),
                kind: Kind::Field(Box::new(field.clone())),
            });
        }
        Some(Node::SchemaTable { .. }) => {
            found.sites.push(Site {
                path: path.clone(),
                kind: Kind::Table,
            });
            collect(module, id, path, found);
        }
        Some(Node::SchemaArray { element }) => {
            found.sites.push(Site {
                path: path.clone(),
                kind: Kind::Array,
            });
            collect_element(module, *element, &join(path, Seg::Index(0)), found);
        }
        _ => {}
    }
}

fn replace_at(value: &Gen, path: &[Seg], replacement: &Gen) -> Option<Gen> {
    if path.is_empty() {
        return Some(replacement.clone());
    }
    match (&value, &path[0]) {
        (Gen::Table(fields), Seg::Key(key)) => {
            let mut out = Vec::new();
            let mut found = false;
            for (name, child) in fields {
                if name == key {
                    out.push((name.clone(), replace_at(child, &path[1..], replacement)?));
                    found = true;
                } else {
                    out.push((name.clone(), child.clone()));
                }
            }
            if !found {
                return None;
            }
            Some(Gen::Table(out))
        }
        (Gen::Array(items), Seg::Index(index)) => {
            let mut out = items.clone();
            let item = out.get_mut(*index)?;
            *item = replace_at(item, &path[1..], replacement)?;
            Some(Gen::Array(out))
        }
        _ => None,
    }
}

fn remove_at(value: &Gen, path: &[Seg]) -> Option<Gen> {
    if path.is_empty() {
        return None;
    }
    match (&value, &path[0]) {
        (Gen::Table(fields), Seg::Key(key)) => {
            if path.len() == 1 {
                let out: Vec<(String, Gen)> = fields
                    .iter()
                    .filter(|(name, _)| name != key)
                    .cloned()
                    .collect();
                if out.len() == fields.len() {
                    return None;
                }
                return Some(Gen::Table(out));
            }
            let mut out = Vec::new();
            let mut found = false;
            for (name, child) in fields {
                if name == key {
                    let removed = remove_at(child, &path[1..])?;
                    out.push((name.clone(), removed));
                    found = true;
                } else {
                    out.push((name.clone(), child.clone()));
                }
            }
            found.then_some(Gen::Table(out))
        }
        (Gen::Array(items), Seg::Index(index)) => {
            if path.len() == 1 {
                let mut out = items.clone();
                if *index >= out.len() {
                    return None;
                }
                let _ = out.remove(*index);
                return Some(Gen::Array(out));
            }
            let mut out = items.clone();
            let item = out.get_mut(*index)?;
            *item = remove_at(item, &path[1..])?;
            Some(Gen::Array(out))
        }
        _ => None,
    }
}

fn add_at(value: &Gen, table_path: &[Seg], key: &str, item: &Gen) -> Option<Gen> {
    if table_path.is_empty() {
        let Gen::Table(fields) = value else {
            return None;
        };
        let mut out = fields.clone();
        if let Some(existing) = out.iter_mut().find(|(name, _)| name == key) {
            existing.1 = item.clone();
        } else {
            out.push((key.to_string(), item.clone()));
        }
        return Some(Gen::Table(out));
    }
    match (&value, &table_path[0]) {
        (Gen::Table(fields), Seg::Key(segment)) => {
            let mut out = Vec::new();
            let mut found = false;
            for (name, child) in fields {
                if name == segment {
                    out.push((name.clone(), add_at(child, &table_path[1..], key, item)?));
                    found = true;
                } else {
                    out.push((name.clone(), child.clone()));
                }
            }
            found.then_some(Gen::Table(out))
        }
        (Gen::Array(items), Seg::Index(index)) => {
            let mut out = items.clone();
            let child = out.get_mut(*index)?;
            *child = add_at(child, &table_path[1..], key, item)?;
            Some(Gen::Array(out))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// requires trigger values.
// ---------------------------------------------------------------------------

fn disabled_value(module: &Module, id: NodeId) -> Option<Gen> {
    match module.get(id)? {
        Node::SchemaField { field } => match &field.typ {
            ActiveType::Null => Some(null()),
            ActiveType::Bool => Some(boolean(false)),
            ActiveType::Int | ActiveType::Bytes => Some(int(0)),
            ActiveType::Float => Some(float(0.0)),
            ActiveType::String | ActiveType::Enum(_) => Some(text("")),
            ActiveType::Duration => Some(duration(0)),
            ActiveType::Table { .. } => Some(table_empty()),
            ActiveType::Secret | ActiveType::Blob => None,
            // Any/OneOf enablement is the L-P10 truthiness; null is the
            // disabled representative (the union also covers false/0/"").
            ActiveType::Any | ActiveType::OneOf => Some(null()),
        },
        Node::SchemaTable { .. } => Some(table_empty()),
        Node::SchemaArray { .. } => Some(array_empty()),
        _ => None,
    }
}

/// Every disabled form worth exercising for a trigger. An `Any`/`OneOf`
/// trigger holds any runtime type, so the zero quantity (a unit the
/// static enum cannot enumerate) and the other truthiness shapes are
/// each tried; a static type only has its single form.
fn disabled_triggers(module: &Module, id: NodeId) -> Vec<Gen> {
    let mut out: Vec<Gen> = disabled_value(module, id).into_iter().collect();
    if matches!(
        module.get(id),
        Some(Node::SchemaField { field })
            if matches!(field.typ, ActiveType::Any | ActiveType::OneOf)
    ) {
        out.push(quantity_zero());
        out.push(int(0));
        out.push(text(""));
        out.push(boolean(false));
        out.push(duration(0));
        out.push(table_empty());
    }
    dedupe(out)
}

fn enabled_value(module: &Module, id: NodeId, rng: &mut Rng) -> Option<Gen> {
    match module.get(id)? {
        Node::SchemaField { field } => match &field.typ {
            ActiveType::Null => None,
            ActiveType::Bool => Some(boolean(true)),
            ActiveType::Int | ActiveType::Bytes => Some(int(1)),
            ActiveType::Float => Some(float(1.0)),
            ActiveType::String => Some(text("x")),
            ActiveType::Enum(members) => members
                .iter()
                .find(|member| !member.is_empty())
                .map(|member| text(member)),
            ActiveType::Duration => Some(duration(1)),
            ActiveType::Table { .. } => {
                let value = base_value(module, id, rng)?;
                match &value {
                    Gen::Table(fields) if fields.is_empty() => None,
                    _ => Some(value),
                }
            }
            ActiveType::Secret => Some(secret()),
            ActiveType::Blob => Some(blob("text/plain", 2)),
            ActiveType::Any => Some(boolean(true)),
            ActiveType::OneOf => field
                .one_of
                .iter()
                .find_map(|branch| enabled_value(module, *branch, rng)),
        },
        Node::SchemaTable { fields, .. } => {
            if fields.is_empty() {
                None
            } else {
                base_value(module, id, rng)
            }
        }
        Node::SchemaArray { .. } => Some(Gen::Array(vec![int(1)])),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Active verdict.
// ---------------------------------------------------------------------------

fn evaluate_document(document: &str) -> (bool, Vec<String>) {
    let module = parse(document, "generated-contract")
        .unwrap_or_else(|diags| panic!("generated source must parse: {diags:?}\n{document}"));
    match Engine::new().evaluate(&module) {
        Ok((_, diagnostics)) => (
            !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error),
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect(),
        ),
        Err(error) => (false, vec![error.to_string()]),
    }
}

/// A schema defect (a static default that does not satisfy its own type)
/// is not a value verdict: the corpus includes such negative fixtures,
/// and a JSON Schema default is an annotation the engine's resilient fill
/// reports separately. Skipped, counted, and printed. Dangling
/// `requires` targets and unknown formats are no longer in this class:
/// the generator renders those fields/tables unsatisfiable (the engine
/// rejects every document), so the differential can check them.
fn schema_defect(messages: &[String]) -> bool {
    messages
        .iter()
        .any(|message| message.contains("schema default does not satisfy"))
}

// ---------------------------------------------------------------------------
// Corpus enumeration.
// ---------------------------------------------------------------------------

fn corpus_roots(workspace: &Path) -> Vec<PathBuf> {
    [
        "conformance/cases",
        "examples",
        "schemas",
        "docs-site/src",
        "tests",
        "agent-kit/vectors",
        "active-core/tests/fixtures",
    ]
    .iter()
    .map(|relative| workspace.join(relative))
    .collect()
}

fn walk_active(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_active(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("active") {
            out.push(path);
        }
    }
}

fn corpus() -> Vec<PathBuf> {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf();
    let mut paths = Vec::new();
    for root in corpus_roots(&workspace) {
        walk_active(&root, &mut paths);
    }
    paths.sort();
    paths.dedup();
    paths
}

fn parse_options() -> ParseOptions {
    let mut options = ParseOptions::strict();
    options.enable_includes = true;
    options.allow_parent_escapes = true;
    options
}

/// Run the batch validator with a 600-second ceiling, trying the common
/// interpreter names. A missing interpreter is skipped; a broken one (the
/// Windows Store alias) is retried under the next name. Stdout is unused
/// and discarded, and stderr is inherited: a pipe left unread can fill
/// and block the child, and an inherited traceback is visible in the test
/// log without one.
fn run_validator(script: &Path, input: &Path, output: &Path) -> std::process::ExitStatus {
    let candidates = std::env::var("PYTHON")
        .ok()
        .into_iter()
        .chain(["python".to_string(), "python3".to_string()]);
    let mut failures: Vec<String> = Vec::new();
    for candidate in candidates {
        let mut child = match std::process::Command::new(&candidate)
            .arg(script)
            .arg(input)
            .arg(output)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => continue,
        };
        let start = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if start.elapsed() > std::time::Duration::from_secs(600) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("the JSON Schema validator exceeded 600s");
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(error) => panic!("waiting for the validator failed: {error}"),
            }
        };
        // Reap the process before deciding; a failed run may still hold
        // resources until waited on.
        let _ = child.wait();
        if status.success() {
            return status;
        }
        failures.push(format!("{candidate}: exit {status}"));
    }
    panic!(
        "JSON Schema validator failed (is Python jsonschema==4.26.0 installed?):\n{}",
        failures.join("\n")
    );
}

/// `requires` targets that cross objects (dotted or root-absolute): JSON
/// Schema cannot express them in the same object, so the generator
/// annotates them and the differential test counts them.
fn cross_object_requires(module: &Module, id: NodeId) -> usize {
    match module.get(id) {
        Some(Node::SchemaTable { fields, .. }) => {
            let mut count = 0;
            for (_, child) in fields {
                if let Some(Node::SchemaField { field }) = module.get(*child) {
                    if let Some(target) = active_core::convert::requires_target(module, field) {
                        // Only a real cross-object target is a limitation;
                        // a dangling target makes the table unsatisfiable
                        // and is checked by the value oracle.
                        if active_core::convert::requires_target_exists(module, *child, &target)
                            && !fields.contains_key(&target)
                        {
                            count += 1;
                        }
                    }
                }
                count += cross_object_requires(module, *child);
            }
            count
        }
        Some(Node::SchemaArray { element }) => cross_object_requires(module, *element),
        // A disjunction branch is its own comparison scope; walk it too.
        Some(Node::SchemaField { field }) => field
            .one_of
            .iter()
            .map(|branch| cross_object_requires(module, *branch))
            .sum(),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// The differential test.
// ---------------------------------------------------------------------------

/// A per-schema candidate bound: a hostile or accidentally enormous schema
/// fails the harness instead of growing the candidate list without limit.
/// The current corpus peaks far below this.
const MAX_CANDIDATES_PER_SCHEMA: usize = 4_000;

fn push_value(values: &mut Vec<(String, Gen)>, pair: (String, Gen)) {
    if values.len() >= MAX_CANDIDATES_PER_SCHEMA {
        panic!(
            "a schema generated more than {MAX_CANDIDATES_PER_SCHEMA} candidates; \
             narrow the fixture rather than lowering the differential bar"
        );
    }
    values.push(pair);
}

struct Batch {
    path: PathBuf,
    schema_source: String,
    json_schema: JsonValue,
    /// (label, wire JSON, Active verdict, generated document)
    values: Vec<(String, JsonValue, bool, String)>,
}

#[test]
fn generated_json_schema_agrees_with_the_active_schema() {
    let mut rng = Rng(0xA5A5_5A5A_1234_5678);
    let mut schemas = 0usize;
    let mut candidates = 0usize;
    let mut skipped_parse = 0usize;
    let mut skipped_render = 0usize;
    let mut skipped_schema_defect = 0usize;
    let mut skipped_cross_object = 0usize;
    let mut skipped_computed_requires = 0usize;
    let mut computed_requires_limited = 0usize;
    let mut limitations = 0usize;
    let mut requires_limited = 0usize;
    let mut batches: Vec<Batch> = Vec::new();

    for path in corpus() {
        let module = match parse_file(&path, parse_options()) {
            Ok(module) if module.schema.is_some() => module,
            Ok(_) => continue,
            Err(_) => {
                skipped_parse += 1;
                continue;
            }
        };
        let Some(schema_source) = schema_source(&module) else {
            skipped_render += 1;
            continue;
        };
        let Ok(json_schema) = schema_to_json_schema(&module) else {
            skipped_render += 1;
            continue;
        };
        let root = module.schema.expect("schema root");
        let Some(base) = base_value(&module, root, &mut rng) else {
            skipped_render += 1;
            continue;
        };
        let mut found = Discovery {
            sites: Vec::new(),
            tables: Vec::new(),
            fields: Vec::new(),
            requires: Vec::new(),
            computed_requires: 0,
        };
        collect(&module, root, &Vec::new(), &mut found);

        // A schema defect (bad default, dangling requires) is a negative
        // corpus fixture, not a value contract.
        let (_, base_messages) = evaluate_document(&document(&schema_source, &base));
        if schema_defect(&base_messages) {
            skipped_schema_defect += 1;
            continue;
        }

        // A cross-object `requires` (dotted or root-absolute) is annotated
        // rather than expressed; the generator's contract is deliberately
        // incomplete there, so the value oracle skips the schema and the
        // limitation is counted (never a silent pass).
        let cross = cross_object_requires(&module, root);
        if cross > 0 {
            requires_limited += cross;
            skipped_cross_object += 1;
            continue;
        }

        // A target computed at evaluation time has no static spelling:
        // the generated contract annotates the field and leaves the
        // conditional unexpressed, so the value oracle cannot pair it
        // with the right trigger. Counted, never a silent pass.
        if found.computed_requires > 0 {
            computed_requires_limited += found.computed_requires;
            skipped_computed_requires += 1;
            continue;
        }

        let mut values: Vec<(String, Gen)> = vec![("base".to_string(), base.clone())];

        for site in &found.sites {
            match &site.kind {
                Kind::Field(field) => {
                    for replacement in replacements(&module, field) {
                        if let Some(value) = replace_at(&base, &site.path, &replacement) {
                            push_value(&mut values, (format!("boundary {:?}", site.path), value));
                        }
                    }
                }
                Kind::Table => {
                    if let Some(value) = replace_at(&base, &site.path, &table_empty()) {
                        push_value(&mut values, (format!("empty table {:?}", site.path), value));
                    }
                }
                Kind::Array => {
                    if let Some(value) = replace_at(&base, &site.path, &array_empty()) {
                        push_value(&mut values, (format!("empty array {:?}", site.path), value));
                    }
                }
            }
        }

        for field_path in &found.fields {
            if let Some(value) = remove_at(&base, field_path) {
                push_value(&mut values, (format!("omit {:?}", field_path), value));
            }
        }

        for table in &found.tables {
            let extra = int(rng.below(10_000) as i64);
            if let Some(value) = add_at(&base, table, "zzz_extra_key_zzz", &extra) {
                push_value(&mut values, (format!("extra key {:?}", table), value));
            }
        }

        // Closed root metadata: the validator exempts the two metadata keys,
        // and the generated contract allows them.
        if matches!(
            module.get(root),
            Some(Node::SchemaTable { closed: true, .. })
        ) {
            if let Some(value) = add_at(&base, &Vec::new(), "active_mode", &text("config")) {
                push_value(&mut values, ("closed root active_mode".to_string(), value));
            }
            if let Some(value) = add_at(&base, &Vec::new(), "active_capabilities", &array_empty()) {
                push_value(
                    &mut values,
                    ("closed root active_capabilities".to_string(), value),
                );
            }
        }

        for pair in &found.requires {
            let target_path = join(&pair.table, Seg::Key(pair.target.clone()));
            let field_path = join(&pair.table, Seg::Key(pair.field.clone()));
            for (variant, disabled) in disabled_triggers(&module, pair.target_id)
                .into_iter()
                .enumerate()
            {
                if let Some(value) = replace_at(&base, &target_path, &disabled) {
                    push_value(
                        &mut values,
                        (
                            format!("requires {} disabled variant {variant}", pair.field),
                            value,
                        ),
                    );
                }
                if let Some(value) = replace_at(&base, &target_path, &disabled) {
                    if let Some(value) = remove_at(&value, &field_path) {
                        push_value(
                            &mut values,
                            (
                                format!(
                                    "requires {} absent, target disabled variant {variant}",
                                    pair.field
                                ),
                                value,
                            ),
                        );
                    }
                }
            }
            if let Some(value) = remove_at(&base, &target_path) {
                push_value(
                    &mut values,
                    (format!("requires {} target absent", pair.field), value),
                );
            }
            if let Some(enabled) = enabled_value(&module, pair.target_id, &mut rng) {
                if let Some(value) = replace_at(&base, &target_path, &enabled) {
                    if let Some(value) = remove_at(&value, &field_path) {
                        push_value(
                            &mut values,
                            (
                                format!("requires {} absent, target enabled", pair.field),
                                value,
                            ),
                        );
                    }
                }
            }
            // An `Any`/`OneOf` trigger also holds a non-zero quantity
            // (enabled): the contract's disabled shape must not catch it.
            if matches!(
                module.get(pair.target_id),
                Some(Node::SchemaField { field })
                    if matches!(field.typ, ActiveType::Any | ActiveType::OneOf)
            ) {
                if let Some(value) = replace_at(&base, &target_path, &quantity_one()) {
                    if let Some(value) = remove_at(&value, &field_path) {
                        push_value(
                            &mut values,
                            (
                                format!(
                                    "requires {} absent, target enabled by quantity",
                                    pair.field
                                ),
                                value,
                            ),
                        );
                    }
                }
            }
        }

        schemas += 1;
        let mut wire = Vec::new();
        for (label, value) in values {
            candidates += 1;
            let document = document(&schema_source, &value);
            let (active, _) = evaluate_document(&document);
            wire.push((label, json_of(&value), active, document));
        }
        batches.push(Batch {
            path: path.clone(),
            schema_source,
            json_schema,
            values: wire,
        });
        if module
            .arena
            .iter()
            .any(|node| matches!(node, Node::SchemaTable { meta, .. } if meta.version.is_some() || meta.min_reader.is_some() || meta.min_writer.is_some()))
        {
            limitations += 1;
        }
    }

    // The verdicts come from the same pinned validator the rest of the
    // tooling uses (Python `jsonschema==4.26.0`, CI-installed), so adding
    // the validator cannot change the Rust dependency graph (and its
    // `serde_json/float_roundtrip` feature) underneath the evaluator.
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf();
    let script = workspace.join("conformance/evidence/jsonschema_batch.py");
    let input = serde_json::Value::Array(
        batches
            .iter()
            .map(|batch| {
                json!({
                    "schema": batch.json_schema,
                    "values": batch
                        .values
                        .iter()
                        .map(|(_, value, _, _)| value.clone())
                        .collect::<Vec<_>>(),
                })
            })
            .collect(),
    );
    // A predictable temp directory in a shared location is a TOCTOU
    // target. RandomState is seeded by the OS, and `create_dir` refuses
    // an existing path (including a planted symlink), so a collision
    // retries under a fresh token instead of following one. On Windows
    // `%TEMP%` is per-user and ACL-restricted by the OS; std cannot set
    // a directory ACL portably, so the mode is tightened on Unix only.
    let token = {
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        );
        hasher.write_u32(std::process::id());
        hasher.finish()
    };
    let mut dir = None;
    for attempt in 0..16u32 {
        let candidate = std::env::temp_dir().join(format!("active-lp25-{token:x}-{attempt}"));
        match std::fs::create_dir(&candidate) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &candidate,
                        std::fs::Permissions::from_mode(0o700),
                    );
                }
                dir = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("cannot create the validator temp directory: {error}"),
        }
    }
    let dir = dir.unwrap_or_else(|| panic!("cannot create a unique validator temp directory"));
    let input_path = dir.join("input.json");
    let output_path = dir.join("output.json");
    std::fs::write(
        &input_path,
        serde_json::to_vec(&input).expect("serialize validator input"),
    )
    .expect("write validator input");
    let _run = run_validator(&script, &input_path, &output_path);
    let verdicts: Vec<Vec<bool>> =
        serde_json::from_slice(&std::fs::read(&output_path).expect("read validator output"))
            .expect("validator output is a JSON array");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(verdicts.len(), batches.len(), "one verdict list per schema");
    for (batch, verdicts) in batches.iter().zip(verdicts) {
        assert_eq!(
            verdicts.len(),
            batch.values.len(),
            "{}: one verdict per value",
            batch.path.display()
        );
        for ((label, wire, active, document), json_ok) in batch.values.iter().zip(verdicts) {
            if *active != json_ok {
                panic!(
                    "{}: {label}\n--- schema ---\n{}\n--- document ---\n{}\n--- json ---\n{}\nactive={active} json_schema={json_ok}\n--- generated schema ---\n{}",
                    batch.path.display(),
                    batch.schema_source,
                    document,
                    wire,
                    serde_json::to_string_pretty(&batch.json_schema).unwrap()
                );
            }
        }
    }

    // Coverage gate: the exercised schemas must stay the majority of the
    // schema-bearing corpus. A pile of unrenderable or skipped files can
    // no longer leave a small green core: adding enough of them fails the
    // ratio even when the absolute count still clears the floor.
    // Parse-skipped files are intentionally outside the denominator:
    // they are the corpus's parse-diagnostic fixtures and mostly declare
    // no schema at all (the conformance suite owns their diagnostics).
    // Render/defect/limitation skips are included because each is a
    // schema the generator could not prove.
    let schema_bearing = schemas
        + skipped_render
        + skipped_schema_defect
        + skipped_cross_object
        + skipped_computed_requires;
    assert!(
        schemas >= 60,
        "corpus too small: {schemas} exercised schemas (floor 60; skipped parse {skipped_parse}, render {skipped_render}, schema defect {skipped_schema_defect}, cross-object {skipped_cross_object}, computed requires {skipped_computed_requires})"
    );
    assert!(
        schemas * 2 >= schema_bearing,
        "differential coverage fell below half the schema-bearing corpus: {schemas} exercised of {schema_bearing} (render {skipped_render}, schema defect {skipped_schema_defect}, cross-object {skipped_cross_object}, computed requires {skipped_computed_requires}); a skip class grew without a conscious gate update"
    );
    eprintln!(
        "L-P25 differential: {schemas} schemas, {candidates} candidates, skipped parse {skipped_parse}, render {skipped_render}, schema-defect {skipped_schema_defect}, cross-object-requires {skipped_cross_object} ({requires_limited} fields), computed-requires {skipped_computed_requires} ({computed_requires_limited} fields), evolution-limited {limitations}, validator divergences 0"
    );
}

#[test]
fn unit_bounds_produce_off_by_one_candidates() {
    // L-P25/F2: `1s`/`1KiB` bounds are Quantity nodes; the boundary
    // generator must normalize them (1000/1024), not fall back to 0/1.
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generated-contracts/bounds.active"),
    )
    .expect("read bounds fixture");
    let module = parse(&source, "bounds").expect("parse");
    let root = module.schema.expect("schema");
    let fields = match module.get(root) {
        Some(Node::SchemaTable { fields, .. }) => fields,
        _ => panic!("schema root"),
    };
    let field = |name: &str| match module.get(fields[name]) {
        Some(Node::SchemaField { field }) => field,
        _ => panic!("field {name}"),
    };
    let sources = |field: &SchemaField| {
        replacements(&module, field)
            .into_iter()
            .map(|value| render_value(&value))
            .collect::<Vec<_>>()
    };
    let durations = sources(field("d"));
    for expected in ["999ms", "1000ms", "1001ms", "1999ms", "2000ms", "2001ms"] {
        assert!(
            durations.iter().any(|source| source == expected),
            "missing {expected}: {durations:?}"
        );
    }
    let bytes = sources(field("b"));
    for expected in ["1023", "1024", "1025", "2047", "2048", "2049"] {
        assert!(
            bytes.iter().any(|source| source == expected),
            "missing {expected}: {bytes:?}"
        );
    }
}
