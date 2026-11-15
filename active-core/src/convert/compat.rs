//! Schema compatibility reports (PROPOSAL §2.4, CHECKLIST SC.006–SC.007).
//!
//! Compares two documents' schema blocks field by field: added and
//! removed fields, type changes, tightened blob constraints, version
//! regressions, and raised reader floors. Findings carry stable C04x
//! codes (E040 summarizes a breaking result); the CLI fails closed
//! when any error-severity finding exists. Nothing here evaluates
//! values — only the two schema trees are read, so secrets cannot
//! enter the report.

use serde::Serialize;
use std::collections::HashMap;

use crate::ir::{Module, Node, SchemaField};

/// Compatibility finding codes (CHECKLIST SC.007).
pub mod code {
    /// Added required field (breaking for old readers).
    pub const ADDED_REQUIRED: &str = "C040";
    /// Removed field (breaking unless removed on schedule).
    pub const REMOVED_FIELD: &str = "C041";
    /// Field type changed or blob constraint tightened.
    pub const TYPE_CHANGED: &str = "C042";
    /// New version regressed below the old version.
    pub const VERSION_REGRESSED: &str = "C043";
    /// `min_reader` raised above the old version (drops readers).
    pub const READER_FLOOR_RAISED: &str = "C044";
    /// Breaking-change summary for a non-empty error set.
    pub const BREAKING_SUMMARY: &str = "E040";
}

/// One compatibility finding: stable code, severity, dotted path, and
/// a value-free message (type names and versions only — never data).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompatFinding {
    pub code: String,
    pub severity: String,
    pub path: String,
    pub message: String,
}

impl CompatFinding {
    fn error(code: &str, path: &str, message: String) -> Self {
        Self {
            code: code.to_string(),
            severity: "error".to_string(),
            path: path.to_string(),
            message,
        }
    }

    fn info(code: &str, path: &str, message: String) -> Self {
        Self {
            code: code.to_string(),
            severity: "info".to_string(),
            path: path.to_string(),
            message,
        }
    }

    pub fn is_breaking(&self) -> bool {
        self.severity == "error"
    }
}

/// Parsed `major.minor.patch` for ordering. Returns None for
/// non-conforming strings (the parser already rejects those with
/// E020, so this is a defensive second gate, never a silent pass).
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// A field is required when it is neither Null-typed nor defaulted.
fn field_required(field: &SchemaField) -> bool {
    !matches!(field.typ, crate::types::ActiveType::Null) && field.default.is_none()
}

/// One comparable schema entry: a scalar field, a table, or an array.
/// Nested tables/arrays are expanded to dotted paths so a change deep
/// in the tree can never hide behind an unchanged parent signature.
#[derive(Clone)]
struct Row {
    path: String,
    signature: String,
    /// Static constraints, compared with direction awareness. A
    /// type-only signature reported min/pattern/format/closed
    /// tightenings as `no schema changes` (LANG-B-004).
    constraints: Constraints,
    required: bool,
    removed_in: Option<String>,
    blob: Option<(Option<i64>, Option<String>)>,
    /// A blob bound argument exists but is not a literal, so it cannot
    /// be compared statically (LANG-B-029).
    blob_dynamic: bool,
}

fn node_signature(module: &Module, id: crate::ir::NodeId) -> String {
    match module.get(id) {
        Some(Node::SchemaField { field }) => field.typ.name(),
        Some(Node::SchemaTable { .. }) => "Table".to_string(),
        Some(Node::SchemaArray { element }) => {
            format!("Array<{}>", node_signature(module, *element))
        }
        _ => "unknown".to_string(),
    }
}

/// Static constraint set for one schema field. Direction matters: raising
/// a minimum (or lowering a maximum) can only invalidate documents that
/// validated before, while the reverse only widens the accepted set
/// (LANG-B-004).
#[derive(Clone, Default, PartialEq)]
struct Constraints {
    min: Option<f64>,
    max: Option<f64>,
    min_len: Option<f64>,
    max_len: Option<f64>,
    pattern: Option<String>,
    format: Option<String>,
    members: Option<Vec<String>>,
    closed: Option<bool>,
    has_computed: bool,
}

