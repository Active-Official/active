//! Wire round-trip per value kind (LANG-F-001/F-002).
//!
//! The contract (spec/converters.md §4): the JSON export is injective and the
//! importer recognises the exporter's own shapes, so ACTIVE → JSON → ACTIVE
//! → evaluate preserves the value's type and data for every kind the language
//! can write. Secrets are the documented exception (they export as a marker
//! and never round-trip to the secret); blobs travel as identity only.

use active_core::convert::{json_text_to_active, value_to_pretty_json};
use active_core::{parse, Engine, Value};
use serde_json::Value as Json;

fn evaluate(src: &str) -> Value {
    let module = parse(src, "wire").unwrap_or_else(|diags| panic!("parse failed: {diags:?}"));
    let (value, diags) = Engine::hermetic()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("evaluation failed: {e}"));
    assert!(diags.is_empty(), "{diags:?}");
    value
}

/// Export, re-import, re-evaluate, export again: the two JSON trees must be
/// identical, which is the fixed point spec/converters.md §4 promises.
fn round_trip(src: &str) -> (Json, Json) {
    let first = evaluate(src);
    let json = value_to_pretty_json(&first);
    let (imported, diags) = json_text_to_active(&json).unwrap_or_else(|e| {
        panic!("import of {json} failed: {e}");
    });
    // Import notes are about *document* shapes (sanitized keys, datetimes),
    // never about a value this exporter produced losing its type.
    assert!(
        !diags.iter().any(|d| d.code == "W016"),
        "the exporter never produces the ambiguous duration shape: {diags:?}"
    );
    let second = evaluate(&imported);
    let again = value_to_pretty_json(&second);
    let parsed_again: Json = serde_json::from_str(&again).expect("valid JSON");
    let parsed_first: Json = serde_json::from_str(&json).expect("valid JSON");
    (parsed_first, parsed_again)
}

#[test]
fn every_value_kind_survives_the_wire_round_trip() {
    for (label, src) in [
        ("null", "v = null\n"),
        ("bool", "v = true\n"),
        ("int", "v = 42\n"),
        ("negative int", "v = -7\n"),
        ("float", "v = 2.5\n"),
        ("string", "v = \"hello\"\n"),
        ("empty string", "v = \"\"\n"),
        ("duration", "v = 5s\n"),
        ("sub-millisecond duration", "v = 1500us\n"),
        ("zero duration", "v = 0s\n"),
        ("percent quantity", "v = 25%\n"),
        ("rate quantity", "v = 100req/s\n"),
        ("byte quantity", "v = 2MiB\n"),
        ("array", "v = [1, 2, 3]\n"),
        ("nested table", "v = { a = 1, b = { c = 2 } }\n"),
        (
            "table with reserved-looking keys",
            "v = { ms = 1, unit = \"x\", value = 2 }\n",
        ),
        ("string that looks like a tag", "v = \"$active.duration\"\n"),
        ("string \"inf\"", "v = \"inf\"\n"),
    ] {
        let (first, again) = round_trip(src);
        assert_eq!(first, again, "{label}: round-trip changed the value");
    }
}

#[test]
fn durations_and_plain_tables_no_longer_collide_on_the_wire() {
    // LANG-F-002: the two documents used to export to byte-identical JSON
    // (`{"ms": 5000}`); the wrapper makes them distinguishable, and the
    // importer reads each one back as its own kind.
    let duration = evaluate("v = 5s\n");
    let table = evaluate("v = { ms = 5000 }\n");
    let duration_json = value_to_pretty_json(&duration);
    let table_json = value_to_pretty_json(&table);
    assert_ne!(duration_json, table_json, "the wire forms must differ");
    assert!(
        duration_json.contains("$active.duration"),
        "{duration_json}"
    );
    assert!(!table_json.contains("$active.duration"), "{table_json}");

    // Each re-imports to its own kind: `5s + 1s` works on the duration and
    // fails on the table.
    let (imported, _) = json_text_to_active(&duration_json).expect("import duration");
    assert!(imported.contains("5000ms"), "{imported}");
    let (imported_table, diags) = json_text_to_active(&table_json).expect("import table");
    assert!(
        !imported_table.contains("$active.duration"),
        "{imported_table}"
    );
    assert!(
        diags.iter().any(|d| d.code == "W016"),
        "a genuine table that happens to be `{{ms = N}}` re-imports with the \
         documented ambiguity note: {diags:?}"
    );
    // The wrapped duration imports with no ambiguity note at all.
    let (duration_source, duration_diags) =
        json_text_to_active(&duration_json).expect("import duration");
    assert!(duration_source.contains("5000ms"), "{duration_source}");
    assert!(
        !duration_diags.iter().any(|d| d.code == "W016"),
        "{duration_diags:?}"
    );
}

