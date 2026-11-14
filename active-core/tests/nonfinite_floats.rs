//! Non-finite float rejection (LANG-B-009, LANG-B-901, LANG-F-003).
//!
//! A float that cannot be represented is an error at every door: parse
//! (E023), arithmetic (T001), and resilient coercion (rejected). The
//! exporters never turn a number into `null`.

use active_core::{parse, Engine};

/// A decimal literal far past f64::MAX (~1.8e308).
fn overflowing_literal() -> String {
    format!("1{}.0", "0".repeat(400))
}

#[test]
fn an_overflowing_literal_is_a_parse_error_not_infinity() {
    let src = format!("x = {}\n", overflowing_literal());
    let diags = parse(&src, "floats").expect_err("must fail to parse");
    assert_eq!(diags[0].code, "E023", "{diags:?}");
    assert!(
        diags[0].message.contains("float literal out of range"),
        "{}",
        diags[0].message
    );
}

#[test]
fn arithmetic_overflow_is_a_typed_error_not_null() {
    // A maximal literal is representable; multiplying it by itself is
    // not, and that must be T001 rather than an `inf` that exports null.
    let src = format!("a = {}.0\nx = a * a\n", "9".repeat(307));
    let module = parse(&src, "floats").expect("parse");
    let err = Engine::new().evaluate(&module).expect_err("must fail");
    assert_eq!(err.code(), "T001");
    assert!(format!("{err}").contains("overflow"), "{err}");
}

#[test]
fn resilient_coercion_rejects_non_finite_spellings() {
    for spelling in ["NaN", "inf", "-inf", "+inf", "infinity"] {
        let src = format!("schema = {{ x = Float }}\nx = \"{spelling}\"\n");
        let module = parse(&src, "floats").expect("parse");
        let mut engine = Engine::new();
        engine.resilient = true;
        let (_value, diags) = engine.evaluate(&module).expect("resilient evaluate");
        assert!(
            diags.iter().any(|d| d.code == "T002"),
            "{spelling} must keep the type error: {diags:?}"
        );
        assert!(
            !diags.iter().any(|d| d.code == "H007"),
            "{spelling} must not coerce: {diags:?}"
        );
    }
    // Control: an ordinary numeric string still coerces.
    let module = parse("schema = { x = Float }\nx = \"2.5\"\n", "floats").expect("parse");
    let mut engine = Engine::new();
    engine.resilient = true;
    let (value, diags) = engine.evaluate(&module).expect("resilient evaluate");
    assert!(diags.iter().any(|d| d.code == "H007"), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("2.5"), "{json}");
}

#[test]
fn a_representable_float_exports_as_a_number() {
    let module = parse("x = 1.5\n", "floats").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("1.5"), "{json}");
    assert!(!json.contains("null"), "{json}");
}
