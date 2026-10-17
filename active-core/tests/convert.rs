//! Converter suite tests (C-046, spec/converters.md).
//!
//! Export shapes for every value kind in all three formats,
//! validity of TOML/YAML output (re-parsed, not string-matched),
//! uniform secret redaction, reverse-direction fixtures with every
//! lossy corner and its code, and the JSON round-trip fixed point.

use active_core::convert::{
    json_text_to_active, json_value_to_active, toml_text_to_active, value_to_pretty_json,
    value_to_toml_string, value_to_yaml_string, yaml_text_to_active,
};
use active_core::{parse, Engine};

fn eval(src: &str) -> active_core::Value {
    let module = parse(src, "convert").expect("must parse");
    Engine::new().evaluate(&module).expect("must evaluate").0
}

fn eval_json(src: &str) -> serde_json::Value {
    let json: serde_json::Value =
        serde_json::from_str(&value_to_pretty_json(&eval(src))).expect("valid json");
    json
}

fn codes(diags: &[active_core::Diagnostic]) -> Vec<&str> {
    diags.iter().map(|d| d.code.as_str()).collect()
}

// ------------------------------------------------------------------
// Export.
// ------------------------------------------------------------------

#[test]
fn export_shapes_cover_every_value_kind() {
    let value = eval(
        "port = 8080\nratio = 0.5\nname = \"svc\"\nflag = true\nnothing = null\nwait = 5s\nsize = 5MiB\n[server]\nhost = \"h\"\n",
    );
    let json = eval_json("port = 1\n");
    let _ = json;
    let pretty = value_to_pretty_json(&value);
    assert!(pretty.contains("\"port\": 8080"), "{pretty}");
    assert!(pretty.contains("\"ms\": 5000"), "{pretty}");
    // Byte quantities normalize to integer bytes at evaluation
    // (like everywhere else in the toolchain).
    assert!(pretty.contains("\"size\": 5242880"), "{pretty}");
    assert!(pretty.contains("\"host\": \"h\""), "{pretty}");

    let yaml = value_to_yaml_string(&value).expect("yaml");
    assert!(yaml.contains("port: 8080"), "{yaml}");
    assert!(yaml.contains("ms: 5000"), "{yaml}");
    assert!(yaml.contains("size: 5242880"), "{yaml}");
    assert!(!yaml.contains("s3cr3t"), "{yaml}");
    // YAML re-parses to the same model.
    let back: serde_json::Value =
        serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(&yaml).expect("reparse"))
            .expect("to json");
    let canonical: serde_json::Value = serde_json::from_str(&pretty).expect("canonical");
    assert_eq!(back, canonical);

    let (toml, diags) = value_to_toml_string(&value).expect("toml");
    assert!(diags.iter().any(|d| d.code == "W006"), "{diags:?}");
    assert!(!toml.contains("nothing"), "{toml}");
    assert!(toml.contains("\"5000ms\""), "{toml}");
    assert!(toml.contains("size = 5242880"), "{toml}");
    // TOML re-parses as a document.
    let reparsed: toml::Value = toml::from_str(&toml).expect("valid toml");
    assert!(reparsed.get("server").is_some(), "{reparsed:?}");
}

#[test]
fn export_redacts_secrets_uniformly() {
    let value = eval("token = secret(\"s3cr3t-convert\")\nport = 1\n");
    let json = value_to_pretty_json(&value);
    assert!(!json.contains("s3cr3t-convert"), "{json}");
    assert!(json.contains("***SECRET***"), "{json}");
    let yaml = value_to_yaml_string(&value).expect("yaml");
    assert!(!yaml.contains("s3cr3t-convert"), "{yaml}");
    assert!(yaml.contains("***SECRET***"), "{yaml}");
    let (toml, _) = value_to_toml_string(&value).expect("toml");
    assert!(!toml.contains("s3cr3t-convert"), "{toml}");
    assert!(toml.contains("***SECRET***"), "{toml}");
}

#[test]
fn toml_arrays_of_tables_emit_double_brackets() {
    let value = eval("items = [{ name = \"a\" }, { name = \"b\" }]\n");
    let (toml, diags) = value_to_toml_string(&value).expect("toml");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(toml.contains("[[items]]"), "{toml}");
    let reparsed: toml::Value = toml::from_str(&toml).expect("valid toml");
    assert_eq!(reparsed["items"].as_array().expect("array").len(), 2);
}

#[test]
fn quantity_values_emit_normalized_magnitude_strings() {
    // Quantities rarely survive evaluation (bytes normalize to Int),
    // but the mapping is total: magnitude plus normalized suffix.
    let qty =
        active_core::Value::Quantity(5.0, active_core::Unit::from_suffix("MiB").expect("unit"));
    let table = active_core::Value::Table([("size".to_string(), qty)].into_iter().collect());
    let (toml, diags) = value_to_toml_string(&table).expect("toml");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(toml.contains("\"5MiB\""), "{toml}");
}

