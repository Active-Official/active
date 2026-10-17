//! Unit round-trip tests.
//!
//! Guards the invariant that every unit suffix produced by `Unit::suffix()`
//! (used by JSON/TOML export and interpolation) is re-parseable by the
//! lexer and `Unit::from_suffix`, so exported configurations can be fed
//! back into Active unchanged. Drives the real shipped path:
//! parse → evaluate → export → parse → evaluate.

use active_core::convert::{value_to_pretty_json, value_to_simple_toml};
use active_core::{parse, Engine};

fn eval_json(src: &str) -> String {
    let module = parse(src, "roundtrip").expect("must parse");
    let engine = Engine::new();
    let (value, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    value_to_pretty_json(&value)
}

#[test]
fn rate_unit_suffixes_round_trip_through_export() {
    // Covers every rate unit and both slash-prefixed and bare spellings.
    let sources = [
        "x = 100/req/s",
        "x = 100/req/m",
        "x = 100/req/h",
        "x = 100req/s",
        "x = 100req/m",
        "x = 100req/h",
        "x = 100/s",
        "x = 100/m",
        "x = 100/h",
    ];
    for src in sources {
        let first = eval_json(src);
        let module = parse(src, "roundtrip").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(
            first, second,
            "round-trip failed for {src}: exported as {exported:?}"
        );
    }
}

#[test]
fn time_and_byte_units_round_trip_through_export() {
    let sources = [
        "x = 5s",
        "x = 500ms",
        "x = 15m",
        "x = 2h",
        "x = 512MiB",
        "x = 2KiB",
        "x = 1GiB",
        "x = 100MB",
    ];
    for src in sources {
        let first = eval_json(src);
        let module = parse(src, "roundtrip").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(
            first, second,
            "round-trip failed for {src}: exported as {exported:?}"
        );
    }
}