#[test]
fn the_ambiguous_bare_duration_shape_is_flagged_on_import() {
    // A sender that emits the old bare shape gets a loud W016 and the
    // faithful table reading - never a silent duration guess.
    let (source, diags) = json_text_to_active("{\"v\": {\"ms\": 5000}}").expect("import succeeds");
    assert!(source.contains("ms = 5000"), "{source}");
    assert!(
        diags.iter().any(|d| d.code == "W016"),
        "the ambiguity is announced: {diags:?}"
    );
}

#[test]
fn secrets_and_blobs_keep_their_documented_boundary_behaviour() {
    // A secret exports as the marker and must NOT come back as the secret.
    let secret = evaluate("v = secret(\"hunter2-wire\")\n");
    let json = value_to_pretty_json(&secret);
    assert!(json.contains("***SECRET***"), "{json}");
    assert!(!json.contains("hunter2-wire"), "{json}");

    // A blob exports identity only and re-imports as a table (the wrapper
    // names bytes the receiver may or may not hold); the importer must not
    // mistake the wrapper for a duration or a quantity.
    let blob = evaluate("v = blob(\"text/plain\", \"hello\")\n");
    let json = value_to_pretty_json(&blob);
    let (source, diags) = json_text_to_active(&json).expect("import blob");
    assert!(!diags.iter().any(|d| d.code == "W016"), "{diags:?}");
    assert!(!source.contains("$active.duration"), "{source}");
}

#[test]
fn inspection_reports_the_layer_that_wrote_a_value() {
    // LANG-F-014: the wire form is layer-flattened by design; the
    // inspection surface names the winning layer so an operator (or a
    // receiver auditing a peer document) knows whether a value came from
    // the base document, a profile, or a patch.
    use active_core::convert::inspect_path_with_layers;
    let src = "base = 1\nprofile \"prod\" { base = 2, extra = 3 }\nactive_profile = \"prod\"\n";
    let module = parse(src, "layers").expect("parse");
    let engine = Engine::hermetic();
    let (value, diags, layers) = engine.evaluate_with_layers(&module).expect("evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(layers.get("base").map(String::as_str), Some("profile:prod"));
    assert_eq!(
        layers.get("extra").map(String::as_str),
        Some("profile:prod")
    );
    assert!(
        !layers.contains_key("__never__"),
        "only merged paths are recorded"
    );

    let report = inspect_path_with_layers(&module, &value, "base", None, None, None, &layers)
        .expect("inspect");
    assert_eq!(report["layer"], serde_json::json!("profile:prod"));
    assert_eq!(report["value"], serde_json::json!(2));

    // A base-document value has no layer key.
    let plain = parse("only = 1\n", "layers-plain").expect("parse");
    let (plain_value, _, plain_layers) =
        engine.evaluate_with_layers(&plain).expect("evaluate plain");
    assert!(plain_layers.is_empty(), "{plain_layers:?}");
    let report = inspect_path_with_layers(
        &plain,
        &plain_value,
        "only",
        None,
        None,
        None,
        &plain_layers,
    )
    .expect("inspect plain");
    assert!(
        report.get("layer").is_none(),
        "base values carry no layer: {report}"
    );
}
