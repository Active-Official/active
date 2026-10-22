//! Minimal diagnostic-driven patch helper (CHECKLIST F.003, PROPOSAL §6.3).
//!
//! Turns the mechanical subset of diagnostics into `@rename` / `@patch`
//! directives agents can apply without guessing: safe key-typo renames
//! and strict-explicit fills from static schema defaults. Everything
//! else yields no patch — the explainer (`explain.rs`) covers guidance
//! for the rest. Deterministic: diagnostic order in, directive order
//! out, or nothing at all.

use crate::diag::{Diagnostic, RepairKind};
use crate::ir::{Module, Node, NodeId, SchemaField};
use crate::Value;

/// The mechanical operation a suggestion performs. Serialized lowercase
/// (`rename`/`fill`) for machine consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PatchKind {
    Rename,
    Fill,
}

/// A single machine-readable repair directive. `directive` is the exact
/// `@rename`/`@patch` line; `kind` and `path` let agents route without
/// parsing it. Deterministic: diagnostic order in, directive order out.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PatchSuggestion {
    pub kind: PatchKind,
    pub path: String,
    pub directive: String,
}

/// Collect minimal repair directives for diagnostics that map exactly:
/// safe `KeyTypo` renames and missing-key `T004`s with a static literal
/// default (strict-explicit misses and requires-triggered misses alike).
/// Unknown shapes are skipped, never approximated.
pub fn suggest_patches(module: &Module, diags: &[Diagnostic]) -> Vec<String> {
    suggest_patches_structured(module, diags)
        .into_iter()
        .map(|s| s.directive)
        .collect()
}

/// Structured suggestions in diagnostic order. Same content as
/// [`suggest_patches`], shaped for agents to parse instead of scrape.
pub fn suggest_patches_structured(module: &Module, diags: &[Diagnostic]) -> Vec<PatchSuggestion> {
    let mut out = Vec::new();
    for d in diags {
        if let Some(repair) = d.repair.as_ref() {
            if !repair.safe {
                continue;
            }
            if matches!(repair.kind, RepairKind::KeyTypo) {
                if let Some((path, directive)) = rename_for(d, repair) {
                    out.push(PatchSuggestion {
                        kind: PatchKind::Rename,
                        path,
                        directive,
                    });
                }
            }
        }
        // Any missing-key T004 with a static literal default has one
        // mechanical fix: write the value out. `fill_for` re-verifies
        // the static-literal default itself and refuses deprecated
        // fields (omitting a deprecated key is the migration path, so
        // suggesting its re-insertion would contradict L005). Matching
        // is structural on code + schema — never on message text, so a
        // diagnostic rewording cannot silently disable suggestions.
        if d.code == "T004" {
            if let Some((path, directive)) = fill_for(module, d) {
                out.push(PatchSuggestion {
                    kind: PatchKind::Fill,
                    path,
                    directive,
                });
            }
        }
    }
    out
}

/// `@rename` for a safe typo: rename requests already carry full dotted
/// paths; segment-level suggestions rebuild from the diagnostic path.
/// Returns the renamed path plus the directive.
fn rename_for(d: &Diagnostic, repair: &crate::diag::Repair) -> Option<(String, String)> {
    if repair.original.contains('.') || repair.suggested.contains('.') {
        // Defensive only: every KeyTypo repair in the codebase carries
        // single segments, so this branch never fires today. Kept so
        // a future full-path repair still resolves instead of
        // vanishing silently.
        return Some((
            repair.original.clone(),
            format!("@rename(\"{}\", \"{}\")", repair.original, repair.suggested),
        ));
    }
    let path = d.path.as_ref()?;
    let parent = path.strip_suffix(&repair.original).and_then(|p| {
        let p = p.strip_suffix('.')?;
        if p.is_empty() {
            None
        } else {
            Some(p)
        }
    });
    match parent {
        Some(p) => Some((
            path.clone(),
            format!("@rename(\"{path}\", \"{p}.{s}\")", s = repair.suggested),
        )),
        None if path == &repair.original => Some((
            repair.original.clone(),
            format!("@rename(\"{}\", \"{}\")", repair.original, repair.suggested),
        )),
        None => None,
    }
}

/// `@patch` with the schema's static default literal for a missing-key
/// T004 (strict-explicit or requires-triggered alike). Dynamic defaults
/// (expressions, secrets) have no source-text form and yield no patch;
/// deprecated fields yield no patch either, since omitting them is the
/// migration path. Returns the filled path plus directive.
fn fill_for(module: &Module, d: &Diagnostic) -> Option<(String, String)> {
    let path = d.path.as_ref()?;
    if path.contains('[') {
        return None;
    }
    let field = schema_field_at(module, path)?;
    if field.deprecated.is_some() {
        return None;
    }
    if !matches!(
        field.default.and_then(|id| module.get(id)),
        Some(Node::Literal(Value::String(_)))
            | Some(Node::Literal(Value::Int(_)))
            | Some(Node::Literal(Value::Float(_)))
            | Some(Node::Literal(Value::Bool(_)))
            | Some(Node::Literal(Value::Null))
    ) {
        return None;
    }
    let value = match field.default.and_then(|id| module.get(id)) {
        // Source-text escaping for the directive: backslashes first,
        // then quotes and control characters. Anything composite
        // (tables, arrays, quantities, durations) has no one-line
        // source form here and yields no patch by design.
        Some(Node::Literal(Value::String(s))) => {
            let mut out = String::with_capacity(s.len() + 2);
            out.push('"');
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
        Some(Node::Literal(Value::Int(i))) => i.to_string(),
        // Debug formatting preserves the decimal point, so 1.0 never
        // degrades into the Int literal 1.
        Some(Node::Literal(Value::Float(f))) => format!("{f:?}"),
        Some(Node::Literal(Value::Bool(b))) => b.to_string(),
        Some(Node::Literal(Value::Null)) => "null".to_string(),
        _ => return None,
    };
    Some((path.clone(), format!("@patch(\"{path}\", {value})")))
}

/// Schema field at a dotted value path (tables transparent, like the
/// generators' nominal walk). Array indices and unknown segments yield
/// nothing — no patch is better than a wrong one.
fn schema_field_at<'m>(module: &'m Module, path: &str) -> Option<&'m SchemaField> {
    let root = module.schema?;
    let mut id: NodeId = root;
    let mut last_field: Option<&'m SchemaField> = None;
    for seg in path.split('.') {
        let node = module.get(id)?;
        let fields = match node {
            Node::SchemaTable { fields, .. } => fields,
            Node::SchemaArray { element } => {
                id = *element;
                let node = module.get(id)?;
                match node {
                    Node::SchemaTable { fields, .. } => fields,
                    Node::SchemaField { field } => return Some(field),
                    _ => return None,
                }
            }
            Node::SchemaField { .. } => {
                // A leaf field cannot be descended into; table-typed
                // fields surface as SchemaTable nodes, not here.
                return None;
            }
            _ => return None,
        };
        let child = fields.iter().find(|(k, _)| k.as_str() == seg)?;
        id = *child.1;
        if let Some(Node::SchemaField { field }) = module.get(id) {
            last_field = Some(field);
        } else {
            last_field = None;
        }
    }
    last_field
}
