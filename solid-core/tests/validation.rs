//! Schema validation tests (C-031, spec/validation.md).
//!
//! Drives the real shipped path (`parse_file` → `Engine::evaluate`) on
//! committed fixtures under `tests/validation/`. Covers full-document
//! acceptance, every violation code (T002/T003/T004), strict fatality
//! with all violations collected, resilient pass-through, deterministic
//! ordering, post-patch validation, span attribution, and the secret
//! invariant (no evaluated value ever reaches a message).

use solid_core::{parse, parse_file, Engine, EvalError, ParseOptions};
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

fn evaluate_strict(name: &str) -> Result<serde_json::Value, Vec<solid_core::Diagnostic>> {
    let module = parse_file(&fixture(name), ParseOptions::default())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    match strict_engine().evaluate(&module) {
        Ok((value, diags)) => {
            assert!(diags.is_empty(), "{name}: unexpected diags: {diags:?}");
            let json = solid_core::convert::value_to_pretty_json(&value);
            Ok(serde_json::from_str(&json).expect("valid json"))
        }
        Err(EvalError::ValidationFailed { diagnostics }) => Err(diagnostics),
        Err(e) => panic!("{name}: unexpected error: {e:?}"),
    }
}

fn evaluate_resilient(name: &str) -> (serde_json::Value, Vec<solid_core::Diagnostic>) {
    let module = parse_file(&fixture(name), ParseOptions::default())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: resilient must succeed: {e:?}"));
    let json = solid_core::convert::value_to_pretty_json(&value);
    (serde_json::from_str(&json).expect("valid json"), diags)
}

#[test]
fn valid_document_passes_in_both_modes() {
    let v = evaluate_strict("valid_full.solid").expect("valid doc must pass strict");
    assert_eq!(v["port"], 8080);
    assert_eq!(v["mode"], "prod");
    assert_eq!(v["tags"], serde_json::json!(["a", "b"]));
    assert_eq!(v["server"]["port"], 5432);
    assert!(v.get("schema").is_none());
    let (_, diags) = evaluate_resilient("valid_full.solid");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn strict_collects_every_violation_then_fails() {
    let diags = evaluate_strict("invalid_mixed.solid").expect_err("must fail strict");
    let codes: Vec<&str> = diags.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(
        codes,
        ["T003", "T002", "T003", "T002", "T002", "T002", "T004"],
        "schema order, all collected: {diags:?}"
    );
    assert!(diags
        .iter()
        .all(|d| d.severity == solid_core::Severity::Error));
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
            "missing_no_default"
        ],
        "{diags:?}"
    );
    // Direct data lines up with the arena: every violation has a span.
    assert!(diags.iter().all(|d| d.span.is_some()), "{diags:?}");
    // The defaulted key stays silent; messages quote the enum contract.
    assert!(diags.iter().all(|d| !d.message.contains("with_default")));
    let mode = &diags[2];
    assert!(mode.message.contains("\"dev\""), "{mode:?}");
    assert!(!mode.message.contains("staging"), "{mode:?}");
}

#[test]
fn strict_error_carries_first_code_and_span() {
    let module =
        parse_file(&fixture("invalid_mixed.solid"), ParseOptions::default()).expect("must parse");
    let err = strict_engine().evaluate(&module).expect_err("must fail");
    assert_eq!(err.code(), "T003");
    assert!(err.span().is_some(), "first violation span");
}

#[test]
fn resilient_collects_but_exports_unchanged() {
    let (v, diags) = evaluate_resilient("invalid_mixed.solid");
    assert_eq!(diags.len(), 7, "{diags:?}");
    // No filling, no dropping: the tree passes through as evaluated.
    assert_eq!(v["port"], 999);
    assert_eq!(v["tags"][1], "two");
    assert!(v.get("missing_no_default").is_none());
}

#[test]
fn validation_is_deterministic() {
    let run = || {
        let module = parse_file(&fixture("invalid_mixed.solid"), ParseOptions::default())
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
    let diags = evaluate_strict("patch_then_validate.solid").expect_err("patched value invalid");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T003");
    assert_eq!(diags[0].path.as_deref(), Some("port"));
    assert!(diags[0].message.contains("above maximum"), "{:?}", diags[0]);
}

#[test]
fn secret_values_never_reach_messages() {
    let diags = evaluate_strict("secret_mismatch.solid").expect_err("must fail");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T002");
    for d in &diags {
        assert!(!d.message.contains("s3cr3t"), "{d:?}");
        assert!(!d.message.contains("hunter2"), "{d:?}");
    }
}

fn eval_inline_strict(src: &str) -> Result<(), Vec<solid_core::Diagnostic>> {
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
