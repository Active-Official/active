//! Schema validation tests (C-031, spec/validation.md).
//!
//! Drives the real shipped path (`parse_file` → `Engine::evaluate`) on
//! committed fixtures under `tests/validation/`. Covers full-document
//! acceptance, every violation code (T002/T003/T004), strict fatality
//! with all violations collected, resilient pass-through, deterministic
//! ordering, post-patch validation, span attribution, and the secret
//! invariant (no evaluated value ever reaches a message).

use active_core::{parse, parse_file, Engine, EvalError, ParseOptions};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("validation")
        .join(name)
}

fn strict_engine() -> Engine {
    let mut engine = Engine::new();
    engine.resilient = false;
    engine
}

fn evaluate_strict(name: &str) -> Result<serde_json::Value, Vec<active_core::Diagnostic>> {
    let module = parse_file(&fixture(name), ParseOptions::default())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    match strict_engine().evaluate(&module) {
        Ok((value, diags)) => {
            assert!(diags.is_empty(), "{name}: unexpected diags: {diags:?}");
            let json = active_core::convert::value_to_pretty_json(&value);
            Ok(serde_json::from_str(&json).expect("valid json"))
        }
        Err(EvalError::ValidationFailed { diagnostics }) => Err(diagnostics),
        Err(e) => panic!("{name}: unexpected error: {e:?}"),
    }
}

fn evaluate_resilient(name: &str) -> (serde_json::Value, Vec<active_core::Diagnostic>) {
    let module = parse_file(&fixture(name), ParseOptions::default())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: resilient must succeed: {e:?}"));
    let json = active_core::convert::value_to_pretty_json(&value);
    (serde_json::from_str(&json).expect("valid json"), diags)
}

#[test]
fn valid_document_passes_in_both_modes() {
    let v = evaluate_strict("valid_full.active").expect("valid doc must pass strict");
    assert_eq!(v["port"], 8080);
    assert_eq!(v["mode"], "prod");
    assert_eq!(v["tags"], serde_json::json!(["a", "b"]));
    assert_eq!(v["server"]["port"], 5432);
    assert!(v.get("schema").is_none());
    let (_, diags) = evaluate_resilient("valid_full.active");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn strict_collects_every_violation_then_fails() {
    let diags = evaluate_strict("invalid_mixed.active").expect_err("must fail strict");
    let codes: Vec<&str> = diags.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(
        codes,
        ["T003", "T002", "T003", "T002", "T002", "T002", "T004", "T004"],
        "schema order, all collected: {diags:?}"
    );
    assert!(diags
        .iter()
        .all(|d| d.severity == active_core::Severity::Error));
    let paths: Vec<&str> = diags
        .iter()
        .map(|d| d.path.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        paths,
        [
            "port",
            "host",
            "mode",
            "ratio",
            "delay",
            "tags[1]",
            "missing_no_default",
            "with_default"
        ],
        "{diags:?}"
    );
    // Direct data lines up with the arena: every violation has a span.
    assert!(diags.iter().all(|d| d.span.is_some()), "{diags:?}");
    // The strict message names the default tension explicitly.
    assert!(
        diags[7].message.contains("despite a schema default"),
        "{:?}",
        diags[7]
    );
    let mode = &diags[2];
    assert!(mode.message.contains("\"dev\""), "{mode:?}");
    assert!(!mode.message.contains("staging"), "{mode:?}");
}

#[test]
fn strict_error_carries_first_code_and_span() {
    let module =
        parse_file(&fixture("invalid_mixed.active"), ParseOptions::default()).expect("must parse");
    let err = strict_engine().evaluate(&module).expect_err("must fail");
    assert_eq!(err.code(), "T003");
    assert!(err.span().is_some(), "first violation span");
}

#[test]
fn resilient_collects_and_fills_defaults() {
    let (v, diags) = evaluate_resilient("invalid_mixed.active");
    assert_eq!(diags.len(), 8, "{diags:?}");
    // The defaulted key fills with a W005; nothing else is touched.
    assert_eq!(v["with_default"], 7);
    assert!(
        diags
            .iter()
            .any(|d| d.code == "W005" && d.path.as_deref() == Some("with_default")),
        "{diags:?}"
    );
    assert_eq!(v["port"], 999);
    assert_eq!(v["tags"][1], "two");
    assert!(v.get("missing_no_default").is_none());
}

#[test]
fn validation_is_deterministic() {
    let run = || {
        let module = parse_file(&fixture("invalid_mixed.active"), ParseOptions::default())
            .expect("must parse");
        match strict_engine().evaluate(&module) {
            Err(EvalError::ValidationFailed { diagnostics }) => diagnostics
                .into_iter()
                .map(|d| (d.code, d.message, d.path))
                .collect::<Vec<_>>(),
            other => panic!("must fail strict: {other:?}"),
        }
    };
    assert_eq!(run(), run());
}

#[test]
fn patches_apply_before_validation() {
    // Base port=80 is valid; the patch drives it to 70000.
    let diags = evaluate_strict("patch_then_validate.active").expect_err("patched value invalid");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T003");
    assert_eq!(diags[0].path.as_deref(), Some("port"));
    assert!(diags[0].message.contains("above maximum"), "{:?}", diags[0]);
}

#[test]
fn secret_values_never_reach_messages() {
    let diags = evaluate_strict("secret_mismatch.active").expect_err("must fail");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T002");
    for d in &diags {
        assert!(!d.message.contains("s3cr3t"), "{d:?}");
        assert!(!d.message.contains("hunter2"), "{d:?}");
    }
}

fn eval_inline_strict(src: &str) -> Result<(), Vec<active_core::Diagnostic>> {
    let module = parse(src, "inline").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    match strict_engine().evaluate(&module) {
        Ok(_) => Ok(()),
        Err(EvalError::ValidationFailed { diagnostics }) => Err(diagnostics),
        Err(e) => panic!("unexpected error: {e:?}"),
    }
}

#[test]
fn int_widens_to_float_but_not_the_reverse() {
    eval_inline_strict("schema = { r = Float }\nr = 1\n").expect("int widens");
    let diags = eval_inline_strict("schema = { n = Int }\nn = 1.5\n").expect_err("float narrows");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, "T002");
}

