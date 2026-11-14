//! Statement-boundary honesty tests (LANG-A-005).
//!
//! A token that follows a complete value on the same line is no longer
//! absorbed in silence: the parser announces it (W015) and keeps the
//! documented value. A missing operand after an operator is E001 at
//! parse time instead of a null that fails later as a type error. The
//! two pinned §4.1 quirks (`x = 5 5`, `d = 1.`) stay silent.

use active_core::{parse, Engine, Severity, Value};

fn codes(src: &str) -> Vec<String> {
    match parse(src, "stmt") {
        Ok(m) => m.healing.iter().map(|d| d.code.clone()).collect(),
        Err(diags) => diags.iter().map(|d| d.code.clone()).collect(),
    }
}

fn eval(src: &str) -> Value {
    let module = parse(src, "stmt").expect("recovering parse");
    Engine::new().evaluate(&module).expect("evaluate").0
}

#[test]
fn stray_numbers_are_announced_not_silently_dropped() {
    assert_eq!(codes("a = 1 2 3\n"), vec!["W015", "W015"]);
    match eval("a = 1 2 3\n") {
        Value::Table(t) => assert!(matches!(t.get("a"), Some(Value::Int(1)))),
        other => panic!("unexpected root: {other:?}"),
    }
}

#[test]
fn dotted_number_runs_are_announced() {
    let c = codes("e = 1.2.3\n");
    assert!(!c.is_empty() && c.iter().all(|x| x == "W015"), "{c:?}");
}

#[test]
fn a_second_statement_on_the_same_line_is_announced() {
    let c = codes("a = 1 b = 2\n");
    assert_eq!(c, vec!["W015"]);
    match eval("a = 1 b = 2\n") {
        Value::Table(t) => {
            assert!(matches!(t.get("a"), Some(Value::Int(1))));
            assert!(matches!(t.get("b"), Some(Value::Int(2))));
        }
        other => panic!("unexpected root: {other:?}"),
    }
}

#[test]
fn a_missing_operand_is_a_parse_error_not_a_null() {
    let diags = parse("a = 1 +\n", "stmt").expect_err("must fail at parse time");
    assert_eq!(diags[0].code, "E001", "{diags:?}");
    assert_eq!(diags[0].severity, Severity::Error);
    // Every operator family is covered by the same guard.
    for src in ["a = 1 *\n", "a = 1 &&\n", "a = 1 <\n"] {
        assert!(
            parse(src, "stmt").is_err(),
            "{src:?} must fail at parse time"
        );
    }
}

#[test]
fn the_two_pinned_quirks_stay_silent() {
    assert_eq!(codes("x = 5 5\n"), Vec::<String>::new());
    assert_eq!(codes("d = 1.\n"), Vec::<String>::new());
    // And they keep their documented values.
    match eval("x = 5 5\n") {
        Value::Table(t) => assert!(matches!(t.get("x"), Some(Value::Int(5)))),
        other => panic!("unexpected root: {other:?}"),
    }
}

#[test]
fn comma_separated_members_stay_legal() {
    assert_eq!(
        codes("profile \"p\" { x = 1, y = 2 }\n"),
        Vec::<String>::new()
    );
    assert_eq!(codes("server { host = \"a\" }\n"), Vec::<String>::new());
}
