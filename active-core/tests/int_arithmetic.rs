//! Integer arithmetic policy (LANG-B-023).
//!
//! Saturation is the deliberate, documented policy (spec/specification.md
//! §3.3): integer `+`/`-`/`*` and unary `-` clamp at the i64 bounds, and
//! `i64::MIN / -1` clamps at `i64::MAX`. It never panics and never wraps,
//! and it is not an error. Float overflow is the opposite: non-finite
//! results are T001 (LANG-B-901), because a float has no honest clamp.

use active_core::{parse, Engine};

fn eval_int(src: &str) -> (i64, Vec<String>) {
    let module = parse(src, "int").expect("parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    let v: serde_json::Value = serde_json::from_str(&json).expect("json");
    (
        v["x"].as_i64().expect("int result"),
        diags.iter().map(|d| d.code.clone()).collect(),
    )
}

#[test]
fn integer_addition_saturates_without_a_diagnostic() {
    let (v, codes) = eval_int("x = 9223372036854775807 + 1\n");
    assert_eq!(v, i64::MAX);
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn integer_multiplication_and_subtraction_saturate() {
    let (v, _c) = eval_int("x = 4611686018427387904 * 4\n");
    assert_eq!(v, i64::MAX);
    let (v, _c) = eval_int("x = -9223372036854775807 - 2\n");
    assert_eq!(v, i64::MIN);
}

#[test]
fn min_divided_by_minus_one_saturates_at_max() {
    // `i64::MIN` has no literal, so reach it as MIN+1 - 1 (B-007's shape).
    let (v, codes) = eval_int("m = -9223372036854775807 - 1\nx = m / -1\n");
    assert_eq!(v, i64::MAX);
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn unary_negation_of_min_saturates() {
    let (v, _c) = eval_int("m = -9223372036854775807 - 1\nx = -m\n");
    assert_eq!(v, i64::MAX);
}
