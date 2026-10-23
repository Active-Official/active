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

/// Collect top-level schema fields: (name, field) in source order.
fn collect_fields(module: &Module, root: crate::ir::NodeId) -> Vec<(String, SchemaField)> {
    match module.get(root) {
        Some(Node::SchemaTable { fields, .. }) => fields
            .iter()
            .filter_map(|(name, id)| match module.get(*id) {
                Some(Node::SchemaField { field }) => Some((name.clone(), field.clone())),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
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
    for (name, old_field) in &old_fields {
        if new_fields.iter().any(|(n, _)| n == name) {
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
        if scheduled {
            findings.push(CompatFinding::info(
                code::REMOVED_FIELD,
                name,
                format!("field '{name}' removed on schedule (removed_in matches new version)"),
            ));
        } else {
            findings.push(CompatFinding::error(
                code::REMOVED_FIELD,
                name,
                format!("field '{name}' removed: old readers and writers lose data"),
            ));
        }
    }

    // Added fields and type changes.
    for (name, new_field) in &new_fields {
        match old_fields.iter().find(|(n, _)| n == name) {
            None => {
                if field_required(new_field) {
                    findings.push(CompatFinding::error(
                        code::ADDED_REQUIRED,
                        name,
                        format!(
                            "field '{name}' added without a default: old documents fail the new schema"
                        ),
                    ));
                } else {
                    findings.push(CompatFinding::info(
                        code::ADDED_REQUIRED,
                        name,
                        format!(
                            "field '{name}' added with a default or Null type: old documents still validate"
                        ),
                    ));
                }
            }
            Some((_, old_field)) => {
                if old_field.typ.name() != new_field.typ.name() {
                    findings.push(CompatFinding::error(
                        code::TYPE_CHANGED,
                        name,
                        format!(
                            "field '{name}' type changed from {} to {}",
                            old_field.typ.name(),
                            new_field.typ.name()
                        ),
                    ));
                } else if matches!(new_field.typ, crate::types::ActiveType::Blob) {
                    check_blob_tightening(old, old_field, new, new_field, name, &mut findings);
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
    old_module: &Module,
    old_field: &SchemaField,
    new_module: &Module,
    new_field: &SchemaField,
    path: &str,
    findings: &mut Vec<CompatFinding>,
) {
    let old_max = literal_int(old_module, old_field.blob_max_size);
    let new_max = literal_int(new_module, new_field.blob_max_size);
    if let (Some(old), Some(new)) = (old_max, new_max) {
        if new < old {
            findings.push(CompatFinding::error(
                code::TYPE_CHANGED,
                path,
                format!("blob '{path}' max_size tightened from {old} to {new}: existing payloads may fail"),
            ));
        }
    }
    let old_media = literal_string(old_module, old_field.blob_media_type);
    let new_media = literal_string(new_module, new_field.blob_media_type);
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
