//! Number-literal normalisation pins (LANG-A-008).
//!
//! The decision is to BLESS and document the grammar's existing rules:
//! underscores are stripped anywhere and leading zeros are stripped, so
//! `010` is ten (never octal). These tests make the decision executable
//! so a future tightening cannot land silently.

use active_core::{parse, Engine, Value};

fn int_of(src: &str) -> i64 {
    let module = parse(src, "nums").expect("parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    match value {
        Value::Table(t) => match t.get("a") {
            Some(Value::Int(n)) => *n,
            other => panic!("unexpected value: {other:?}"),
        },
        other => panic!("unexpected root: {other:?}"),
    }
}

#[test]
fn underscores_are_stripped_anywhere() {
    assert_eq!(int_of("a = 1_000\n"), 1000);
    assert_eq!(int_of("a = 1_\n"), 1);
    assert_eq!(int_of("a = 1__000\n"), 1000);
}

#[test]
fn leading_zeros_are_decimal_never_octal() {
    assert_eq!(int_of("a = 010\n"), 10);
    assert_eq!(int_of("a = 007\n"), 7);
    assert_eq!(int_of("a = 0\n"), 0);
}