fn constraints_of(module: &Module, field: &SchemaField) -> Constraints {
    fn number(module: &Module, id: Option<crate::ir::NodeId>) -> (Option<f64>, bool) {
        match id.and_then(|n| module.get(n)) {
            Some(Node::Literal(crate::ir::Value::Int(n))) => (Some(*n as f64), false),
            Some(Node::Literal(crate::ir::Value::Float(f))) => (Some(*f), false),
            Some(_) => (None, true),
            None => (None, false),
        }
    }
    fn text(module: &Module, id: Option<crate::ir::NodeId>) -> (Option<String>, bool) {
        match id.and_then(|n| module.get(n)) {
            Some(Node::Literal(crate::ir::Value::String(s))) => (Some(s.clone()), false),
            Some(_) => (None, true),
            None => (None, false),
        }
    }
    let (min, c1) = number(module, field.min);
    let (max, c2) = number(module, field.max);
    let (min_len, c3) = number(module, field.min_len);
    let (max_len, c4) = number(module, field.max_len);
    let (pattern, c5) = text(module, field.pattern);
    let (format, c6) = text(module, field.format);
    let (members, closed) = match &field.typ {
        crate::types::ActiveType::Enum(members) => (Some(members.clone()), None),
        crate::types::ActiveType::Table { closed } => (None, Some(*closed)),
        _ => (None, None),
    };
    Constraints {
        min,
        max,
        min_len,
        max_len,
        pattern,
        format,
        members,
        closed,
        has_computed: c1 || c2 || c3 || c4 || c5 || c6,
    }
}

/// True when `new` can reject a document `old` accepted.
fn tightened(old: &Constraints, new: &Constraints) -> bool {
    // A computed constraint cannot be compared statically; any change in
    // its presence is surfaced rather than assumed safe.
    if old.has_computed != new.has_computed {
        return true;
    }
    let raises = |o: Option<f64>, n: Option<f64>| match (o, n) {
        (None, Some(_)) => true,
        (Some(o), Some(n)) => n > o,
        _ => false,
    };
    let lowers = |o: Option<f64>, n: Option<f64>| match (o, n) {
        (None, Some(_)) => true,
        (Some(o), Some(n)) => n < o,
        _ => false,
    };
    if raises(old.min, new.min)
        || lowers(old.max, new.max)
        || raises(old.min_len, new.min_len)
        || lowers(old.max_len, new.max_len)
    {
        return true;
    }
    if old.pattern != new.pattern || old.format != new.format {
        // Any edit can narrow the accepted set; report rather than guess.
        return old.pattern != new.pattern || old.format != new.format;
    }
    match (&old.members, &new.members) {
        (Some(o), Some(n)) => n.iter().any(|m| !o.contains(m)),
        (None, Some(_)) => true,
        _ => false,
    }
    .then_some(true)
    .unwrap_or(matches!(
        (old.closed, new.closed),
        (Some(false), Some(true)) | (None, Some(true))
    ))
}

/// Render the constraints that are set, for the finding message.
fn describe_constraints(c: &Constraints) -> String {
    let mut parts = Vec::new();
    if let Some(v) = c.min {
        parts.push(format!("min={v}"));
    }
    if let Some(v) = c.max {
        parts.push(format!("max={v}"));
    }
    if let Some(v) = c.min_len {
        parts.push(format!("min_len={v}"));
    }
    if let Some(v) = c.max_len {
        parts.push(format!("max_len={v}"));
    }
    if let Some(v) = &c.pattern {
        parts.push(format!("pattern={v:?}"));
    }
    if let Some(v) = &c.format {
        parts.push(format!("format={v:?}"));
    }
    if let Some(v) = &c.members {
        parts.push(format!("enum={}", v.join("|")));
    }
    if let Some(v) = c.closed {
        parts.push(format!("closed={v}"));
    }
    if c.has_computed {
        parts.push("computed".to_string());
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    }
}