#[test]
fn absent_null_keys_and_extra_keys_are_quiet() {
    eval_inline_strict("schema = { x = Null }\n").expect("absent null is null");
    eval_inline_strict("schema = { port = Int }\nport = 1\nother = 2\n")
        .expect("open world ignores extras");
}

#[test]
fn duration_and_bytes_bounds_compare_in_domain() {
    eval_inline_strict("schema = { d = Duration(min = 1s, max = 1m) }\nd = 30s\n")
        .expect("in-domain duration");
    let diags = eval_inline_strict("schema = { d = Duration(min = 1s) }\nd = 500ms\n")
        .expect_err("below minimum");
    assert_eq!(diags[0].code, "T003");
    eval_inline_strict("schema = { q = Bytes(max = 1MiB) }\nq = 512KiB\n")
        .expect("in-domain bytes");
}

#[test]
fn string_lengths_and_patterns() {
    eval_inline_strict("schema = { s = String(min_len = 2, max_len = 4) }\ns = \"abc\"\n")
        .expect("length ok");
    let diags = eval_inline_strict("schema = { s = String(min_len = 2) }\ns = \"a\"\n")
        .expect_err("too short");
    assert_eq!(diags[0].code, "T003");
    assert!(diags[0].message.contains("too short"), "{:?}", diags[0]);
    eval_inline_strict("schema = { s = String(pattern = \"^[a-z]+$\") }\ns = \"abc\"\n")
        .expect("pattern matches");
    let diags =
        eval_inline_strict("schema = { s = String(pattern = \"^[a-z]+$\") }\ns = \"ABC\"\n")
            .expect_err("pattern mismatch");
    assert!(
        diags[0].message.contains("does not match pattern"),
        "{:?}",
        diags[0]
    );
    let diags = eval_inline_strict("schema = { s = String(pattern = \"([\") }\ns = \"x\"\n")
        .expect_err("invalid pattern");
    assert!(
        diags[0].message.contains("invalid pattern"),
        "{:?}",
        diags[0]
    );
}

#[test]
fn bytes_rejects_float_bounds() {
    let diags = eval_inline_strict("schema = { q = Bytes(max = 1.5) }\nq = 3\n")
        .expect_err("float bound on Bytes");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T003");
    assert!(
        diags[0].message.contains("incomparable max bound"),
        "{:?}",
        diags[0]
    );
}