// ------------------------------------------------------------------
// Reverse: JSON.
// ------------------------------------------------------------------

#[test]
fn json_import_emits_parsing_active() {
    let (source, diags) = json_text_to_active(
        r#"{"server": {"host": "h", "port": 8080}, "flags": [true, false], "ratio": 0.5, "nothing": null}"#,
    )
    .expect("import");
    assert!(diags.is_empty(), "{diags:?}");
    let value = eval(&source);
    let json = value_to_pretty_json(&value);
    assert!(json.contains("\"host\": \"h\""), "{json}\n{source}");
    assert!(json.contains("\"port\": 8080"), "{json}\n{source}");
    assert!(json.contains("true"), "{json}\n{source}");
}

#[test]
fn json_top_level_scalars_wrap_with_w009() {
    for (input, kind) in [("[1, 2]", "array"), ("42", "number"), ("\"s\"", "string")] {
        let (source, diags) = json_text_to_active(input).expect("import");
        assert!(source.starts_with("value = "), "{source:?}");
        assert!(codes(&diags).contains(&"W009"), "{diags:?} for {kind}");
        // The wrapped document parses.
        parse(&source, "convert").expect("must parse");
    }
}

#[test]
fn json_bad_keys_sanitize_with_w007() {
    let (source, diags) = json_text_to_active(r#"{"a-b": 1, "123": 2}"#).expect("import");
    assert!(codes(&diags).contains(&"W007"), "{diags:?}");
    assert!(source.contains("a_b = 1"), "{source:?}");
    assert!(source.contains("_123 = 2"), "{source:?}");
    parse(&source, "convert").expect("must parse");
}

#[test]
fn toml_mixed_arrays_are_hard_errors_with_paths() {
    let value = eval("items = [1, \"x\"]\n");
    let err = value_to_toml_string(&value).expect_err("mixed arrays must fail");
    assert!(err.contains("items"), "{err}");
    // Nulls omit rather than mixing: homogeneous after omission.
    let value = eval("items = [1, null]\nport = 1\n");
    let (toml, diags) = value_to_toml_string(&value).expect("toml");
    assert!(diags.iter().any(|d| d.code == "W006"), "{diags:?}");
    assert!(toml.contains("items = [1]"), "{toml}");
}

#[test]
fn json_huge_integers_note_w010() {
    let big = serde_json::json!({"n": 18446744073709551615u64});
    let (source, diags) = json_value_to_active(&big);
    assert!(codes(&diags).contains(&"W010"), "{diags:?}");
    // Display form with `.0`: parses as Float, never scientific.
    assert!(source.contains("n = 18446744073709552000.0"), "{source:?}");
    parse(&source, "convert").expect("must parse");
}

#[test]
fn json_syntax_errors_are_hard() {
    assert!(json_text_to_active("{bad").is_err());
}

// ------------------------------------------------------------------
// Reverse: YAML and TOML.
// ------------------------------------------------------------------

#[test]
fn yaml_import_handles_mappings_sequences_and_scalars() {
    let (source, diags) =
        yaml_text_to_active("server:\n  host: h\n  port: 8080\nflags: [true]\n").expect("import");
    assert!(diags.is_empty(), "{diags:?}");
    let value = eval(&source);
    let json = value_to_pretty_json(&value);
    assert!(json.contains("\"host\": \"h\""), "{json}\n{source}");
}

#[test]
fn yaml_non_string_keys_stringify_with_w007() {
    let (source, diags) = yaml_text_to_active("123: a\ntrue: b\n").expect("import");
    assert!(codes(&diags).contains(&"W007"), "{diags:?}");
    let value = eval(&source);
    let json = value_to_pretty_json(&value);
    assert!(json.contains("\"_123\": \"a\""), "{json}\n{source}");
    // `true` is a reserved spelling: stringified, then suffixed.
    assert!(json.contains("\"true_\": \"b\""), "{json}\n{source}");
}

#[test]
fn yaml_syntax_errors_are_hard() {
    assert!(yaml_text_to_active("[unclosed\n").is_err());
}

#[test]
fn toml_import_maps_tables_arrays_and_scalars() {
    let (source, diags) =
        toml_text_to_active("[server]\nhost = \"h\"\nport = 8080\n").expect("import");
    assert!(diags.is_empty(), "{diags:?}");
    let value = eval(&source);
    let json = value_to_pretty_json(&value);
    assert!(json.contains("\"host\": \"h\""), "{json}\n{source}");
}

#[test]
fn toml_datetimes_stringify_with_w008() {
    let (source, diags) = toml_text_to_active("when = 2026-10-16T10:00:00Z\n").expect("import");
    assert!(codes(&diags).contains(&"W008"), "{diags:?}");
    assert!(source.contains("when = \"2026-10-16"), "{source:?}");
    parse(&source, "convert").expect("must parse");
}

#[test]
fn toml_syntax_errors_are_hard() {
    assert!(toml_text_to_active("[unclosed\n").is_err());
}

// ------------------------------------------------------------------
// Round trips and one-way markers.
// ------------------------------------------------------------------

#[test]
fn json_round_trip_is_fixed_point() {
    let input = r#"{"a": 1, "b": "x", "c": true, "d": null, "e": [1, "s"], "f": {"g": 2.5}, "h": {}, "i": []}"#;
    let (source, _) = json_text_to_active(input).expect("import");
    let value = eval(&source);
    let out: serde_json::Value = serde_json::from_str(&value_to_pretty_json(&value)).expect("json");
    let expected: serde_json::Value = serde_json::from_str(input).expect("fixture");
    assert_eq!(out, expected);
}

#[test]
fn secret_markers_import_as_plain_strings() {
    // A redacted export must never resurrect into a live secret.
    let (source, diags) = json_text_to_active(r#"{"token": "***SECRET***"}"#).expect("import");
    assert!(diags.is_empty(), "{diags:?}");
    let value = eval(&source);
    let json = value_to_pretty_json(&value);
    assert!(json.contains("***SECRET***"), "{json}");
    match &value {
        active_core::Value::Table(map) => match map.get("token") {
            Some(active_core::Value::String(_)) => {}
            other => panic!("must stay a plain string: {other:?}"),
        },
        other => panic!("table expected: {other:?}"),
    }
}

// ------------------------------------------------------------------
// Review hardening: secrets in errors, special keys, non-finite
// floats, inline notes.
// ------------------------------------------------------------------

#[test]
fn malformed_inputs_never_echo_secrets() {
    // Parser errors echo the offending line: the secret must not
    // survive into the hard error string on any format.
    for (name, result) in [
        (
            "toml",
            toml_text_to_active("password = \"s3cr3t-import-err\"\n[unclosed\n"),
        ),
        (
            "yaml",
            yaml_text_to_active("password: \"s3cr3t-import-err\"\n: bad: [\n"),
        ),
        (
            "json",
            json_text_to_active("{\"password\": \"s3cr3t-import-err\""),
        ),
    ] {
        let err = result.expect_err("{name} must fail hard");
        assert!(!err.contains("s3cr3t-import-err"), "{name}: {err}");
    }
}

#[test]
fn parser_special_keys_rename_only_where_meaning_changes() {
    // A `schema` object would fail as E020; a `depends_on` array
    // would vanish as metadata. Both rename with notes.
    let (source, diags) =
        json_text_to_active(r#"{"schema": {"port": 1}, "depends_on": ["a"]}"#).expect("import");
    assert!(codes(&diags).contains(&"W007"), "{diags:?}");
    assert!(source.contains("schema_ {"), "{source:?}");
    assert!(source.contains("depends_on_ = "), "{source:?}");
    let value = eval(&source);
    let json = value_to_pretty_json(&value);
    assert!(json.contains("\"schema_\""), "{json}\n{source}");
    assert!(json.contains("\"depends_on_\""), "{json}\n{source}");
    // Every other shape under contextual names stays untouched:
    // scalars, blocks, and non-array depends_on parse as data.
    let (source, diags) =
        json_text_to_active(r#"{"include": 1, "profile": {"a": 1}, "depends_on": 1, "as": 2}"#)
            .expect("import");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(source.contains("include = 1"), "{source:?}");
    assert!(source.contains("profile {"), "{source:?}");
    assert!(source.contains("depends_on = 1"), "{source:?}");
    assert!(source.contains("as = 2"), "{source:?}");
    eval(&source);
}

#[test]
fn non_finite_floats_convert_to_null_with_w010() {
    let (source, diags) = yaml_text_to_active("x: .nan\ny: .inf\n").expect("import");
    assert!(codes(&diags).contains(&"W010"), "{diags:?}");
    assert!(source.contains("x = null"), "{source:?}");
    let (source, diags) = toml_text_to_active("x = nan\n").expect("import");
    assert!(codes(&diags).contains(&"W010"), "{diags:?}");
    assert!(source.contains("x = null"), "{source:?}");
    parse(&source, "convert").expect("must parse");
}

#[test]
fn inline_objects_report_key_notes() {
    let (source, diags) = json_text_to_active(r#"{"items": [{"a-b": 1}]}"#).expect("import");
    assert!(codes(&diags).contains(&"W007"), "{diags:?}");
    assert!(source.contains("a_b = 1"), "{source:?}");
    parse(&source, "convert").expect("must parse");
}