fn row_for_field(module: &Module, path: &str, field: &SchemaField) -> Row {
    let blob = matches!(field.typ, crate::types::ActiveType::Blob).then(|| {
        (
            literal_int(module, field.blob_max_size),
            literal_string(module, field.blob_media_type),
        )
    });
    let blob_dynamic = field
        .blob_max_size
        .is_some_and(|id| literal_int(module, Some(id)).is_none())
        || field
            .blob_media_type
            .is_some_and(|id| literal_string(module, Some(id)).is_none());
    Row {
        path: path.to_string(),
        signature: field.typ.name(),
        constraints: constraints_of(module, field),
        required: field_required(field),
        removed_in: field.removed_in.clone(),
        blob,
        blob_dynamic,
    }
}

/// Collect every schema entry (tables and arrays included) as dotted
/// rows in source order.
fn collect_rows(module: &Module, root: crate::ir::NodeId, prefix: &str, out: &mut Vec<Row>) {
    let Some(Node::SchemaTable { fields, .. }) = module.get(root) else {
        return;
    };
    for (key, id) in fields {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match module.get(*id) {
            Some(Node::SchemaField { field }) => out.push(row_for_field(module, &path, field)),
            Some(Node::SchemaTable { closed, .. }) => {
                out.push(Row {
                    path: path.clone(),
                    signature: "Table".to_string(),
                    // The `closed` flag lives on the table node; a
                    // false->true flip rejects previously valid keys
                    // (LANG-B-004).
                    constraints: Constraints {
                        closed: Some(*closed),
                        ..Constraints::default()
                    },
                    required: true,
                    removed_in: None,
                    blob: None,
                    blob_dynamic: false,
                });
                collect_rows(module, *id, &path, out);
            }
            Some(Node::SchemaArray { element }) => {
                out.push(Row {
                    path: path.clone(),
                    signature: format!("Array<{}>", node_signature(module, *element)),
                    constraints: Constraints::default(),
                    required: true,
                    removed_in: None,
                    blob: None,
                    blob_dynamic: false,
                });
                // Every element shape is addressable: tables expand to
                // fields, nested arrays recurse, and a scalar element is
                // a row at `path[]` (this is how Array<Blob> bounds and
                // media changes become visible).
                collect_element_rows(module, *element, &format!("{path}[]"), out);
            }
            _ => {}
        }
    }
}

fn collect_fields(module: &Module, root: crate::ir::NodeId) -> Vec<Row> {
    let mut out = Vec::new();
    collect_rows(module, root, "", &mut out);
    out
}

/// Rows inside an array element. Table elements expand to
/// `path[].field`; scalar elements (including blobs) become one row at
/// `path[]`; nested arrays recurse with another `[]` segment.
fn collect_element_rows(
    module: &Module,
    element: crate::ir::NodeId,
    prefix: &str,
    out: &mut Vec<Row>,
) {
    match module.get(element) {
        Some(Node::SchemaTable { .. }) => collect_rows(module, element, prefix, out),
        Some(Node::SchemaArray { element: inner }) => {
            out.push(Row {
                path: prefix.to_string(),
                signature: format!("Array<{}>", node_signature(module, *inner)),
                constraints: Constraints::default(),
                required: true,
                removed_in: None,
                blob: None,
                blob_dynamic: false,
            });
            collect_element_rows(module, *inner, &format!("{prefix}[]"), out);
        }
        Some(Node::SchemaField { field }) => out.push(row_for_field(module, prefix, field)),
        _ => {}
    }
}

fn schema_meta(module: &Module) -> crate::ir::SchemaMeta {
    match module.schema.and_then(|root| module.get(root)) {
        Some(Node::SchemaTable { meta, .. }) => meta.clone(),
        _ => crate::ir::SchemaMeta::default(),
    }
}

fn literal_int(module: &Module, id: Option<crate::ir::NodeId>) -> Option<i64> {
    match id.and_then(|node| module.get(node)) {
        Some(Node::Literal(crate::ir::Value::Int(n))) => Some(*n),
        _ => None,
    }
}

fn literal_string(module: &Module, id: Option<crate::ir::NodeId>) -> Option<String> {
    match id.and_then(|node| module.get(node)) {
        Some(Node::Literal(crate::ir::Value::String(s))) => Some(s.clone()),
        _ => None,
    }
}

