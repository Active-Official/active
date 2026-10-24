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
    let steps_match = first_steps == second_steps;
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
/// regression needs (target code, attempts, sizes). Authored
/// `secret("…")` literals are redacted in the exported text: the bundle
/// is display data, never a secret carrier.
pub fn minimal_case(source: &str, target_code: &str, max_attempts: usize) -> JsonValue {
    let (reduced, attempts) = reduce(source, target_code, max_attempts);
    let reduced_bytes = reduced.len();
    let reduced = redact_secret_literals(&reduced);
    json!({
        "target_code": target_code,
        "attempts": attempts,
        "original_bytes": source.len(),
        "reduced_bytes": reduced_bytes,
        "reduced": reduced,
    })
}

/// Replace secret-bearing string literals in exported source with a
/// fixed marker. Two passes:
///
/// 1. **Semantic** (recovering parse): every string literal that feeds a
///    `secret(...)` locator — including through a `let`/table binding —
///    is replaced by its source span.
/// 2. **Lexical fallback** (unparseable source): the lexer stream finds
///    `secret` followed by parentheses and a string token, handling
///    whitespace, nested parentheses, triple-quoted strings, and
///    unterminated strings.
///
/// `secret.env(NAME)` names are not material and pass through.
fn redact_secret_literals(source: &str) -> String {
    let mut spans: Vec<crate::diag::Span> = Vec::new();
    let (module, parse_diags) =
        crate::parse_recovering(source, "redact", crate::ParseOptions::default());
    for node in &module.arena {
        if let crate::ir::Node::Secret { locator } = node {
            let mut set = std::collections::HashSet::new();
            collect_for_redaction(&module, *locator, &mut set);
            for id in set {
                let is_string = matches!(
                    module.get(id),
                    Some(crate::ir::Node::Literal(crate::ir::Value::String(_)))
                        | Some(crate::ir::Node::Interpolate { .. })
                );
                if is_string {
                    if let Some(span) = module.node_spans.get(&id) {
                        spans.push(span.clone());
                    }
                }
            }
        }
    }
    // The lexical pass also covers malformed calls and recovering-parse
    // documents where the semantic pass cannot see a Secret node.
    // Overlaps are unioned below, so running both is safe.
    if !parse_diags.is_empty() || spans.is_empty() {
        spans.extend(lexical_secret_spans(source));
    }
    if spans.is_empty() {
        return source.to_string();
    }
    let mut ranges: Vec<(usize, usize)> = spans
        .iter()
        .filter_map(|span| span_offsets(source, span))
        .collect();
    ranges.sort_unstable();
    // Coalesce overlapping/adjacent ranges (an interpolated string
    // contains its literal parts; replacing both with stale offsets
    // would corrupt the text and can split a UTF-8 boundary).
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if start >= end || end > source.len() {
            continue;
        }
        if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
            continue;
        }
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    let mut out = source.to_string();
    for (start, end) in merged.iter().rev() {
        out.replace_range(*start..*end, "\"***SECRET***\"");
    }
    out
}

/// Collect a locator subtree (following references) for redaction.
fn collect_for_redaction(
    module: &crate::Module,
    root: crate::NodeId,
    set: &mut std::collections::HashSet<crate::NodeId>,
) {
    use crate::ir::Node;
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !set.insert(id) {
            continue;
        }
        match module.get(id) {
            Some(Node::Ref(target)) => stack.push(*target),
            Some(Node::ApplyConstraint { target, .. }) => stack.push(*target),
            Some(Node::Interpolate { parts }) => {
                for part in parts {
                    if let crate::ir::InterpPart::Expr(expr) = part {
                        stack.push(*expr);
                    }
                }
            }
            Some(Node::Binary { lhs, rhs, .. }) => {
                stack.push(*lhs);
                stack.push(*rhs);
            }
            Some(Node::Unary { expr, .. }) => stack.push(*expr),
            Some(Node::Array { elements }) => stack.extend(elements.iter().copied()),
            Some(Node::Table { fields }) => stack.extend(fields.values().copied()),
            Some(Node::Quantity { value, .. }) => stack.push(*value),
            Some(Node::Semantic { inner, .. }) => stack.push(*inner),
            Some(Node::ContentAddressed { media_type, data }) => {
                stack.push(*media_type);
                stack.push(*data);
            }
            Some(Node::Env {
                default: Some(default),
                ..
            }) => stack.push(*default),
            Some(Node::Secret { locator }) => stack.push(*locator),
            _ => {}
        }
    }
}

