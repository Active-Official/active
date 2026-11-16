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

#[test]
fn the_export_is_injective_for_non_finite_floats_and_null() {
    // LANG-F-003: the export tags a non-finite float as an object, so it
    // cannot collide with null (the serde default) or with a genuine string
    // `"inf"`. content_hash hashes that tagged form, so the identities
    // differ too.
    use active_core::convert::value_to_pretty_json;
    use active_core::provenance::content_hash;
    use active_core::Value;
    let inf = Value::Float(f64::INFINITY);
    let neg = Value::Float(f64::NEG_INFINITY);
    let nan = Value::Float(f64::NAN);
    assert_eq!(
        value_to_pretty_json(&inf),
        "{\n  \"$active.float\": \"inf\"\n}"
    );
    assert_eq!(
        value_to_pretty_json(&neg),
        "{\n  \"$active.float\": \"-inf\"\n}"
    );
    assert_eq!(
        value_to_pretty_json(&nan),
        "{\n  \"$active.float\": \"nan\"\n}"
    );

    let null_hash = content_hash(&Value::Null);
    let string_hash = content_hash(&Value::String("inf".into()));
    for value in [inf, neg, nan] {
        let hash = content_hash(&value);
        assert_ne!(hash, null_hash, "inf must not share null's identity");
        assert_ne!(hash, string_hash, "inf must not share the string \"inf\"");
    }
    assert_eq!(
        content_hash(&Value::Float(f64::INFINITY)),
        content_hash(&Value::Float(f64::INFINITY)),
        "the tagged identity is deterministic"
    );
}

#[test]
fn importing_a_non_finite_wrapper_is_loud_and_never_silent_null() {
    // Active source has no non-finite literal, so the importer cannot
    // round-trip the value: it imports null and records W010. The negative
    // control is the silence this replaces (serde turned inf into null with
    // no note).
    let json = "{\"x\": {\"$active.float\": \"inf\"}}";
    let (source, diags) = active_core::convert::json_text_to_active(json).expect("import succeeds");
    assert!(source.contains("x = null"), "{source}");
    assert!(
        diags.iter().any(|d| d.code == "W010"),
        "the loss is announced: {diags:?}"
    );
    // A genuine single-key table with an unrelated key is data, not a tag.
    let json = "{\"x\": {\"$active.float\": \"sideways\"}}";
    let (source, diags) = active_core::convert::json_text_to_active(json).expect("import succeeds");
    assert!(!source.contains("W010"), "{source}");
    assert!(!diags.iter().any(|d| d.code == "W010"), "{diags:?}");
}
