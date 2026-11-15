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