#[test]
fn schema_bugs_report_once_across_array_elements() {
    // Three elements share one broken pattern: a single diagnostic.
    let diags = eval_inline_strict(
        "schema = { xs = Array<String(pattern = \"([\")> }\nxs = [\"a\", \"b\", \"c\"]\n",
    )
    .expect_err("invalid pattern");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(
        diags[0].message.contains("invalid pattern"),
        "{:?}",
        diags[0]
    );
    // Same for an incomparable bound reached through every element.
    let diags = eval_inline_strict("schema = { xs = Array<Int(min = \"low\")> }\nxs = [1, 2, 3]\n")
        .expect_err("incomparable bound");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(
        diags[0].message.contains("incomparable min bound"),
        "{:?}",
        diags[0]
    );
}

#[test]
fn nested_arrays_attribute_to_their_own_level() {
    let src = "schema = {\n  m = Array<Array<Int>>,\n}\nm = [\n  [1, 2],\n  [3, \"x\"],\n]\n";
    let diags = eval_inline_strict(src).expect_err("nested violation");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].path.as_deref(), Some("m[1][1]"));
    // The span sits on the inner array's line, not the outer's.
    let span = diags[0].span.as_ref().expect("span");
    assert_eq!(span.line, 6, "{span:?}");
}

#[test]
fn length_changed_arrays_fall_back_to_schema_spans() {
    // The patch removes an element, breaking positional mapping: the
    // remaining violation attributes to the schema, not the stale slot.
    let src = "schema = { xs = Array<Int(min = 0)> }\nxs = [1, -5]\n@remove(\"xs[0]\")\n";
    let diags = eval_inline_strict(src).expect_err("patched violation");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].path.as_deref(), Some("xs[0]"));
    let span = diags[0].span.as_ref().expect("span");
    assert_eq!(span.line, 1, "{span:?}");
}

#[test]
fn defaults_fill_inside_array_elements() {
    let src = "schema = { nodes = Array<Table { retries = Int(default = 3) }> }\n\
         nodes = [{ name = \"a\" }, { name = \"b\", retries = 1 }]\n";
    let module = parse(src, "inline").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("resilient must succeed: {e:?}"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "W005");
    assert_eq!(diags[0].path.as_deref(), Some("nodes[0].retries"));
    let json = active_core::convert::value_to_pretty_json(&value);
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(v["nodes"][0]["retries"], 3);
    assert_eq!(v["nodes"][1]["retries"], 1);
    // Strict demands the explicit value at the element path.
    let module = parse(src, "inline").expect("must parse");
    let mut strict = Engine::new();
    strict.resilient = false;
    match strict.evaluate(&module) {
        Err(EvalError::ValidationFailed { diagnostics }) => {
            assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
            assert_eq!(diagnostics[0].code, "T004");
            assert_eq!(diagnostics[0].path.as_deref(), Some("nodes[0].retries"));
        }
        other => panic!("must fail strict: {other:?}"),
    }
}

#[test]
fn null_defaults_fill_like_any_other_default() {
    let src = "schema = { x = Null(default = null) }\n";
    let module = parse(src, "inline").expect("must parse");
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("resilient must succeed: {e:?}"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "W005");
    let json = active_core::convert::value_to_pretty_json(&value);
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert!(v["x"].is_null(), "{v}");
}

#[test]
fn filled_value_violations_point_at_the_default() {
    // The bad default lives on schema line 1; the filled T002 must
    // attribute there, not to the (missing) value.
    let src = "schema = { bad = Int(default = \"oops\") }\n";
    let diags = eval_inline_strict(src).expect_err("strict still fails");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T004");
    let module = parse(src, "inline").expect("must parse");
    let (_, diags) = Engine::new()
        .evaluate(&module)
        .expect("resilient must succeed");
    assert_eq!(diags.len(), 2, "{diags:?}");
    assert_eq!(diags[0].code, "W005");
    assert_eq!(diags[1].code, "T002");
    let span = diags[1].span.as_ref().expect("span");
    assert_eq!(span.line, 1, "{span:?}");
}

#[test]
fn resilient_fills_defaults_nested_and_validates_them() {
    let (v, diags) = evaluate_resilient("defaults.active");
    // Five fills (port, host, level, bad, server.retries) + the mistyped
    // default surfacing as T002.
    assert_eq!(diags.len(), 6, "{diags:?}");
    assert_eq!(
        diags.iter().filter(|d| d.code == "W005").count(),
        5,
        "{diags:?}"
    );
    assert_eq!(v["port"], 8080);
    assert_eq!(v["host"], "localhost");
    assert_eq!(v["level"], 3);
    assert_eq!(v["server"]["retries"], 3);
    let bad: Vec<_> = diags.iter().filter(|d| d.code == "T002").collect();
    assert_eq!(bad.len(), 1, "{diags:?}");
    assert_eq!(bad[0].path.as_deref(), Some("bad"));
    // W005 is a warning: resilient exit policy stays green on fills alone.
    assert!(diags
        .iter()
        .filter(|d| d.code == "W005")
        .all(|d| d.severity == active_core::Severity::Warning));
}

