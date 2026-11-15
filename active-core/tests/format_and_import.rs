//! Import round-trip and format-validator boundaries (LANG-B-007, B-013).

use active_core::{parse, Engine};

fn format_ok(kind: &str, value: &str) -> bool {
    let src = format!("schema = {{ v = String(format = \"{kind}\") }}\nv = \"{value}\"\n");
    let module = parse(&src, "fmt").expect("parse");
    let (_v, diags) = Engine::new().evaluate(&module).expect("evaluate");
    !diags.iter().any(|d| d.code == "T003")
}

#[test]
fn email_accepts_single_label_domains_and_short_labels() {
    for v in ["user@localhost", "a@b.c", "first.last@example.com"] {
        assert!(format_ok("email", v), "{v} must be a valid email");
    }
    for v in ["no-at-sign", "@example.com", "user@", "user@@x"] {
        assert!(!format_ok("email", v), "{v} must be rejected");
    }
}

#[test]
fn uri_accepts_any_scheme_form() {
    for v in [
        "https://example.com/x",
        "mailto:a@b.co",
        "urn:isbn:1234567890",
        "file:/tmp/x",
    ] {
        assert!(format_ok("uri", v), "{v} must be a valid uri");
    }
    for v in ["://missing-scheme", "1nvalid:scheme", "no-colon"] {
        assert!(!format_ok("uri", v), "{v} must be rejected");
    }
}

#[test]
fn i64_min_round_trips_through_import() {
    // The importer must emit source the language can parse: the plain
    // digits are E023 because the literal does not fit i64.
    let doc = serde_json::json!({ "a": i64::MIN, "b": i64::MAX });
    let (text, _notes) = active_core::convert::import::json_value_to_active(&doc);
    assert!(text.contains("-9223372036854775807 - 1"), "{text}");
    let module = parse(&text, "roundtrip").expect("imported source must parse");
    let (value, _diags) = Engine::new().evaluate(&module).expect("evaluate");
    let json: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(json["a"], i64::MIN, "{json}");
    assert_eq!(json["b"], i64::MAX, "{json}");
}

#[test]
fn sanitized_key_collisions_keep_every_value() {
    // `café` and `caf_` both sanitize to `caf_`; the importer must not
    // drop one value. The second is suffixed and announced (LANG-B-008).
    let doc = serde_json::json!({ "café": 1, "caf_": 2 });
    let (text, notes) = active_core::convert::import::json_value_to_active(&doc);
    assert_eq!(text.matches("caf_").count(), 2, "{text}");
    assert!(text.contains("caf__2 = 2"), "{text}");
    assert!(
        notes
            .iter()
            .any(|d| d.code == "W007" && d.message.contains("collided")),
        "{notes:?}"
    );
    // Both values survive the round trip.
    let module = parse(&text, "collision").expect("parse");
    let (value, _d) = Engine::new().evaluate(&module).expect("evaluate");
    let json: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(json["caf_"], 1, "{json}");
    assert_eq!(json["caf__2"], 2, "{json}");
}

#[test]
fn schema_field_collisions_are_disambiguated() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "café": {"type": "string"}, "caf_": {"type": "string"} }
    });
    let (text, notes) =
        active_core::convert::import::jsonschema_to_active(&schema.to_string()).expect("import");
    assert!(text.contains("caf__2"), "{text}");
    assert!(
        notes
            .iter()
            .any(|d| d.code == "W007" && d.message.contains("collided")),
        "{notes:?}"
    );
}

#[test]
fn duplicate_keys_follow_one_documented_policy_per_format() {
    // YAML rejects duplicates (the YAML spec makes them an error) and the
    // message names the JSON difference; JSON keeps the last value because
    // its document model cannot see duplicates (LANG-B-012).
    let err = active_core::convert::import::yaml_text_to_active("a: 1\na: 2\n")
        .expect_err("YAML duplicates must fail");
    assert!(err.contains("duplicate mapping keys"), "{err}");
    assert!(err.contains("JSON import keeps the last value"), "{err}");

    let (text, _notes) =
        active_core::convert::import::json_text_to_active("{\"a\": 1, \"a\": 2}").expect("json");
    assert_eq!(text.trim(), "a = 2", "{text}");
}

#[test]
fn quantities_round_trip_through_the_json_wrapper() {
    // LANG-B-019: `{"%": n}` was table-shaped and `%` is not an
    // identifier, so the unit was mangled on import. The reserved wrapper
    // is a fixed point for every quantity kind.
    for src in [
        "p = 25%\n",
        "r = 100req/s\n",
        "t = 500tok/s\n",
        "f = 2kHz\n",
        "pr = 2Pa\n",
    ] {
        let module = parse(src, "qty").expect("parse");
        let (value, _d) = Engine::new().evaluate(&module).expect("evaluate");
        let json = active_core::convert::value_to_pretty_json(&value);
        let (imported, _notes) =
            active_core::convert::import::json_text_to_active(&json).expect("import");
        let back = parse(&imported, "qty-back").expect("imported source parses");
        let (value2, _d2) = Engine::new().evaluate(&back).expect("evaluate");
        let json2 = active_core::convert::value_to_pretty_json(&value2);
        assert_eq!(json, json2, "round trip for {src:?}");
    }
}