/// Compare two documents' schema blocks. Deterministic: fields walk in
/// source order, versions compare numerically, findings emit in a
/// fixed order (removed, added, changed, version gates).
pub fn compare(old: &Module, new: &Module) -> Result<Vec<CompatFinding>, String> {
    let old_root = old
        .schema
        .ok_or_else(|| "old document has no schema block".to_string())?;
    let new_root = new
        .schema
        .ok_or_else(|| "new document has no schema block".to_string())?;
    let old_fields = collect_fields(old, old_root);
    let new_fields = collect_fields(new, new_root);
    // Index by path: comparing two n-row schemas is O(n), not O(n²).
    let old_by_path: HashMap<&str, &Row> = old_fields
        .iter()
        .map(|row| (row.path.as_str(), row))
        .collect();
    let new_by_path: HashMap<&str, &Row> = new_fields
        .iter()
        .map(|row| (row.path.as_str(), row))
        .collect();

    let mut findings = Vec::new();

    // Removed fields: breaking unless the removal was scheduled at or
    // before the new version (removed_in <= new version).
    let new_version = schema_meta(new).version;
    for old_field in &old_fields {
        if new_by_path.contains_key(old_field.path.as_str()) {
            continue;
        }
        let scheduled = match (&old_field.removed_in, &new_version) {
            (Some(removed_in), Some(current)) => {
                match (parse_version(removed_in), parse_version(current)) {
                    (Some(r), Some(c)) => r <= c,
                    // Unparseable versions fail safe: scheduled only
                    // when both sides parse and order correctly.
                    _ => false,
                }
            }
            _ => false,
        };
        let path = &old_field.path;
        if scheduled {
            findings.push(CompatFinding::info(
                code::REMOVED_FIELD,
                path,
                format!("field '{path}' removed on schedule (removed_in matches new version)"),
            ));
        } else {
            findings.push(CompatFinding::error(
                code::REMOVED_FIELD,
                path,
                format!("field '{path}' removed: old readers and writers lose data"),
            ));
        }
    }

    // Added fields, type changes, and blob tightening.
    for new_field in &new_fields {
        match old_by_path.get(new_field.path.as_str()).copied() {
            None => {
                let path = &new_field.path;
                if new_field.required {
                    findings.push(CompatFinding::error(
                        code::ADDED_REQUIRED,
                        path,
                        format!(
                            "field '{path}' added without a default: old documents fail the new schema"
                        ),
                    ));
                } else {
                    findings.push(CompatFinding::info(
                        code::ADDED_REQUIRED,
                        path,
                        format!(
                            "field '{path}' added with a default or Null type: old documents still validate in resilient mode; strict mode still requires the value explicitly (T004)"
                        ),
                    ));
                }
            }
            Some(old_field) => {
                let path = &new_field.path;
                if old_field.signature != new_field.signature {
                    findings.push(CompatFinding::error(
                        code::TYPE_CHANGED,
                        path,
                        format!(
                            "field '{path}' type changed from {} to {}",
                            old_field.signature, new_field.signature
                        ),
                    ));
                }
                // These checks are INDEPENDENT (LANG-B-029): the old
                // else-if chain reported only the first change and hid
                // the rest (a simultaneous requiredness + blob change
                // dropped the blob finding entirely).
                if tightened(&old_field.constraints, &new_field.constraints) {
                    findings.push(CompatFinding::error(
                        code::TYPE_CHANGED,
                        path,
                        format!(
                            "field '{path}' constraints tightened ({} -> {}): documents that validated before may fail now",
                            describe_constraints(&old_field.constraints),
                            describe_constraints(&new_field.constraints)
                        ),
                    ));
                }
                if old_field.required != new_field.required && new_field.required {
                    // Removing a default makes the field required
                    // without changing its type: old documents that
                    // omitted it now fail.
                    findings.push(CompatFinding::error(
                        code::TYPE_CHANGED,
                        path,
                        format!(
                            "field '{path}' became required (default removed): old documents fail the new schema"
                        ),
                    ));
                }
                if let (Some((old_max, old_media)), Some((new_max, new_media))) =
                    (&old_field.blob, &new_field.blob)
                {
                    if old_max.is_some() || new_max.is_some() || old_media != new_media {
                        check_blob_tightening(
                            path,
                            old_max,
                            old_media,
                            new_max,
                            new_media,
                            &mut findings,
                        );
                    }
                }
                if old_field.blob_dynamic || new_field.blob_dynamic {
                    findings.push(CompatFinding::info(
                        code::TYPE_CHANGED,
                        path,
                        format!(
                            "field '{path}' blob bounds are computed; compatibility cannot compare them"
                        ),
                    ));
                }
            }
        }
    }

    // Version gates from schema metadata.
    check_version_gates(old, new, &mut findings);

    // Messages echo schema prose (versions, media types); redact with
    // both documents' material sets so neither side can leak a copy.
    let old_profile = crate::secret::SecretProfile::analyze(old);
    let new_profile = crate::secret::SecretProfile::analyze(new);
    for finding in &mut findings {
        if let Some(text) = old_profile.redact_authored_text(&finding.message) {
            finding.message = text;
        }
        if let Some(text) = new_profile.redact_authored_text(&finding.message) {
            finding.message = text;
        }
    }

    Ok(findings)
}

