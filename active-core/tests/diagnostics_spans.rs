//! Diagnostic quality tests (C-015).
//!
//! Proves on the real shipped path that every diagnostic class carries a
//! stable code and a precise source span: parse errors (E0xx), evaluation
//! type errors (T0xx), and unresolved-identifier warnings (W0xx).

use active_core::{parse, Engine};

#[test]
fn parse_errors_carry_code_and_span() {
    let src = "[app\nname = 1\n";
    let diags = parse(src, "spans").expect_err("unclosed table header must fail");
    let d = diags
        .iter()
        .find(|d| d.code == "E001")
        .expect("E001 expected");
    assert_eq!(d.severity, active_core::diag::Severity::Error);
    // The span points at the offending token — the Newline at column 5,
    // immediately after `[app` where `]` was expected.
    let span = d.span.as_ref().expect("parse diagnostics must carry spans");
    assert_eq!(
        (span.line, span.column),
        (1, 5),
        "span must point at the unexpected Newline token"
    );
}

#[test]
fn e004_depth_error_carries_code_and_span() {
    let src = format!("x = {}1{}", "(".repeat(200), ")".repeat(200));
    let diags = parse(&src, "spans").expect_err("depth limit must fail");
    let d = diags
        .iter()
        .find(|d| d.code == "E004")
        .expect("E004 expected");
    assert!(d.span.is_some(), "E004 must carry a span");
}

#[test]
fn e003_bad_secret_env_carries_code_and_span() {
    let src = "x = secret.env(42)\n";
    let diags = parse(src, "spans").expect_err("non-literal secret.env arg must fail");
    let d = diags
        .iter()
        .find(|d| d.code == "E003")
        .expect("E003 expected");
    assert!(d.span.is_some(), "E003 must carry a span");
}

#[test]
fn unresolved_identifier_warning_carries_code_and_span() {
    // The identifier sits at line 1, column 8 (`x = missing`).
    let src = "x = missing\n";
    let module = parse(src, "spans").expect("must parse");
    let engine = Engine::new();
    let (value, diags) = engine.evaluate(&module).expect("must evaluate");
    let d = diags
        .iter()
        .find(|d| d.code == "W001")
        .expect("W001 expected");
    assert_eq!(d.severity, active_core::diag::Severity::Warning);
    let span = d
        .span
        .as_ref()
        .expect("W001 must carry a span via node_spans");
    assert_eq!(span.line, 1, "span must point at line 1");
    assert_eq!(span.column, 5, "span must point at the identifier start");
    // The value degrades to null rather than failing.
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(
        json.contains("null"),
        "unresolved value must be null: {json}"
    );
}

#[test]
fn type_error_carries_code_and_span() {
    // `x = !5` — the unary op starts at line 1, column 5.
    let src = "x = !5\n";
    let module = parse(src, "spans").expect("must parse");
    let engine = Engine::new();
    let err = engine
        .evaluate(&module)
        .expect_err("type error must abort evaluation");
    assert_eq!(err.code(), "T001");
    let span = err.span().expect("T001 must carry a span via node_spans");
    assert_eq!((span.line, span.column), (1, 5));
}

#[test]
fn secret_leak_error_has_stable_code() {
    let src = "let p = secret(\"k\")\nurl = \"http://${p}\"\n";
    let module = parse(src, "spans").expect("must parse");
    let engine = Engine::new();
    let err = engine
        .evaluate(&module)
        .expect_err("secret interpolation must be refused");
    assert_eq!(err.code(), "S004");
}

#[test]
fn eval_errors_have_stable_codes() {
    // Cycle detection before any value is produced.
    let module = parse("let a = b\nlet b = a\n", "spans").expect("must parse");
    let err = Engine::new()
        .evaluate(&module)
        .expect_err("cycle must fail");
    assert_eq!(err.code(), "S001");
}