#[test]
fn strict_demands_explicit_values_despite_defaults() {
    let diags = evaluate_strict("defaults.active").expect_err("strict fills nothing");
    assert_eq!(diags.len(), 5, "{diags:?}");
    assert!(diags.iter().all(|d| d.code == "T004"), "{diags:?}");
    let paths: Vec<&str> = diags
        .iter()
        .map(|d| d.path.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        paths,
        ["port", "host", "level", "bad", "server.retries"],
        "{diags:?}"
    );
}

#[test]
fn closed_tables_reject_extra_keys() {
    let diags = evaluate_strict("closed.active").expect_err("closed must fail strict");
    assert_eq!(diags.len(), 2, "{diags:?}");
    assert!(diags.iter().all(|d| d.code == "T005"), "{diags:?}");
    // Depth-first schema order: the nested table's extra precedes the
    // root's extra.
    assert_eq!(diags[0].path.as_deref(), Some("server.debug"));
    assert_eq!(diags[1].path.as_deref(), Some("extra"));
    assert!(diags[1].message.contains("'extra'"), "{:?}", diags[1]);
    // Resilient collects but keeps the extras (validation never drops).
    let (v, diags) = evaluate_resilient("closed.active");
    assert_eq!(diags.len(), 2, "{diags:?}");
    assert_eq!(v["extra"], "drift");
    assert_eq!(v["server"]["debug"], true);
}

#[test]
fn closed_tables_accept_exact_keys() {
    eval_inline_strict("schema = { closed = true, port = Int }\nport = 1\n")
        .expect("exact keys pass");
}

use active_core::diag::Severity;

fn strict_diags(src: &str) -> Vec<active_core::Diagnostic> {
    let module = parse(src, "inline").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    match strict_engine().evaluate(&module) {
        Ok(_) => panic!("strict must fail: {src}"),
        Err(EvalError::ValidationFailed { diagnostics }) => diagnostics,
        Err(e) => panic!("unexpected error: {e:?}"),
    }
}

fn resilient_eval(src: &str) -> (serde_json::Value, Vec<active_core::Diagnostic>) {
    let module = parse(src, "inline").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("resilient must succeed: {e:?}"));
    let json = active_core::convert::value_to_pretty_json(&value);
    (serde_json::from_str(&json).expect("valid json"), diags)
}

// --- C-016 phase-2/3 schema wiring ------------------------------------

#[test]
fn closed_table_typo_suggests_in_strict() {
    let diags = strict_diags("schema = { closed = true, port = Int }\nprt = 8080\n");
    let t005 = diags
        .iter()
        .find(|d| d.code == "T005")
        .expect("T005 expected");
    assert!(t005.message.contains("did you mean 'port'?"), "{t005:?}");
    let repair = t005.repair.as_ref().expect("repair attached");
    assert_eq!(repair.suggested, "port");
    assert_eq!(repair.original, "prt");
    // Suggest-only confidence is the match score, never 1.0; the
    // near-identical-edit rule keeps `safe` false here.
    assert!(t005.confidence >= 0.70 && t005.confidence < 1.0, "{t005:?}");
    assert!(!repair.safe, "{t005:?}");
    // Strict never renames: the missing key still fails alongside.
    assert!(diags.iter().any(|d| d.code == "T004"), "{diags:?}");
}

#[test]
fn resilient_renames_confident_typo_and_retracts_absence() {
    let (v, diags) = resilient_eval("schema = { closed = true, port = Int }\nprt = 8080\n");
    assert_eq!(v["port"], 8080);
    assert!(v.get("prt").is_none(), "{v}");
    // Exactly one diagnostic: the H005 warning. The stale T004 for
    // the healed absence is retracted; no T005 remains.
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "H005");
    assert_eq!(diags[0].severity, Severity::Warning);
    assert!(diags[0].confidence >= 0.85, "{:?}", diags[0]);
    assert!(
        diags[0].message.contains("renamed key 'prt' to 'port'"),
        "{:?}",
        diags[0]
    );
}

