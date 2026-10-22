//! Deterministic failure explainer (CHECKLIST F.004, PROPOSAL §6.3).
//!
//! Maps diagnostic codes plus their spans to stable, value-free repair
//! guidance. No models, no heuristics, no randomness: the same code
//! always yields the same text, so agents can cache and quote it.
//! Guidance names the repair direction (fix the value, add the key,
//! loosen nothing silently); the `patch` surface turns the mechanical
//! subset into `@patch`/`@rename` directives.

/// Stable one-paragraph repair guidance for a diagnostic code.
/// Unknown codes fall back to a generic pointer instead of inventing
/// advice — the code itself stays the machine-readable contract.
pub fn explain_code(code: &str) -> &'static str {
    match code {
        "E001" => "Expected a different token here: the document structure is incomplete or a delimiter is missing. Compare the span against the grammar (tables, brackets, commas) and close what is open.",
        "E002" => "Unexpected token at this position: something appears where the grammar allows nothing. Remove it or move it into the table it belongs to; do not quote it to silence the parser.",
        "E003" => "Malformed secret.env form: secret.env() takes a single string-literal name. Fix the call shape at the span; any other value form is rejected here.",
        "E004" => "Nesting cap exceeded: the parser allows 64 levels of expression nesting and 64 levels of schema nesting, fixed (no flag raises either). Split the expression with let bindings and flatten nested schemas.",
        "E006" => "Tab in leading whitespace: Active indents with spaces only. Replace the tab with spaces matching the surrounding level.",
        "E007" => "Inconsistent dedentation: the dedent does not match any open indent level. Align it with a level that is actually open.",
        "E011" => "Maximum block nesting depth (64) exceeded: flatten the document structure. The cap is fixed by the parser.",
        "E019" => "Patch failure. At parse time: a patch directive outside the document root, a missing paren or non-literal path, an interpolated path, an existing rename target, or an overlap with an immutable path. At apply time: the target changed under the patch or two patches collide. Fix the directive shape and rebase onto current paths.",
        "E020" => "Bad schema type expression at the span: unknown type or argument, malformed Array<T> / Enum(...) shape, non-boolean closed flag, or a schema block where schemas are not allowed (e.g. inside profiles). Fix the type expression.",
        "E023" => "Integer out of range: positive literals saturate at i64::MAX, and negating the minimum yields MIN+1. Use a smaller magnitude or a different unit.",
        "E032" => "Signature or trust failure: the sidecar is missing, mismatched, or untrusted. Verify against the trust file for the recorded key id; never bypass verification to make it pass.",
        "T001" => "Unit or quantity violation at the span: an unsupported operator or dimension mismatch. Fix the unit expression; the code names the exact mismatch.",
        "T002" => "Type mismatch: the value's type does not satisfy the schema or operator at the span. Change the value to the expected type (or widen the schema deliberately), not the other way around.",
        "T003" => "Schema or format violation: the value breaks a constraint (format, pattern, range, membership). Fix the VALUE to a well-formed one, or correct the schema if the constraint itself is wrong — never delete the constraint to silence it. Unknown format names are schema bugs: use email|uri|uuid|semver.",
        "T004" => "Missing required key: add the named key. When the message names a trigger (`required by present`), the key is conditional on that trigger — set it or unset the trigger, but do not remove the requires rule.",
        "T005" => "Unexpected key in a closed table: remove the key, move it to the table it belongs to, or follow the did-you-mean suggestion (`@rename`). Opening the table trades a typo guard for silence.",
        "W004" => "Patch target warning: a @remove/@rename names a path that does not exist. Nothing was changed; check the path spelling.",
        "W005" => "Resilient default fill (informational): the engine inserted a schema default for a missing key. Harmless in resilient mode; strict mode requires the key written out.",
        "H005" => "Unknown key with a near-identical known key: apply the did-you-mean suggestion (`@rename`) when it matches intent.",
        "S001" => "Dependency cycle: the listed keys depend on each other and no evaluation order exists. Break the cycle by removing one dependency edge.",
        _ => "No canned guidance for this code: the code and span are the contract — read the message, fix the span it names, and re-run check.",
    }
}

/// One stable guidance line per diagnostic: `CODE path span: guidance`.
/// Message text never enters the output (value-free by construction).
pub fn explain_all(items: &[(String, Option<String>, Option<String>)]) -> Vec<String> {
    items
        .iter()
        .map(|(code, path, span)| {
            let where_ = match (path, span) {
                (Some(p), Some(s)) => format!("{p} {s}"),
                (Some(p), None) => p.clone(),
                (None, Some(s)) => s.clone(),
                (None, None) => "document".to_string(),
            };
            format!("{code} {where_}: {}", explain_code(code))
        })
        .collect()
}
