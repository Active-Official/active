//! Debugger primitives: breakpoints, deterministic replay, and
//! minimal failing-case reduction (TOOLING DEBUG.001-005).
//!
//! Everything here is a pure function of the shipped evaluator and the
//! step trace from `Engine::evaluate_with_steps`. Values are redacted by
//! construction; breakpoints select, never reveal, secret material.

use serde_json::{json, Value as JsonValue};

use crate::eval::StepEvent;
use crate::{Engine, Module};

/// First step index matching a breakpoint selector:
/// `taint`, `path:<prefix>`, `code:<CODE>`, or `node:<n>` (0-based
/// topological index). Unknown selectors match nothing.
pub fn breakpoint_index(steps: &[StepEvent], selector: &str) -> Option<usize> {
    if selector == "taint" {
        return steps.iter().position(|step| step.tainted);
    }
    if let Some(prefix) = selector.strip_prefix("path:") {
        return steps
            .iter()
            .position(|step| step.path.as_deref().is_some_and(|p| p.starts_with(prefix)));
    }
    if let Some(code) = selector.strip_prefix("code:") {
        return steps
            .iter()
            .position(|step| step.codes.iter().any(|c| c == code));
    }
    if let Some(index) = selector
        .strip_prefix("node:")
        .and_then(|n| n.parse::<usize>().ok())
    {
        return (index < steps.len()).then_some(index);
    }
    None
}

/// Deterministic replay: evaluate twice and compare the redacted
/// content hash, the diagnostic code lists, and the step traces. When
/// `expected` is given, the first hash must equal it (the audit
/// trail's `result` value).
pub fn replay(module: &Module, expected: Option<&str>) -> Result<JsonValue, String> {
    let (first, first_diags, first_steps) = Engine::new()
        .evaluate_with_steps(module)
        .map_err(|e| e.to_string())?;
    let (second, second_diags, second_steps) = Engine::new()
        .evaluate_with_steps(module)
        .map_err(|e| e.to_string())?;

    let first_hash = crate::provenance::content_hash(&first);
    let second_hash = crate::provenance::content_hash(&second);
    let first_codes: Vec<&str> = first_diags.iter().map(|d| d.code.as_str()).collect();
    let second_codes: Vec<&str> = second_diags.iter().map(|d| d.code.as_str()).collect();
    let steps_match =
        serde_json::to_string(&first_steps).ok() == serde_json::to_string(&second_steps).ok();
    let expected_ok = expected.is_none_or(|text| text == first_hash);
    let ok = first_hash == second_hash && first_codes == second_codes && steps_match && expected_ok;

    Ok(json!({
        "ok": ok,
        "hash": first_hash,
        "second_hash": second_hash,
        "codes": first_codes,
        "steps": first_steps.len(),
        "steps_match": steps_match,
        "expected": expected,
        "expected_match": expected_ok,
    }))
}

/// Reduce a failing document by deleting lines while the target
/// diagnostic code survives. Deterministic (top-down, first productive
/// deletion wins), bounded by `max_attempts` parse/evaluate cycles.
/// The reduced source is always a subsequence of the original lines.
pub fn reduce(source: &str, target_code: &str, max_attempts: usize) -> (String, usize) {
    let mut lines: Vec<String> = source.split('\n').map(str::to_string).collect();
    let mut attempts = 0usize;
    let mut progressed = true;
    while progressed && attempts < max_attempts && lines.len() > 1 {
        progressed = false;
        let mut index = 0usize;
        while index < lines.len() {
            if attempts >= max_attempts {
                break;
            }
            let mut trial = lines.clone();
            trial.remove(index);
            let candidate = trial.join("\n");
            attempts += 1;
            if still_fails(&candidate, target_code) {
                lines = trial;
                progressed = true;
            } else {
                index += 1;
            }
        }
    }
    (lines.join("\n"), attempts)
}

/// Whether `text` still produces `target_code` through the shipped
/// parse/evaluate path (parse diagnostics and terminal error codes
/// included).
fn still_fails(text: &str, target_code: &str) -> bool {
    match crate::parse(text, "reduce") {
        Ok(module) => match Engine::new().evaluate(&module) {
            Ok((_, diagnostics)) => diagnostics.iter().any(|d| d.code == target_code),
            Err(error) => error.code() == target_code,
        },
        Err(diagnostics) => diagnostics.iter().any(|d| d.code == target_code),
    }
}

/// Minimal-failing-case export: the reduced source plus the metadata a
/// regression needs (target code, attempts, sizes). Callers add the
/// budget/profile/mode context they ran under.
pub fn minimal_case(source: &str, target_code: &str, max_attempts: usize) -> JsonValue {
    let (reduced, attempts) = reduce(source, target_code, max_attempts);
    json!({
        "target_code": target_code,
        "attempts": attempts,
        "original_bytes": source.len(),
        "reduced_bytes": reduced.len(),
        "reduced": reduced,
    })
}

/// Human rendering of a step trace (values redacted).
pub fn render_steps(steps: &[StepEvent]) -> String {
    let mut out = String::new();
    for (index, step) in steps.iter().enumerate() {
        out.push_str(&format!(
            "#{index} node{} {}: {} = {}{}{}\n",
            step.node,
            step.path.as_deref().unwrap_or("(internal)"),
            step.kind,
            serde_json::to_string(&step.value).unwrap_or_else(|_| "null".into()),
            if step.codes.is_empty() {
                String::new()
            } else {
                format!(" codes={:?}", step.codes)
            },
            if step.tainted { " tainted" } else { "" },
        ));
    }
    out
}