#[test]
fn resilient_keeps_typo_when_target_taken() {
    let (v, diags) = resilient_eval("schema = { closed = true, port = Int }\nport = 1\nprt = 2\n");
    assert_eq!(v["port"], 1);
    assert_eq!(v["prt"], 2);
    assert!(diags.iter().any(|d| d.code == "T005"), "{diags:?}");
    assert!(!diags.iter().any(|d| d.code == "H005"), "{diags:?}");
}

#[test]
fn resilient_keeps_typo_when_value_mistyped() {
    // `prt` suggests `port`, but the string value does not
    // type-check as Int: suggest-only, no rename, no coerce-across
    // (the value stays under its written key, unvalidated).
    let (v, diags) = resilient_eval("schema = { closed = true, port = Int }\nprt = \"8080\"\n");
    assert_eq!(v["prt"], "8080");
    assert!(v.get("port").is_none(), "{v}");
    assert!(diags.iter().any(|d| d.code == "T005"), "{diags:?}");
    assert!(!diags.iter().any(|d| d.code == "H005"), "{diags:?}");
}

#[test]
fn strict_notes_coercible_mismatch() {
    let diags = strict_diags("schema = { timeout = Duration }\ntimeout = \"5s\"\n");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T002");
    assert!(
        diags[0].message.contains("coercible under --resilient"),
        "{:?}",
        diags[0]
    );
    // No evaluated value in the message (no-echo policy).
    assert!(!diags[0].message.contains("5s"), "{:?}", diags[0]);
}

#[test]
fn resilient_coerces_string_to_target() {
    let (v, diags) = resilient_eval("schema = { timeout = Duration }\ntimeout = \"5s\"\n");
    assert_eq!(v["timeout"]["ms"], 5000, "{v}");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "H007");
    assert_eq!(diags[0].severity, Severity::Warning);
    assert!(
        diags[0].repair.is_none(),
        "applied repairs need no Repair attachment"
    );
}

#[test]
fn resilient_coerced_value_still_constraint_checked() {
    let (v, diags) = resilient_eval("schema = { n = Int(min = 0, max = 10) }\nn = \"500\"\n");
    assert_eq!(v["n"], 500, "{v}");
    // Coerced, then the range check fires on the coerced value.
    assert!(diags.iter().any(|d| d.code == "H007"), "{diags:?}");
    assert!(diags.iter().any(|d| d.code == "T003"), "{diags:?}");
}

#[test]
fn sentinel_word_is_recognized_not_mapped() {
    let diags = strict_diags("schema = { retries = Int }\nretries = \"unlimited\"\n");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T002");
    assert!(
        diags[0]
            .message
            .contains("sentinel 'unlimited' signals limit"),
        "{:?}",
        diags[0]
    );
    // Resilient cannot map it either (no schema syntax designates
    // sentinel values): the violation is collected, tree unchanged.
    let (v, diags) = resilient_eval("schema = { retries = Int }\nretries = \"unlimited\"\n");
    assert_eq!(v["retries"], "unlimited");
    assert!(diags.iter().any(|d| d.code == "T002"), "{diags:?}");
    assert!(!diags.iter().any(|d| d.code == "H007"), "{diags:?}");
}

#[test]
fn nested_rename_revalidates_subtree() {
    // Typos at two levels: the outer rename exposes the inner one,
    // which the fixpoint pass then heals too.
    let src = "schema = { closed = true, server = Table { closed = true, port = Int } }\n\
         serverr = { prot = 1 }\n";
    let (v, diags) = resilient_eval(src);
    assert_eq!(v["server"]["port"], 1, "{v}");
    assert!(v.get("serverr").is_none(), "{v}");
    let h005: Vec<_> = diags.iter().filter(|d| d.code == "H005").collect();
    assert_eq!(h005.len(), 2, "{diags:?}");
    assert!(!diags.iter().any(|d| d.code.starts_with('T')), "{diags:?}");
}

#[test]
fn rename_wins_over_default_fill() {
    // The walk queues both a fill (port missing, has a default) and
    // a rename (prt typo): the healed user value must survive, not
    // be clobbered by the default afterwards.
    let src = "schema = { closed = true, port = Int(default = 1) }\nprt = 8080\n";
    let (v, diags) = resilient_eval(src);
    assert_eq!(v["port"], 8080, "{v}");
    assert!(v.get("prt").is_none(), "{v}");
    assert!(diags.iter().any(|d| d.code == "H005"), "{diags:?}");
    assert!(
        !diags.iter().any(|d| d.code == "W005"),
        "healed path must not fill: {diags:?}"
    );
    // Strict still fails explicitly (T004 + T005, no rename).
    let diags = strict_diags(src);
    assert!(diags.iter().any(|d| d.code == "T004"), "{diags:?}");
    assert!(diags.iter().any(|d| d.code == "T005"), "{diags:?}");
}

