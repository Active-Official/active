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
/// parsing it. `from`/`to` carry the rename endpoints (None for fills)
/// so planners can reason about blast radius without scraping the
/// directive string. Deterministic: diagnostic order in, directive
/// order out.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PatchSuggestion {
    pub kind: PatchKind,
    pub path: String,
    pub directive: String,
    pub from: Option<String>,
    pub to: Option<String>,
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
    let profile = crate::secret::SecretProfile::analyze(module);
    suggest_patches_structured_with_profile(module, diags, &profile)
}

/// Structured suggestions under an already analyzed profile: an
/// evaluated caller passes the engine context so a default that copies
/// an environment or profile value is withheld too.
pub fn suggest_patches_structured_with_profile(
    module: &Module,
    diags: &[Diagnostic],
    profile: &crate::secret::SecretProfile,
) -> Vec<PatchSuggestion> {
    let mut out = Vec::new();
    for d in diags {
        if let Some(repair) = d.repair.as_ref() {
            if !repair.safe {
                continue;
            }
            if matches!(repair.kind, RepairKind::KeyTypo) {
                if let Some((path, from, to, directive)) = rename_for(d, repair) {
                    out.push(PatchSuggestion {
                        kind: PatchKind::Rename,
                        path,
                        directive,
                        from: Some(from),
                        to: Some(to),
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
            if let Some((path, directive)) = fill_for(module, profile, d) {
                out.push(PatchSuggestion {
                    kind: PatchKind::Fill,
                    path,
                    directive,
                    from: None,
                    to: None,
                });
            }
        }
    }
    out
}

/// `@rename` for a safe typo: rename requests already carry full dotted
/// paths; segment-level suggestions rebuild from the diagnostic path.
/// Returns the renamed path, the full from/to endpoints, and the
/// directive. Endpoints interpolate raw, like fill paths: both are
/// key segments, and key segments are lexer identifiers by
/// construction (ASCII alphanumerics plus underscore — never quotes,
/// backslashes, or control characters), so no breakout is
/// expressible. Values, which CAN carry arbitrary text, go through
/// `escape_directive_str` instead.
fn rename_for(
    d: &Diagnostic,
    repair: &crate::diag::Repair,
) -> Option<(String, String, String, String)> {
    // Returns (path, from, to, directive).
    let built = if repair.original.contains('.') || repair.suggested.contains('.') {
        // Defensive only: every KeyTypo repair in the codebase carries
        // single segments, so this branch never fires today. Kept so
        // a future full-path repair still resolves instead of
        // vanishing silently.
        Some((
            repair.original.clone(),
            repair.original.clone(),
            repair.suggested.clone(),
        ))
    } else {
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
                path.clone(),
                format!("{p}.{}", repair.suggested),
            )),
            None if path == &repair.original => Some((
                repair.original.clone(),
                repair.original.clone(),
                repair.suggested.clone(),
            )),
            None => None,
        }
    };
    let (path, from, to) = built?;
    // Never suggest a rename into or out of root metadata: the applier
    // rejects it (E019), and the plan must not claim an unappliable
    // step.
    if crate::eval::path_targets_document_metadata(&from)
        || crate::eval::path_targets_document_metadata(&to)
    {
        return None;
    }
    Some((
        path,
        from.clone(),
        to.clone(),
        format!("@rename(\"{from}\", \"{to}\")"),
    ))
}

/// Single-quoted source form: `${…}` stays literal. Only `\\` and `\'`
/// are escapes in this form; every other character is verbatim, so
/// callers must reject control characters first.
fn escape_single_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            c => out.push(c),
        }
    }
    out
}

/// Control characters outside the escape table (`\n`, `\r`, `\t`) have
/// no faithful Active source form.
fn has_unrepresentable_control(s: &str) -> bool {
    s.chars()
        .any(|c| (c as u32) < 0x20 && !matches!(c, '\n' | '\r' | '\t'))
}

/// Source-text escaping for directive strings: backslashes first,
/// then quotes and control characters. Shared by fill values and
/// rename endpoints so neither can break out of the directive string.
fn escape_directive_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
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
    out
}