/// Blob constraint tightening is breaking for existing payloads:
/// `max_size` lowered or `media_type` changed (or newly pinned).
fn check_blob_tightening(
    path: &str,
    old_max: &Option<i64>,
    old_media: &Option<String>,
    new_max: &Option<i64>,
    new_media: &Option<String>,
    findings: &mut Vec<CompatFinding>,
) {
    if let (Some(old), Some(new)) = (old_max, new_max) {
        if new < old {
            findings.push(CompatFinding::error(
                code::TYPE_CHANGED,
                path,
                format!("blob '{path}' max_size tightened from {old} to {new}: existing payloads may fail"),
            ));
        }
    } else if old_max.is_none() && new_max.is_some() {
        // Newly pinning a bound is a tightening: payloads that were
        // acceptable before may now fail.
        let new = new_max.unwrap_or(0);
        findings.push(CompatFinding::error(
            code::TYPE_CHANGED,
            path,
            format!("blob '{path}' max_size newly pinned to {new}: existing payloads may fail"),
        ));
    }
    if old_media != new_media {
        findings.push(CompatFinding::error(
            code::TYPE_CHANGED,
            path,
            format!(
                "blob '{path}' media type changed from {} to {}",
                old_media.as_deref().unwrap_or("(unset)"),
                new_media.as_deref().unwrap_or("(unset)"),
            ),
        ));
    }
}

/// Version gates: the new version must not regress, and `min_reader`
/// must not leapfrog the old version (that orphans old readers).
fn check_version_gates(old: &Module, new: &Module, findings: &mut Vec<CompatFinding>) {
    let old_meta = schema_meta(old);
    let new_meta = schema_meta(new);
    if let (Some(old_v), Some(new_v)) = (&old_meta.version, &new_meta.version) {
        if let (Some(o), Some(n)) = (parse_version(old_v), parse_version(new_v)) {
            if n < o {
                findings.push(CompatFinding::error(
                    code::VERSION_REGRESSED,
                    "schema.version",
                    format!("version regressed from {old_v} to {new_v}"),
                ));
            }
        }
    }
    if let (Some(old_v), Some(floor)) = (&old_meta.version, &new_meta.min_reader) {
        if let (Some(o), Some(f)) = (parse_version(old_v), parse_version(floor)) {
            if f > o {
                findings.push(CompatFinding::error(
                    code::READER_FLOOR_RAISED,
                    "schema.min_reader",
                    format!(
                        "min_reader {floor} exceeds old version {old_v}: old readers are dropped"
                    ),
                ));
            }
        }
    }
    if let (Some(old_v), Some(floor)) = (&old_meta.version, &new_meta.min_writer) {
        if let (Some(o), Some(f)) = (parse_version(old_v), parse_version(floor)) {
            if f > o {
                findings.push(CompatFinding::error(
                    code::READER_FLOOR_RAISED,
                    "schema.min_writer",
                    format!(
                        "min_writer {floor} exceeds old version {old_v}: old writers are dropped"
                    ),
                ));
            }
        }
    }
}
