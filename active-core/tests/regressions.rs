//! Regression tests for fixed bugs. Each test names the bug it guards.

use active_core::{parse, parse_recovering, Engine, ParseOptions, Value};

fn eval_json(src: &str) -> String {
    let module = parse(src, "regression").expect("must parse");
    let engine = Engine::new();
    let (value, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    active_core::convert::value_to_pretty_json(&value)
}

#[test]
fn sibling_self_reference_falls_through_to_let_binding() {
    // Bug: `[app] name = name` resolved the key's own value to its own field,
    // creating a self-loop cycle. It must resolve to the `let` binding.
    let json = eval_json("let name = \"outer\"\n[app]\nname = name\n");
    assert!(
        json.contains("\"outer\""),
        "app.name must equal the let binding, got: {json}"
    );
}

#[test]
fn sibling_self_reference_without_let_yields_null_not_cycle() {
    // No let binding exists; the self-reference must be a warning + null,
    // not a cycle error.
    let module = parse("[app]\nname = name\n", "regression").expect("must parse");
    let engine = Engine::new();
    let (value, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(
        !diags.is_empty(),
        "unresolved self-reference should produce a diagnostic"
    );
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("null"), "expected null value, got: {json}");
}

#[test]
fn sibling_reference_to_other_key_still_resolves() {
    // Regression guard for the fix above: normal sibling references must
    // keep working, including forward references.
    let json = eval_json("[limits]\nmax = 10\ncan_scale = !is_prod\nis_prod = false\n");
    assert!(
        json.contains("\"can_scale\": true") && json.contains("\"max\": 10"),
        "sibling references must resolve, got: {json}"
    );
}

#[test]
fn integer_literal_overflow_is_e023_not_silent_zero() {
    // Bug (found via toml-test integer/long): `tok.text.parse::<i64>()`
    // used `unwrap_or(0)`, so `9223372036854775808` silently became `0`
    // — including `-9223372036854775808` (valid i64::MIN as TOML).
    // Strict parsing must fail loudly with E023 at the literal's span.
    let diags = parse("x = 9223372036854775808\n", "regression").expect_err("must fail");
    let hit = diags
        .iter()
        .find(|d| d.code == "E023")
        .expect("E023 expected");
    let span = hit.span.clone().expect("E023 carries a span");
    assert_eq!((span.line, span.column), (1, 5), "span: {span:?}");
    assert!(hit.message.contains("9223372036854775808"), "{hit:?}");
}

#[test]
fn integer_overflow_saturates_at_max_under_recovery() {
    // Recovery keeps the document usable: the value saturates at
    // i64::MAX and the E023 diagnostic is preserved for tooling.
    let (module, diags) = parse_recovering(
        "x = 9223372036854775808\n",
        "regression",
        ParseOptions::default(),
    );
    assert!(diags.iter().any(|d| d.code == "E023"), "{diags:?}");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(
        json.contains("9223372036854775807"),
        "saturates at MAX, got: {json}"
    );
}

#[test]
fn unary_minus_on_overflowed_literal_evaluates_min_plus_one() {
    // `-` is a separate operator over the saturated literal, so the
    // classic i64::MIN spelling evaluates to `i64::MIN + 1` — loudly.
    let (module, diags) = parse_recovering(
        "x = -9223372036854775808\n",
        "regression",
        ParseOptions::default(),
    );
    assert!(diags.iter().any(|d| d.code == "E023"), "{diags:?}");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let table = match value {
        Value::Table(t) => t,
        other => panic!("expected table, got {other:?}"),
    };
    match table.get("x") {
        Some(Value::Int(n)) => assert_eq!(*n, i64::MIN + 1, "negated saturation"),
        other => panic!("expected Int, got {other:?}"),
    }
}

#[test]
fn integer_max_parses_cleanly_with_no_e023() {
    // Boundary guard for the fix above: i64::MAX itself is exact and silent.
    let module = parse("x = 9223372036854775807\n", "regression").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let table = match value {
        Value::Table(t) => t,
        other => panic!("expected table, got {other:?}"),
    };
    match table.get("x") {
        Some(Value::Int(n)) => assert_eq!(*n, i64::MAX, "exact boundary"),
        other => panic!("expected Int, got {other:?}"),
    }
}

fn int_field(src: &str, key: &str) -> i64 {
    let module = parse(src, "regression").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    match value {
        Value::Table(t) => match t.get(key) {
            Some(Value::Int(n)) => *n,
            other => panic!("expected Int for {key}, got {other:?}"),
        },
        other => panic!("expected table, got {other:?}"),
    }
}

#[test]
fn int_arithmetic_saturates_never_panics_or_wraps() {
    // Audit find: `m * 2` with `m = i64::MAX` panicked in debug builds
    // (overflow checks) and would wrap in release. All integer ops now
    // saturate, consistent with the DurationMs arms.
    assert_eq!(
        int_field("let m = 9223372036854775807\nx = m + 1\n", "x"),
        i64::MAX
    );
    assert_eq!(
        int_field("let m = -9223372036854775807 - 1\nx = m - 1\n", "x"),
        i64::MIN
    );
    assert_eq!(
        int_field("let m = 9223372036854775807\nx = m * 2\n", "x"),
        i64::MAX
    );
    assert_eq!(
        int_field("let m = -9223372036854775807 - 1\nx = m * 2\n", "x"),
        i64::MIN
    );
    // i64::MIN / -1 is the only overflowing division: saturates at MAX.
    assert_eq!(
        int_field("let m = -9223372036854775807 - 1\nx = m / -1\n", "x"),
        i64::MAX
    );
    // Negation of MIN saturates (never panics).
    assert_eq!(
        int_field("let m = -9223372036854775807 - 1\nx = -m\n", "x"),
        i64::MAX
    );
    // Ordinary arithmetic is untouched by saturation.
    assert_eq!(int_field("x = 6 * 7 - 2\n", "x"), 40);
}

#[test]
fn huge_byte_quantities_saturate_not_wrap_negative() {
    // Audit find: `bytes as i64` wrapped u64 counts past i64::MAX into
    // negative values (`20000000TiB` → negative). Now saturates at MAX.
    assert_eq!(int_field("x = 20000000TiB\n", "x"), i64::MAX);
    assert_eq!(int_field("x = 512MiB\n", "x"), 536870912);
}

#[test]
fn huge_durations_evaluate_without_panic() {
    // 9999999999999999 days overflows u64 milliseconds: saturates at
    // u64::MAX (f64→u64 `as` saturates) instead of panicking.
    let module = parse("x = 9999999999999999d\n", "regression").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(
        json.contains(&u64::MAX.to_string()),
        "saturates at u64::MAX ms, got: {json}"
    );
}