/// Lexer-based fallback: from every `secret` keyword, find the call's
/// opening parenthesis (tolerating malformed punctuation) and redact
/// every string token up to its matching close parenthesis. This covers
/// `secret ("x")`, `secret(("x"))`, triple-quoted strings, extra
/// arguments, and unterminated strings (the lexer heals them).
fn lexical_secret_spans(source: &str) -> Vec<crate::diag::Span> {
    use crate::parser::TokenKind as K;
    let mut lexer = crate::parser::Lexer::new(source);
    let lexed = lexer.tokenize();
    let tokens = &lexed.tokens;
    let mut spans = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        let is_secret =
            matches!(tokens[index].kind, K::Secret | K::Ident) && tokens[index].text == "secret";
        if !is_secret {
            index += 1;
            continue;
        }
        // Allow a bounded number of newlines between the keyword and
        // `(` (a line break is legal whitespace), but require either a
        // run of `(` or nothing else. `secret.env("NAME")` is a name,
        // not material, so a `.` before `env` skips the call entirely.
        let mut open = None;
        let mut probe = index + 1;
        let mut newlines = 0usize;
        while probe < tokens.len() {
            match tokens[probe].kind {
                K::Newline => {
                    newlines += 1;
                    if newlines > 2 {
                        break;
                    }
                    probe += 1;
                }
                K::LParen => {
                    open = Some(probe);
                    break;
                }
                K::Dot => break,
                _ => break,
            }
        }
        if open.is_none()
            && matches!(tokens.get(index + 1).map(|t| &t.kind), Some(K::Dot))
            && matches!(tokens.get(index + 2).map(|t| &t.kind), Some(K::Env))
        {
            // `secret.env(NAME)`: variable names are metadata, not
            // secret material.
            index += 3;
            continue;
        }
        let Some(open) = open else {
            index += 1;
            continue;
        };
        let mut depth = 0i32;
        let mut cursor = open;
        while cursor < tokens.len() {
            match tokens[cursor].kind {
                K::LParen => depth += 1,
                K::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                K::String(_) | K::SingleString | K::MlString(_) | K::MlSingleString => {
                    let token = &tokens[cursor];
                    spans.push(crate::diag::Span {
                        line: token.line,
                        column: token.col,
                        end_line: token.end_line,
                        end_column: token.end_col,
                    });
                }
                _ => {}
            }
            cursor += 1;
        }
        index = cursor.max(index + 1);
    }
    spans
}

/// Convert a 1-based core span into byte offsets in `text`.
fn span_offsets(text: &str, span: &crate::diag::Span) -> Option<(usize, usize)> {
    let line_starts = line_byte_starts(text);
    let start = byte_at(&line_starts, text, span.line, span.column)?;
    let end = byte_at(&line_starts, text, span.end_line, span.end_column)?;
    (end > start).then_some((start, end))
}

fn line_byte_starts(text: &str) -> Vec<usize> {
    // The lexer skips a leading BOM, so line-1 columns count from after
    // it; start the first line at the BOM's byte length.
    let bom = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let mut starts = vec![bom];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn byte_at(starts: &[usize], text: &str, line: u32, column: u32) -> Option<usize> {
    let start = *starts.get(line.checked_sub(1)? as usize)?;
    let rest = &text[start..];
    let line_text = rest.split('\n').next().unwrap_or("");
    let target = column.checked_sub(1)? as usize;
    let mut seen = 0usize;
    for (offset, ch) in line_text.char_indices() {
        if seen == target {
            return Some(start + offset);
        }
        seen += 1;
        let _ = ch;
    }
    (seen == target).then_some(start + line_text.len())
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