/// `@patch` with the schema's static default literal for a missing-key
/// T004 (strict-explicit or requires-triggered alike). Dynamic defaults
/// (expressions, secrets) have no source-text form and yield no patch;
/// deprecated fields yield no patch either, since omitting them is the
/// migration path. Returns the filled path plus directive.
fn fill_for(
    module: &Module,
    profile: &crate::secret::SecretProfile,
    d: &Diagnostic,
) -> Option<(String, String)> {
    let path = d.path.as_ref()?;
    if path.contains('[') {
        return None;
    }
    // Root metadata is resolved from the source, never from a fill:
    // `@patch("active_mode", …)` is rejected with E019, so suggesting
    // it would produce a plan step that cannot apply.
    if crate::eval::path_targets_document_metadata(path) {
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
        // Anything composite (tables, arrays, quantities, durations)
        // has no one-line source form here and yields no patch by
        // design. Shared escaper with rename endpoints.
        Some(Node::Literal(Value::String(s))) => {
            // Test the raw text before escaping: escaping could hide
            // the match the directive would carry.
            if profile.blob_identity_is_material(s) {
                return None;
            }
            if has_unrepresentable_control(s) {
                return None;
            }
            if s.contains("${") {
                // A double-quoted directive would interpolate: single
                // quotes keep `${…}` literal (spec §2.5).
                if s.chars().any(|c| (c as u32) < 0x20) {
                    return None;
                }
                format!("'{}'", escape_single_quoted(s))
            } else {
                format!("\"{}\"", escape_directive_str(s))
            }
        }
        Some(Node::Literal(Value::Int(i))) => i.to_string(),
        // Debug formatting preserves the decimal point, so 1.0 never
        // degrades into the Int literal 1. Non-finite floats have no
        // Active source form (the lexer rejects them, and imports map
        // them to null with W010), so they yield no patch rather than
        // an unparseable directive.
        Some(Node::Literal(Value::Float(f))) => {
            if !f.is_finite() {
                return None;
            }
            format!("{f:?}")
        }
        Some(Node::Literal(Value::Bool(b))) => b.to_string(),
        Some(Node::Literal(Value::Null)) => "null".to_string(),
        _ => return None,
    };
    // Paths interpolate raw: `path` is built from schema field and
    // value key segments, which lex as identifiers (ASCII
    // alphanumerics plus underscore) — the applier's
    // `validate_patch_path` enforces the same shape, so quoting or
    // escaping here would produce directives the engine rejects.
    // Array paths never reach this point (rejected above).
    // A default that is or contains a material value would copy the
    // secret into a patch plan; suggest nothing instead.
    if profile.blob_identity_is_material(&value) {
        return None;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Severity;

    fn t004(path: &str) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            code: "T004".to_string(),
            message: format!("missing required key: {path}"),
            path: Some(path.to_string()),
            span: None,
            repair: None,
            confidence: 1.0,
            file: None,
        }
    }

    #[test]
    fn non_finite_float_default_yields_no_patch() {
        // The lexer rejects non-finite literals and imports map them to
        // null (W010), so no parser path can produce this module state —
        // perturb the parsed default to prove the guard fires anyway.
        // Positive control first: the finite default fills normally.
        let mut module = crate::parse("schema = { ratio = Float(default = 1.5) }\n", "fill-finite")
            .expect("must parse");
        assert!(fill_for(
            &module,
            &crate::secret::SecretProfile::analyze(&module),
            &t004("ratio")
        )
        .is_some());
        let default_id = schema_field_at(&module, "ratio")
            .expect("field present")
            .default
            .expect("default present");
        match module.get_mut(default_id).expect("default node") {
            Node::Literal(Value::Float(f)) => *f = f64::INFINITY,
            other => panic!("expected float default, got {other:?}"),
        }
        assert!(
            fill_for(
                &module,
                &crate::secret::SecretProfile::analyze(&module),
                &t004("ratio")
            )
            .is_none(),
            "inf default must not emit an unparseable directive"
        );
    }
}
