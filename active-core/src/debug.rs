//! Debugger primitives: breakpoints, deterministic replay, and
//! minimal failing-case reduction (TOOLING DEBUG.001-005).
//!
//! Everything here is a pure function of the shipped evaluator and the
//! step trace from `Engine::evaluate_with_steps`. Values are redacted by
//! construction; breakpoints select, never reveal, secret material.

use serde_json::{json, Value as JsonValue};

use crate::eval::StepEvent;
use crate::{Engine, EvalError, Module};

/// Minimum length for a substring duplicate match. A shorter structural
/// secret is still matched exactly: spreading it as a substring would
/// redact unrelated fragments of every string in the bundle.
const MIN_SUBSTRING_SEED: usize = 4;

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

/// Deterministic replay: compare the redacted content hash, the
/// diagnostic code lists, and the step traces. A wall-bounded preflight
/// mirrors `eval` (identity-only blobs, no step collection); two
/// canonical materialised step passes then run without a wall deadline
/// and provide the hash, codes, and steps. When `expected` is given,
/// the first hash must equal it (the audit trail's `result` value).
/// Uses the shipped default budget; callers whose audit ran under a
/// different limit pass it to [`replay_with_budget`].
pub fn replay(
    module: &Module,
    expected: Option<&str>,
    override_profile: Option<&str>,
) -> Result<JsonValue, String> {
    replay_with_budget(
        module,
        expected,
        override_profile,
        &crate::Budget::default(),
        false,
    )
}

/// Replay under an explicit budget and resilient mode. `resilient`
/// matches the audited run: `eval` and the audit trail are strict by
/// default, so replay is strict too. The wall-bounded preflight mirrors
/// `eval`'s work; the two canonical step/hash passes have no wall
/// deadline, so a successful first pass cannot turn into a
/// timing-dependent mismatch.
pub fn replay_with_budget(
    module: &Module,
    expected: Option<&str>,
    override_profile: Option<&str>,
    budget: &crate::Budget,
    resilient: bool,
) -> Result<JsonValue, String> {
    let mut engine = Engine::new();
    engine.profile_override = override_profile.map(str::to_string);
    engine.resilient = resilient;
    engine.budget = *budget;
    // The wall-bounded preflight mirrors `eval` exactly: identity-only
    // blobs, no step collection. Materialising for the canonical passes
    // below would copy every payload per alias and could trip X002 on a
    // document `eval` accepted.
    if budget.max_wall_ms != 0 {
        let _preflight = engine
            .evaluate_with_seeds(module)
            .map_err(|e| e.to_string())?;
    }
    // Canonical audit form: materialise so payload-resident secrets are
    // visible to the redactor, then strip payloads so the identity does
    // not depend on the flag the audited run used. The probe skips
    // per-alias payload accounting; count/size limits still bind so an
    // untrusted document fails fast instead of exhausting memory. These
    // passes have no wall deadline, so step bookkeeping cannot turn a
    // successful evaluation into a timing-dependent failure.
    engine.budget.max_wall_ms = 0;
    engine.materialise_blobs = true;
    engine.taint_probe = true;
    let (first, first_diags, first_steps, first_seeds) = engine
        .evaluate_with_steps_and_seeds(module)
        .map_err(|e| e.to_string())?;
    // Hash the first canonical tree before the second pass so only one
    // materialised tree is live at a time.
    let profile = crate::secret::SecretProfile::for_engine(module, &engine, &first_seeds);
    let first_hash = crate::provenance::content_hash(&crate::convert::strip_blob_payloads(
        &crate::convert::redact_value(&profile, &first),
    ));
    let first_codes: Vec<String> = first_diags.iter().map(|d| d.code.clone()).collect();
    let first_steps_digest = steps_digest(&first_steps);
    let first_steps_len = first_steps.len();
    drop(first_steps);
    drop(first_diags);
    drop(first);

    let (second, second_diags, second_steps, second_seeds) = engine
        .evaluate_with_steps_and_seeds(module)
        .map_err(|e| e.to_string())?;
    let second_profile = crate::secret::SecretProfile::for_engine(module, &engine, &second_seeds);
    let second_hash = crate::provenance::content_hash(&crate::convert::strip_blob_payloads(
        &crate::convert::redact_value(&second_profile, &second),
    ));
    let second_codes: Vec<String> = second_diags.iter().map(|d| d.code.clone()).collect();
    let steps_match = first_steps_digest == steps_digest(&second_steps);
    drop(second_steps);
    drop(second_diags);
    drop(second);
    let expected_ok = expected.is_none_or(|text| text == first_hash);
    let ok = first_hash == second_hash && first_codes == second_codes && steps_match && expected_ok;

    Ok(json!({
        "ok": ok,
        "hash": first_hash,
        "second_hash": second_hash,
        "codes": first_codes,
        "steps": first_steps_len,
        "steps_match": steps_match,
        "expected": expected,
        "expected_match": expected_ok,
    }))
}