#[test]
fn fill_then_coerce_chains_through_fixpoint() {
    // A filled default that needs coercion: the fill pass inserts
    // "5s", revalidation queues a coercion, and the fixpoint loop
    // applies it (a single fill pass would leave the string).
    let src = "schema = { timeout = Duration(default = \"5s\") }\n";
    let (v, diags) = resilient_eval(src);
    assert_eq!(v["timeout"]["ms"], 5000, "{v}");
    assert!(diags.iter().any(|d| d.code == "W005"), "{diags:?}");
    assert!(diags.iter().any(|d| d.code == "H007"), "{diags:?}");
}

#[test]
fn format_validators_accept_clean_values() {
    let v = evaluate_strict("extensions_valid.active").expect("clean formats must pass");
    assert_eq!(v["contact"], "ops@example.com");
    assert_eq!(v["release"], "1.4.0");
    assert_eq!(v["color"], "green");
}

#[test]
fn format_validators_reject_with_codes() {
    let diags = evaluate_strict("extensions_invalid.active").expect_err("must fail strict");
    let details: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
    for want in [
        "does not match format 'email'",
        "does not match format 'uri'",
        "does not match format 'uuid'",
        "does not match format 'semver'",
        "descriptions length (3) must match Enum members (2)",
        "not a member of Enum(\"red\", \"green\")",
        "unknown format 'phone' in schema",
    ] {
        assert!(
            details.iter().any(|m| m.contains(want)),
            "missing {want}: {details:?}"
        );
    }
    assert!(diags.iter().all(|d| d.code == "T003"), "{diags:?}");
}

#[test]
fn unknown_format_reports_once_per_field() {
    // Array elements re-trigger the walk; the schema bug must appear once.
    let src = "schema = { tags = Array<String(format = \"phone\")> }\ntags = [\"a\", \"b\"]\n";
    let diags = strict_diags(src);
    let unknown: Vec<_> = diags
        .iter()
        .filter(|d| d.message.contains("unknown format"))
        .collect();
    assert_eq!(unknown.len(), 1, "{diags:?}");
}

#[test]
fn deprecated_defaulted_fields_skip_strict_explicitness() {
    // Omitting a deprecated key is the migration path, not a
    // violation: strict leaves it absent, resilient fills it (W005).
    // Explicit use still warns via L005 (pinned in lint.rs).
    let v = evaluate_strict("deprecated_omitted.active")
        .expect("deprecated+default may be omitted in strict");
    assert_eq!(v["keep"], "k");
    assert!(v.get("old").is_none(), "{v}");
    let (v, diags) = evaluate_resilient("deprecated_omitted.active");
    assert_eq!(v["old"], "o");
    assert!(diags.iter().any(|d| d.code == "W005"), "{diags:?}");
}

#[test]
fn requires_fires_only_when_trigger_present() {
    let diags = evaluate_strict("requires_triggered.active").expect_err("must fail strict");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "T004" && d.message.contains("required by present 'mode'")),
        "{diags:?}"
    );
    // Trigger absent: no requires diagnostic.
    let src = "schema = { mode = String, replicas = Int(requires = \"mode\") }\n";
    let (v, diags) = resilient_eval(src);
    assert!(v.get("replicas").is_none(), "{v}");
    assert!(
        !diags
            .iter()
            .any(|d| d.message.contains("required by present")),
        "{diags:?}"
    );
}

#[test]
fn requires_yields_to_resilient_default_fill() {
    let (v, diags) = evaluate_resilient("requires_filled.active");
    assert_eq!(v["replicas"], 1, "{v}");
    assert!(diags.iter().any(|d| d.code == "W005"), "{diags:?}");
    assert!(!diags.iter().any(|d| d.code == "T004"), "{diags:?}");
}

#[test]
fn description_metadata_is_ignored_by_verdicts() {
    let src = "schema = { host = String(description = \"DNS name.\") }\nhost = \"example.com\"\n";
    let (v, diags) = resilient_eval(src);
    assert_eq!(v["host"], "example.com");
    assert!(diags.is_empty(), "{diags:?}");
}
