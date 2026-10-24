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
    reduce_with(source, target_code, max_attempts, None)
}

/// File-aware reduction: `file_ctx` enables include resolution so a
/// target code that lives in an included file is reproducible.
pub fn reduce_with(
    source: &str,
    target_code: &str,
    max_attempts: usize,
    file_ctx: Option<&std::path::Path>,
) -> (String, usize) {
    let protected = secret_call_lines(source);
    // Keep the protected flag attached to the line itself: line
    // numbers shift as lines are removed, so an index set would drift.
    let mut lines: Vec<(String, bool)> = source
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            let keep = protected.contains(&((index + 1) as u32));
            (line.to_string(), keep)
        })
        .collect();
    let mut attempts = 0usize;
    let mut progressed = true;
    while progressed && attempts < max_attempts && lines.len() > 1 {
        progressed = false;
        let mut index = 0usize;
        while index < lines.len() {
            // Never delete a line that participates in a secret call:
            // removing the use would strand its literal outside any
            // redactable context (a leak in the exported bundle).
            if lines[index].1 {
                index += 1;
                continue;
            }
            if attempts >= max_attempts {
                break;
            }
            let mut trial = lines.clone();
            trial.remove(index);
            let candidate = trial
                .iter()
                .map(|(line, _)| line.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            attempts += 1;
            if still_fails(&candidate, target_code, file_ctx) {
                lines = trial;
                progressed = true;
            } else {
                index += 1;
            }
        }
    }
    (
        lines
            .iter()
            .map(|(line, _)| line.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        attempts,
    )
}

/// Whether `text` still produces `target_code` through the shipped
/// parse/evaluate path (parse diagnostics and terminal error codes
/// included).
fn still_fails(text: &str, target_code: &str, file_ctx: Option<&std::path::Path>) -> bool {
    let parsed = match file_ctx {
        Some(path) => {
            let options = crate::ParseOptions {
                enable_includes: true,
                ..Default::default()
            };
            crate::parse_text_recovering_with_file(text, &path.to_string_lossy(), options, path)
        }
        None => match crate::parse(text, "reduce") {
            Ok(module) => (module, Vec::new()),
            Err(diagnostics) => {
                return diagnostics.iter().any(|d| d.code == target_code);
            }
        },
    };
    let (module, parse_diags) = parsed;
    if parse_diags.iter().any(|d| d.code == target_code) {
        return true;
    }
    match Engine::new().evaluate(&module) {
        Ok((_, diagnostics)) => diagnostics.iter().any(|d| d.code == target_code),
        Err(error) => error.code() == target_code,
    }
}

/// Minimal-failing-case export: the reduced source plus the metadata a
/// regression needs (target code, attempts, sizes). Authored
/// `secret("…")` literals are redacted in the exported text: the bundle
/// is display data, never a secret carrier.
pub fn minimal_case(source: &str, target_code: &str, max_attempts: usize) -> JsonValue {
    minimal_case_with(source, target_code, max_attempts, None)
}

/// File-aware minimal case (includes resolve, so included targets are
/// reproducible).
pub fn minimal_case_with(
    source: &str,
    target_code: &str,
    max_attempts: usize,
    file_ctx: Option<&std::path::Path>,
) -> JsonValue {
    let (reduced, attempts) = reduce_with(source, target_code, max_attempts, file_ctx);
    let target_present = still_fails(source, target_code, file_ctx);
    let reduced_bytes = reduced.len();
    let reduced = redact_secret_literals(&reduced);
    json!({
        "target_code": target_code,
        "target_present": target_present,
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
    // Line breaks between `secret` and `(` are rejected by the parser
    // but still denote a call for display purposes. Normalize the gap
    // in place (newlines → spaces) so a recovering parse sees the call
    // and every span maps 1:1 back to the original bytes.
    let normalized = normalize_secret_gaps(source);
    let mut spans = semantic_secret_spans(&normalized);
    // The lexical pass also covers malformed calls and locators the
    // parser cannot resolve. Overlaps are unioned below.
    spans.extend(lexical_secret_spans(&normalized));
    if spans.is_empty() {
        return source.to_string();
    }
    let mut ranges: Vec<(usize, usize)> = spans
        .iter()
        .filter_map(|span| span_offsets(&normalized, span))
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

/// Replace newlines in a `secret`…`(` gap with spaces (length-preserving),
/// so the recovering parser accepts a call broken across lines and span
/// offsets stay valid against the original text.
fn normalize_secret_gaps(source: &str) -> String {
    let lexed = crate::parser::Lexer::new(source).tokenize();
    let tokens = &lexed.tokens;
    let mut bytes = source.as_bytes().to_vec();
    // Blank comment text first (preserving newlines): a comment inside
    // a `secret`…`(` gap must not swallow the call when the newline is
    // turned into a space.
    for comment in &lexed.comments {
        if let Some((start, end)) = span_offsets(source, &comment.1) {
            for byte in &mut bytes[start..end] {
                if !matches!(*byte, b'\n' | b'\r') {
                    *byte = b' ';
                }
            }
        }
    }
    let span_of = |token: &crate::parser::Token| crate::diag::Span {
        line: token.line,
        column: token.col,
        end_line: token.end_line,
        end_column: token.end_col,
    };
    for index in 0..tokens.len() {
        if !is_secret_token(tokens, index) {
            continue;
        }
        let Some(call) = call_opening(tokens, index + 1, tokens[index].text == "secret") else {
            continue;
        };
        let probe = call.open;
        let Some((_, gap_start)) = span_offsets(source, &span_of(&tokens[index])) else {
            continue;
        };
        let Some((gap_end, _)) = span_offsets(source, &span_of(&tokens[probe])) else {
            continue;
        };
        if gap_start < gap_end && gap_end <= bytes.len() {
            for byte in &mut bytes[gap_start..gap_end] {
                if matches!(*byte, b'\n' | b'\r') {
                    *byte = b' ';
                }
            }
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| source.to_string())
}

/// String literals that feed a `secret(...)` locator, following
/// references and containers.
fn semantic_secret_spans(source: &str) -> Vec<crate::diag::Span> {
    let (module, _) = crate::parse_recovering(source, "redact", crate::ParseOptions::default());
    let mut spans = Vec::new();
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
    spans
}

/// Line numbers (1-based) that participate in a secret call: the line
/// with the `secret` keyword through the closing parenthesis. The
/// reducer never deletes these lines, so a reduced bundle can never
/// strand a secret literal outside a redactable call.
fn secret_call_lines(source: &str) -> std::collections::HashSet<u32> {
    use crate::parser::TokenKind as K;
    let mut lexer = crate::parser::Lexer::new(source);
    let lexed = lexer.tokenize();
    let tokens = &lexed.tokens;
    let mut lines = std::collections::HashSet::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if !is_secret_token(tokens, index) {
            index += 1;
            continue;
        }
        let start_line = tokens[index].line;
        let Some(call) = call_opening(tokens, index + 1, tokens[index].text == "secret") else {
            index += 1;
            continue;
        };
        let mut probe = call.open;
        let mut depth = 0i32;
        let mut end_line = start_line;
        while probe < tokens.len() {
            match tokens[probe].kind {
                K::LParen => depth += 1,
                K::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        end_line = tokens[probe].end_line;
                        break;
                    }
                }
                _ => {}
            }
            end_line = tokens[probe].end_line;
            probe += 1;
        }
        for line in start_line..=end_line {
            lines.insert(line);
        }
        index = probe.max(index + 1);
    }
    lines
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
    // Names of canonical `secret.env("NAME", …)` calls across the
    // document: the fail-closed sweep below keeps them visible.
    let mut canonical_env_names: std::collections::HashSet<(u32, u32, u32, u32)> =
        std::collections::HashSet::new();
    for probe in 0..tokens.len() {
        if !is_secret_token(tokens, probe) {
            continue;
        }
        let Some(call) = call_opening(tokens, probe + 1, tokens[probe].text == "secret") else {
            continue;
        };
        if !call.canonical_env {
            continue;
        }
        let mut first = skip_layout(tokens, call.open + 1);
        while matches!(tokens.get(first).map(|t| &t.kind), Some(K::LParen)) {
            first = skip_layout(tokens, first + 1);
        }
        if let Some(token) = tokens.get(first) {
            if matches!(
                token.kind,
                K::String(_) | K::SingleString | K::MlString(_) | K::MlSingleString
            ) {
                canonical_env_names.insert((token.line, token.col, token.end_line, token.end_col));
            }
        }
    }
    let mut index = 0usize;
    while index < tokens.len() {
        if !is_secret_token(tokens, index) {
            index += 1;
            continue;
        }
        // The gap was normalized to spaces, so the call opens at the
        // next token; `secret.env("NAME")` is a name (metadata), so its
        // first string is exempt but later arguments are still scanned.
        let Some(call) = call_opening(tokens, index + 1, tokens[index].text == "secret") else {
            index += 1;
            continue;
        };
        let open = call.open;
        let skip_first_string = call.canonical_env;
        // The env-name exemption applies only when the first argument
        // is itself a string (grouping parentheses allowed), so
        // `secret.env(default = "x", "NAME")` and
        // `secret.env(f("x"))` redact their nested strings.
        let mut lookahead = skip_layout(tokens, open + 1);
        while matches!(tokens.get(lookahead).map(|t| &t.kind), Some(K::LParen)) {
            lookahead = skip_layout(tokens, lookahead + 1);
        }
        let first_arg_plain = matches!(
            tokens.get(lookahead).map(|t| &t.kind),
            Some(K::String(_) | K::SingleString | K::MlString(_) | K::MlSingleString)
        );
        let mut depth = 0i32;
        let mut cursor = open;
        let mut seen_string = false;
        let mut seen_comma = false;
        let mut exempt_span: Option<crate::diag::Span> = None;
        let mut first_string_index: Option<usize> = None;
        while cursor < tokens.len() {
            match tokens[cursor].kind {
                K::LParen => depth += 1,
                K::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                K::Comma if depth == 1 => seen_comma = true,
                K::String(_) | K::SingleString | K::MlString(_) | K::MlSingleString => {
                    let exempt =
                        skip_first_string && first_arg_plain && !seen_comma && !seen_string;
                    seen_string = true;
                    let token = &tokens[cursor];
                    let span = crate::diag::Span {
                        line: token.line,
                        column: token.col,
                        end_line: token.end_line,
                        end_column: token.end_col,
                    };
                    if exempt {
                        first_string_index = Some(cursor);
                        exempt_span = Some(span);
                    } else {
                        spans.push(span);
                    }
                }
                _ => {}
            }
            cursor += 1;
        }
        // An env call with more tokens after the first argument — a
        // comma, a named `default =`, anything — may reference a
        // binding/table we cannot resolve lexically. Note that a
        // missing comma is precisely the malformed shape a reader
        // would still treat as a second argument, so count tokens, not
        // separators.
        let extra_env_args = skip_first_string
            && first_string_index.is_some_and(|first| {
                tokens[first + 1..cursor].iter().any(|token| {
                    !matches!(
                        token.kind,
                        K::Comma | K::LParen | K::RParen | K::Newline | K::Indent | K::Dedent
                    )
                })
            });
        // Fail closed: a malformed spelling, an env call whose first
        // argument is not a name, or an env call carrying anything
        // beyond its name redacts every string literal in the document
        // except canonical env names.
        if call.malformed
            || (skip_first_string && !first_arg_plain)
            || (skip_first_string && (seen_comma || extra_env_args))
        {
            for token in tokens {
                if matches!(
                    token.kind,
                    K::String(_) | K::SingleString | K::MlString(_) | K::MlSingleString
                ) {
                    let span = crate::diag::Span {
                        line: token.line,
                        column: token.col,
                        end_line: token.end_line,
                        end_column: token.end_col,
                    };
                    let is_exempt = exempt_span.as_ref().is_some_and(|e| {
                        e.line == span.line
                            && e.column == span.column
                            && e.end_line == span.end_line
                            && e.end_column == span.end_column
                    }) || canonical_env_names.contains(&(
                        span.line,
                        span.column,
                        span.end_line,
                        span.end_column,
                    ));
                    if !is_exempt {
                        spans.push(span);
                    }
                }
            }
        }
        index = cursor.max(index + 1);
    }
    spans
}

/// Skip layout trivia (newline/indent/dedent) before a punctuation
/// token; a call can be broken across lines outside brackets.
/// Shared call-opening scan: skip layout, tolerate any run of `.`
/// tokens and one `env`, then require `(`. Returns the parenthesis
/// index, whether the call is an env lookup, and whether it is the
/// canonical single-dot `secret.env(...)` form (the only form whose
/// first string argument is an exempt name).
struct CallOpen {
    open: usize,
    canonical_env: bool,
    /// Anything other than exactly `secret(` or `secret.env(`: the
    /// shape is not trustworthy, so redaction fails closed over the
    /// whole document.
    malformed: bool,
}

/// Case-insensitive `secret` keyword or identifier. Malformed spellings
/// (`Secret(`) are still secret calls for display purposes.
fn is_secret_token(tokens: &[crate::parser::Token], index: usize) -> bool {
    use crate::parser::TokenKind as K;
    tokens.get(index).is_some_and(|token| {
        matches!(token.kind, K::Secret | K::Ident) && token.text.eq_ignore_ascii_case("secret")
    })
}

fn call_opening(
    tokens: &[crate::parser::Token],
    start: usize,
    spelling_ok: bool,
) -> Option<CallOpen> {
    use crate::parser::TokenKind as K;
    let mut probe = skip_layout(tokens, start);
    let mut dots_before_env = 0usize;
    let mut dots_after_env = 0usize;
    let mut env = false;
    let mut ident_run = false;
    loop {
        match tokens.get(probe).map(|t| &t.kind) {
            Some(K::Dot) => {
                if env {
                    dots_after_env += 1;
                } else {
                    dots_before_env += 1;
                }
                probe = skip_layout(tokens, probe + 1);
            }
            Some(K::Env) => {
                env = true;
                probe = skip_layout(tokens, probe + 1);
            }
            // Any identifier between `secret` and `(` (a case variant
            // such as `Env`, or a dotted member run) still opens a
            // reader-plausible call. It is malformed, not ignored.
            Some(K::Ident) => {
                ident_run = true;
                probe = skip_layout(tokens, probe + 1);
            }
            _ => break,
        }
    }
    if !matches!(tokens.get(probe).map(|t| &t.kind), Some(K::LParen)) {
        return None;
    }
    let canonical_env = env && dots_before_env == 1 && dots_after_env == 0 && !ident_run;
    // Whitespace is not a malformation; stray dots, identifiers around
    // `env`, or a case-divergent keyword are. A malformed shape still
    // opens a call (the parenthesis is there), so callers can protect
    // and fail closed.
    let malformed = if env {
        !canonical_env
    } else {
        dots_before_env != 0 || ident_run
    } || !spelling_ok;
    Some(CallOpen {
        open: probe,
        canonical_env,
        malformed,
    })
}

fn skip_layout(tokens: &[crate::parser::Token], mut index: usize) -> usize {
    use crate::parser::TokenKind as K;
    while index < tokens.len() && matches!(tokens[index].kind, K::Newline | K::Indent | K::Dedent) {
        index += 1;
    }
    index
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