/// Digest of a step array, so two passes can be compared without
/// retaining both rendered vectors.
fn steps_digest(steps: &[StepEvent]) -> [u8; 32] {
    let bytes = serde_json::to_vec(steps).unwrap_or_default();
    *blake3::hash(&bytes).as_bytes()
}

/// Evaluation inputs for reduction, so a minimized case is produced
/// under the same mode, parse options, profile, and limits as the run
/// it explains. `Default` is the strict shipped profile.
#[derive(Clone, Copy, Default)]
pub struct DebugOptions<'a> {
    pub resilient: bool,
    pub allow_parent_escapes: bool,
    pub profile: Option<&'a str>,
    pub budget: Option<&'a crate::Budget>,
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
    reduce_with_options(
        source,
        target_code,
        max_attempts,
        file_ctx,
        DebugOptions::default(),
    )
}

/// File-aware reduction under explicit evaluation inputs.
pub fn reduce_with_options(
    source: &str,
    target_code: &str,
    max_attempts: usize,
    file_ctx: Option<&std::path::Path>,
    options: DebugOptions<'_>,
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
            if still_fails(&candidate, target_code, file_ctx, options) {
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
fn still_fails(
    text: &str,
    target_code: &str,
    file_ctx: Option<&std::path::Path>,
    options: DebugOptions<'_>,
) -> bool {
    let parse_options = crate::ParseOptions {
        // String-based reduction stays hermetic: includes resolve only
        // when a file context was supplied.
        enable_includes: file_ctx.is_some(),
        allow_parent_escapes: options.allow_parent_escapes,
        resilient: options.resilient,
        ..Default::default()
    };
    let parsed = match file_ctx {
        Some(path) => crate::parse_text_recovering_with_file(
            text,
            &path.to_string_lossy(),
            parse_options,
            path,
        ),
        None => crate::parse_recovering(text, "reduce", parse_options),
    };
    let (module, parse_diags) = parsed;
    if parse_diags.iter().any(|d| d.code == target_code) {
        return true;
    }
    // The shipped CLI never evaluates a module with parse errors (in
    // either mode; resilient heals warnings, not syntax errors), so a
    // runtime code cannot be "present" through the real path.
    if parse_diags
        .iter()
        .any(|d| d.severity == crate::diag::Severity::Error)
    {
        return false;
    }
    let mut engine = Engine::new();
    engine.resilient = options.resilient;
    engine.profile_override = options.profile.map(str::to_string);
    if let Some(budget) = options.budget {
        engine.budget = *budget;
    }
    // Reduction must not depend on machine load: candidates run without
    // a wall deadline (count/size limits still bind), except when the
    // wall-clock code itself is the target.
    if target_code != "X002" {
        engine.budget.max_wall_ms = 0;
    }
    match engine.evaluate(&module) {
        Ok((_, diagnostics)) => diagnostics.iter().any(|d| d.code == target_code),
        // Strict validation surfaces its diagnostics inside the error;
        // the target may be any of them, not only the wrapper code.
        Err(EvalError::ValidationFailed { diagnostics }) => {
            diagnostics.iter().any(|d| d.code == target_code)
        }
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
    minimal_case_with_options(
        source,
        target_code,
        max_attempts,
        file_ctx,
        DebugOptions::default(),
    )
}

/// File-aware minimal case under explicit evaluation inputs.
pub fn minimal_case_with_options(
    source: &str,
    target_code: &str,
    max_attempts: usize,
    file_ctx: Option<&std::path::Path>,
    options: DebugOptions<'_>,
) -> JsonValue {
    let (reduced, attempts) =
        reduce_with_options(source, target_code, max_attempts, file_ctx, options);
    let target_present = still_fails(source, target_code, file_ctx, options);
    let reduced_bytes = reduced.len();
    // Seeds cover values the source-level locator scan cannot see:
    // environment reads, computed interpolations, and multi-line
    // dedenting. The evaluator runs under the same options as the run
    // being explained.
    let seeds = bundle_seeds(source, file_ctx, options);
    let reduced = redact_secret_literals(&reduced, &seeds.0, seeds.1);
    json!({
        "target_code": target_code,
        "target_present": target_present,
        "attempts": attempts,
        "original_bytes": source.len(),
        "reduced_bytes": reduced_bytes,
        "reduced": reduced,
    })
}

/// Runtime secret seeds for a reduction bundle. Evaluation uses the
/// caller's options (profile, parse flags, limits) with no wall
/// deadline; seeds are collected during the walk, so a hard error keeps
/// the material text discovered so far. A document with no secret,
/// environment, or profile input skips the walk entirely.
fn bundle_seeds(
    source: &str,
    file_ctx: Option<&std::path::Path>,
    options: DebugOptions<'_>,
) -> (crate::secret::SecretRedactor, bool) {
    let parse_options = crate::ParseOptions {
        enable_includes: file_ctx.is_some(),
        allow_parent_escapes: options.allow_parent_escapes,
        resilient: options.resilient,
        ..Default::default()
    };
    let (module, _) = match file_ctx {
        Some(path) => crate::parse_text_recovering_with_file(
            source,
            &path.to_string_lossy(),
            parse_options,
            path,
        ),
        None => crate::parse_recovering(source, "minimize", parse_options),
    };
    if !module_is_material(&module) {
        return (crate::secret::SecretRedactor::new(), false);
    }
    let mut engine = Engine::new();
    // Seeds are a value snapshot, not a verdict: evaluate resiliently so
    // a strict validation failure still yields the material strings the
    // bundle must redact. Seeds are written during the walk, so a hard
    // error keeps what was discovered.
    engine.resilient = true;
    engine.profile_override = options.profile.map(str::to_string);
    if let Some(budget) = options.budget {
        engine.budget = *budget;
    }
    engine.budget.max_wall_ms = 0;
    let (outcome, mut seeds) = engine.evaluate_collecting_seeds(&module);
    // A hard error can stop the walk before later secret nodes; the
    // bundle then fails closed by redacting every string literal.
    let has_secret = module
        .arena
        .iter()
        .any(|node| matches!(node, crate::ir::Node::Secret { .. }));
    let fail_closed = outcome.is_err() && has_secret;
    // Profile-authored literals at material paths are known secrets
    // even when their node value is path-material only.
    let profile = crate::secret::SecretProfile::with_context(
        &module,
        options.profile,
        Some(&engine.env),
        Some(&seeds),
    );
    seeds.merge(profile.literals());
    (seeds, fail_closed)
}

fn module_is_material(module: &Module) -> bool {
    !module.profiles.is_empty()
        || module.arena.iter().any(|node| {
            matches!(
                node,
                crate::ir::Node::Secret { .. } | crate::ir::Node::Env { .. }
            )
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
///
/// 3. **Duplicates** (value pass): any other string token whose decoded
///    value equals a known secret value (authored locator or evaluator
///    seed, including env/computed/interpolated values) is replaced
///    too, and any comment containing one is redacted wholesale. A
///    base64 or hex encoding of a known value counts as a duplicate.
fn redact_secret_literals(
    source: &str,
    seeds: &crate::secret::SecretRedactor,
    fail_closed: bool,
) -> String {
    // Line breaks between `secret` and `(` are rejected by the parser
    // but still denote a call for display purposes. Normalize the gap
    // in place (newlines → spaces) so a recovering parse sees the call
    // and every span maps 1:1 back to the original bytes.
    let normalized = normalize_secret_gaps(source);
    let line_starts = line_byte_starts(&normalized);
    let offsets = |span: &crate::diag::Span| span_offsets_with(&line_starts, &normalized, span);
    let mut spans = semantic_secret_spans(&normalized);
    // The lexical pass also covers malformed calls and locators the
    // parser cannot resolve. Overlaps are unioned below.
    spans.extend(lexical_secret_spans(&normalized));
    if spans.is_empty() && seeds.is_empty() && !fail_closed {
        return source.to_string();
    }
    let mut ranges: Vec<(usize, usize)> = spans.iter().filter_map(offsets).collect();
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
    let lexed = crate::parser::Lexer::new(&normalized).tokenize();
    let env_names = env_name_span_keys(&lexed.tokens);
    let structural = structural_span_keys(&lexed.tokens);
    // Authored locator values (decoded token text).
    let mut authored = crate::secret::SecretRedactor::new();
    for token in &lexed.tokens {
        if !is_string_token(token) {
            continue;
        }
        let Some((start, end)) = offsets(&token_span(token)) else {
            continue;
        };
        if merged.iter().any(|&(s, e)| start >= s && end <= e) {
            authored.insert(decoded_token_text(token).unwrap_or_else(|| token.text.clone()));
        }
    }
    // Duplicates of a known value elsewhere in the document (a blob
    // payload, a copy, an echo) leak just as badly as the locator.
    let mut extras: Vec<(usize, usize)> = Vec::new();
    for token in &lexed.tokens {
        if !is_string_token(token) {
            continue;
        }
        let Some((start, end)) = offsets(&token_span(token)) else {
            continue;
        };
        if merged.iter().any(|&(s, e)| start >= s && end <= e) {
            continue;
        }
        if env_names.contains(&token_key(token)) || structural.contains(&token_key(token)) {
            continue;
        }
        if fail_closed {
            extras.push((start, end));
            continue;
        }
        let matched = if token.text.contains("${") {
            // An interpolation keeps `${…}` in the token text; the
            // resolved value is unavailable, so fail closed on a known
            // substring.
            authored.redact_text(&token.text).is_some() || seeds.redact_text(&token.text).is_some()
        } else {
            match decoded_token_text(token) {
                Some(value) => {
                    // Exact (whole-token) duplicates catch the common
                    // copy; a substring match catches a secret embedded
                    // in surrounding text. Values shorter than four
                    // bytes stay exact-only so a tiny structural secret
                    // cannot redact unrelated text everywhere.
                    value_is_known(&value, &authored, seeds)
                        || authored.contains_substring(&value, MIN_SUBSTRING_SEED)
                        || seeds.contains_substring(&value, MIN_SUBSTRING_SEED)
                }
                None => false,
            }
        };
        if matched {
            extras.push((start, end));
        }
    }
    // Comments are non-semantic: redact the whole comment when it
    // contains a known value (substring match is the fail-closed rule).
    // Lex the original text: `normalized` blanks comment bytes so the
    // recovering parser sees no comment at all.
    let comment_lex = crate::parser::Lexer::new(source).tokenize();
    let source_starts = line_byte_starts(source);
    let mut comment_ranges: Vec<(usize, usize)> = Vec::new();
    for (_, span) in &comment_lex.comments {
        let Some((start, end)) = span_offsets_with(&source_starts, source, span) else {
            continue;
        };
        let text = &source[start..end];
        let authored_hit = authored.redact_text(text).is_some();
        if fail_closed || authored_hit || seeds.redact_text(text).is_some() {
            comment_ranges.push((start, end));
        }
    }
    let mut all: Vec<(usize, usize)> = merged
        .iter()
        .copied()
        .chain(extras)
        .chain(comment_ranges)
        .collect();
    all.sort_unstable();
    let mut coalesced: Vec<(usize, usize)> = Vec::new();
    for (start, end) in all {
        match coalesced.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => coalesced.push((start, end)),
        }
    }
    let mut out = source.to_string();
    for (start, end) in coalesced.iter().rev() {
        let replacement = if source[*start..].starts_with('#') {
            "# ***SECRET***"
        } else {
            "\"***SECRET***\""
        };
        out.replace_range(*start..*end, replacement);
    }
    out
}

/// True when a decoded token value is a known secret, exactly or via a
/// base64/hex encoding of one.
fn value_is_known(
    value: &str,
    authored: &crate::secret::SecretRedactor,
    seeds: &crate::secret::SecretRedactor,
) -> bool {
    if authored.contains_exact(value) || seeds.contains_exact(value) {
        return true;
    }
    if let Some(decoded) = decode_base64(value) {
        if authored.contains_exact(&decoded) || seeds.contains_exact(&decoded) {
            return true;
        }
    }
    if let Some(decoded) = decode_hex(value) {
        if authored.contains_exact(&decoded) || seeds.contains_exact(&decoded) {
            return true;
        }
    }
    false
}

fn decode_base64(value: &str) -> Option<String> {
    use base64::Engine as _;
    let compact: String = value.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let engines = [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        &base64::engine::general_purpose::URL_SAFE,
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ];
    for engine in engines {
        if let Ok(bytes) = engine.decode(&compact) {
            if let Ok(text) = String::from_utf8(bytes) {
                return Some(text);
            }
        }
    }
    None
}

fn decode_hex(value: &str) -> Option<String> {
    let compact: String = value
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect::<String>()
        .trim_start_matches("0x")
        .to_string();
    let chars: Vec<char> = compact.chars().collect();
    if chars.len() < 2 || chars.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(chars.len() / 2);
    for pair in chars.chunks_exact(2) {
        let digit = |c: char| c.to_digit(16).map(|d| d as u8);
        bytes.push(digit(pair[0])? * 16 + digit(pair[1])?);
    }
    String::from_utf8(bytes).ok()
}

/// Span key of a lexer token.
fn token_key(token: &crate::parser::Token) -> (u32, u32, u32, u32) {
    (token.line, token.col, token.end_line, token.end_col)
}

/// Keys of the first string argument of every `secret.env("NAME")`
/// call: canonical env names are not material and stay readable.
fn env_name_span_keys(
    tokens: &[crate::parser::Token],
) -> std::collections::HashSet<(u32, u32, u32, u32)> {
    let mut out = std::collections::HashSet::new();
    for index in 0..tokens.len() {
        if !is_secret_token(tokens, index) {
            continue;
        }
        let line = tokens[index].line;
        let mut probe = index + 1;
        // Strict same-line shape: `secret` `.` `env` `(` [`(`…] STRING.
        // A loose lookahead could cross a line and exempt a later call.
        let expect = |probe: usize, text: &str| {
            tokens
                .get(probe)
                .is_some_and(|token| token.line == line && token.text == text)
        };
        if !expect(probe, ".") {
            continue;
        }
        probe += 1;
        if !expect(probe, "env") {
            continue;
        }
        probe += 1;
        if !expect(probe, "(") {
            continue;
        }
        probe += 1;
        while expect(probe, "(") {
            probe += 1;
        }
        if let Some(token) = tokens.get(probe) {
            if token.line == line && is_string_token(token) && is_env_name(&token.text) {
                out.insert(token_key(token));
            }
        }
    }
    out
}

/// A canonical environment name: identifier shape. Anything else in
/// `secret.env(...)` is not trustworthy and stays redactable.
fn is_env_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Tokens whose text is document structure rather than data: `include`
/// targets, profile names, the active-profile selector, and the path
/// arguments of `@patch`/`@remove`/`@rename`. A minimal case must keep
/// them readable, or the bundle can no longer resolve includes, select
/// the profile, or apply its patches. Each shape must start a line and
/// close on that same line; an incomplete shape is not trusted, so
/// fail-closed still redacts it.
fn structural_span_keys(
    tokens: &[crate::parser::Token],
) -> std::collections::HashSet<(u32, u32, u32, u32)> {
    // Bracket depth at each token. Structural keywords and directives
    // are document-level; a table field that happens to be named
    // `active_profile` (or a nested `include = "…"`) is data, not
    // structure. String tokens never change the depth.
    let mut depth_at: Vec<usize> = Vec::with_capacity(tokens.len());
    let mut depth = 0usize;
    for token in tokens {
        depth_at.push(depth);
        if is_string_token(token) {
            continue;
        }
        match token.text.as_str() {
            "{" | "[" | "(" => depth += 1,
            "}" | "]" | ")" => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let mut out = std::collections::HashSet::new();
    for index in 0..tokens.len() {
        let line = tokens[index].line;
        let starts_line = index == 0 || tokens[index - 1].line < line;
        if !starts_line || depth_at.get(index).copied().unwrap_or(0) != 0 {
            continue;
        }
        let same_line = |at: usize| tokens.get(at).is_some_and(|token| token.line == line);
        let next_string = |at: usize| {
            tokens
                .get(at)
                .filter(|token| token.line == line && is_string_token(token))
        };
        match tokens[index].text.as_str() {
            "include" | "profile" => {
                if let Some(token) = next_string(index + 1) {
                    out.insert(token_key(token));
                }
            }
            "active_profile" => {
                if same_line(index + 1) && tokens[index + 1].text == "=" {
                    if let Some(token) = next_string(index + 2) {
                        out.insert(token_key(token));
                    }
                }
            }
            "@" => {
                let directive = tokens[index + 1].text.as_str();
                let is_directive =
                    same_line(index + 1) && matches!(directive, "patch" | "remove" | "rename");
                if !is_directive || !same_line(index + 2) || tokens[index + 2].text != "(" {
                    continue;
                }
                // Find the matching close on this line first: an
                // unclosed directive is malformed and exempts nothing.
                // String tokens never count as punctuation: a value
                // such as `"("` must not change the nesting.
                let mut probe = index + 3;
                let mut depth = 1usize;
                let mut close = None;
                while let Some(token) = tokens.get(probe) {
                    if token.line != line {
                        break;
                    }
                    if !is_string_token(token) {
                        match token.text.as_str() {
                            "(" => depth += 1,
                            ")" => {
                                depth = depth.saturating_sub(1);
                                if depth == 0 {
                                    close = Some(probe);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    probe += 1;
                }
                let Some(close) = close else {
                    continue;
                };
                // Only path arguments are structure: the path of
                // `@patch`/`@remove`, the source and target of
                // `@rename`. A value argument is data and goes through
                // the normal redaction passes. A path must be a single
                // bare string token; an expression that builds one from
                // several tokens is not trusted, so none of its parts
                // are exempt.
                let path_args = if directive == "rename" { 2 } else { 1 };
                let mut depth = 1usize;
                let mut argument = 0usize;
                let mut tokens_in_argument = 0usize;
                let mut strings_in_argument = 0usize;
                let mut lone_string = None;
                for token in &tokens[index + 3..close] {
                    if !is_string_token(token) {
                        match token.text.as_str() {
                            "(" => {
                                depth += 1;
                                tokens_in_argument += 1;
                                continue;
                            }
                            ")" => {
                                depth = depth.saturating_sub(1);
                                tokens_in_argument += 1;
                                continue;
                            }
                            "," if depth == 1 => {
                                if argument < path_args
                                    && tokens_in_argument == 1
                                    && strings_in_argument == 1
                                {
                                    if let Some(key) = lone_string {
                                        out.insert(key);
                                    }
                                }
                                argument += 1;
                                tokens_in_argument = 0;
                                strings_in_argument = 0;
                                lone_string = None;
                                continue;
                            }
                            _ => {
                                tokens_in_argument += 1;
                                continue;
                            }
                        }
                    }
                    tokens_in_argument += 1;
                    strings_in_argument += 1;
                    lone_string = Some(token_key(token));
                }
                if argument < path_args && tokens_in_argument == 1 && strings_in_argument == 1 {
                    if let Some(key) = lone_string {
                        out.insert(key);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Decoded value of a string token. `token.text` is the decoded literal
/// text (an interpolation's `${…}` stays in the text; nested tokens are
/// not part of the lexer stream). Multi-line strings are dedented.
fn decoded_token_text(token: &crate::parser::Token) -> Option<String> {
    use crate::parser::TokenKind as K;
    let (decoded, marks) = match &token.kind {
        K::MlString(positions) => {
            let (text, marks) =
                crate::parser::ml_text_and_marks(&token.text, token.line, token.col, positions);
            (text, Some(marks))
        }
        K::MlSingleString => (crate::parser::strip_common_indent(&token.text), None),
        // A plain string keeps its `\$` sentinel; the lexer records the
        // sentinel in the position map. Without the marks a duplicate of
        // `"pa\$word"` would keep U+E000 and under-redact.
        K::String(positions) => (
            token.text.clone(),
            Some(
                token
                    .text
                    .chars()
                    .enumerate()
                    .map(|(index, c)| {
                        c == crate::parser::ESCAPED_DOLLAR
                            && positions.get(index) == Some(&crate::parser::SENTINEL_POS)
                    })
                    .collect(),
            ),
        ),
        K::SingleString => (token.text.clone(), None),
        _ => return None,
    };
    let mut out = String::with_capacity(decoded.len());
    for (index, c) in decoded.chars().enumerate() {
        if c == crate::parser::ESCAPED_DOLLAR {
            // Only an escaped `\$` carries a sentinel mark; a genuine
            // U+E000 is user data and stays as-is.
            let escaped = marks
                .as_ref()
                .and_then(|marks| marks.get(index))
                .is_some_and(|marked| *marked);
            out.push(if escaped { '$' } else { c });
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// True for a token that carries a string value.
fn is_string_token(token: &crate::parser::Token) -> bool {
    matches!(
        token.kind,
        crate::parser::TokenKind::String(_)
            | crate::parser::TokenKind::SingleString
            | crate::parser::TokenKind::MlString(_)
            | crate::parser::TokenKind::MlSingleString
    )
}

/// Source span of a lexer token.
fn token_span(token: &crate::parser::Token) -> crate::diag::Span {
    crate::diag::Span {
        line: token.line,
        column: token.col,
        end_line: token.end_line,
        end_column: token.end_col,
    }
}

/// Replace newlines in a `secret`…`(` gap with spaces (length-preserving),
/// so the recovering parser accepts a call broken across lines and span
/// offsets stay valid against the original text.
fn normalize_secret_gaps(source: &str) -> String {
    let line_starts = line_byte_starts(source);
    let lexed = crate::parser::Lexer::new(source).tokenize();
    let tokens = &lexed.tokens;
    let mut bytes = source.as_bytes().to_vec();
    // Blank comment text first (preserving newlines): a comment inside
    // a `secret`…`(` gap must not swallow the call when the newline is
    // turned into a space.
    for comment in &lexed.comments {
        if let Some((start, end)) = span_offsets_with(&line_starts, source, &comment.1) {
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
        let Some((_, gap_start)) =
            span_offsets_with(&line_starts, source, &span_of(&tokens[index]))
        else {
            continue;
        };
        let Some((gap_end, _)) = span_offsets_with(&line_starts, source, &span_of(&tokens[probe]))
        else {
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
        if !call.canonical_env || call.malformed {
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
            ) && is_env_name(&token.text)
            {
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
        let skip_first_string = call.canonical_env && !call.malformed;
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
    let is_call = tokens.get(index).is_some_and(|token| {
        matches!(token.kind, K::Secret | K::Ident) && token.text.eq_ignore_ascii_case("secret")
    });
    if !is_call {
        return false;
    }
    // `a.secret.env(NAME)` is a member, not the keyword call.
    let mut probe = index;
    while probe > 0 {
        probe -= 1;
        match tokens[probe].kind {
            K::Newline | K::Indent | K::Dedent => continue,
            K::Dot => return false,
            _ => break,
        }
    }
    true
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
/// Span offsets with a precomputed line index (per-comment scanning is
/// otherwise O(comments × source)).
fn span_offsets_with(
    line_starts: &[usize],
    text: &str,
    span: &crate::diag::Span,
) -> Option<(usize, usize)> {
    let start = byte_at(line_starts, text, span.line, span.column)?;
    let end = byte_at(line_starts, text, span.end_line, span.end_column)?;
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
