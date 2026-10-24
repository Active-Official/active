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
    required: bool,
    removed_in: Option<String>,
    blob: Option<(Option<i64>, Option<String>)>,
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

fn row_for_field(module: &Module, path: &str, field: &SchemaField) -> Row {
    let blob = matches!(field.typ, crate::types::ActiveType::Blob).then(|| {
        (
            literal_int(module, field.blob_max_size),
            literal_string(module, field.blob_media_type),
        )
    });
    Row {
        path: path.to_string(),
        signature: field.typ.name(),
        required: field_required(field),
        removed_in: field.removed_in.clone(),
        blob,
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
            Some(Node::SchemaTable { .. }) => {
                out.push(Row {
                    path: path.clone(),
                    signature: "Table".to_string(),
                    required: true,
                    removed_in: None,
                    blob: None,
                });
                collect_rows(module, *id, &path, out);
            }
            Some(Node::SchemaArray { element }) => {
                out.push(Row {
                    path: path.clone(),
                    signature: format!("Array<{}>", node_signature(module, *element)),
                    required: true,
                    removed_in: None,
                    blob: None,
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
                required: true,
                removed_in: None,
                blob: None,
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

    let mut findings = Vec::new();

    // Removed fields: breaking unless the removal was scheduled at or
    // before the new version (removed_in <= new version).
    let new_version = schema_meta(new).version;
    for old_field in &old_fields {
        if new_fields.iter().any(|row| row.path == old_field.path) {
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
        match old_fields.iter().find(|row| row.path == new_field.path) {
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
                            "field '{path}' added with a default or Null type: old documents still validate"
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
                } else if old_field.required != new_field.required && new_field.required {
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
                } else if let (Some((old_max, old_media)), Some((new_max, new_media))) =
                    (&old_field.blob, &new_field.blob)
                {
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
        }
    }

    // Version gates from schema metadata.
    check_version_gates(old, new, &mut findings);

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
