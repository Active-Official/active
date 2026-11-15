//! Coercion and rename guards (LANG-B-010, LANG-B-011).
//!
//! Resilient healing must never move a plausible distinct key, clamp a
//! negative duration to zero, or coerce a non-finite float.

use active_core::{parse, Engine};

fn resilient(src: &str) -> (serde_json::Value, Vec<String>) {
    let module = parse(src, "guards").expect("parse");
    let mut engine = Engine::new();
    engine.resilient = true;
    let (value, diags) = engine.evaluate(&module).expect("evaluate");
    let json =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    (json, diags.iter().map(|d| d.code.clone()).collect())
}

#[test]
fn plausible_distinct_keys_are_not_renamed() {
    // `pert`/`pory`/`gost` sit in the suggestion band but are nowhere near
    // the 0.95 edit bar, so their values must stay put.
    for (typo, field) in [("pert", "port"), ("pory", "port"), ("gost", "host")] {
        let ty = if field == "port" { "Int" } else { "String" };
        let src = format!("schema = {{ closed = true, {field} = {ty} }}\n{typo} = 1\n");
        if field == "host" {
            let src = format!("schema = {{ closed = true, host = String }}\n{typo} = \"x\"\n");
            let (v, _c) = resilient(&src);
            assert!(v.get(typo).is_some(), "{typo} must stay: {v}");
            assert!(v.get(field).is_none(), "{typo} must not move: {v}");
            continue;
        }
        let (v, _c) = resilient(&src);
        assert!(v.get(typo).is_some(), "{typo} must stay: {v}");
        assert!(v.get(field).is_none(), "{typo} must not move: {v}");
    }
}

#[test]
fn a_near_identical_key_still_renames() {
    let (v, codes) = resilient(
        "schema = { closed = true, configuration_timeout = Int }\nconfiguration_timeoutt = 5\n",
    );
    assert_eq!(v["configuration_timeout"], 5, "{v}");
    assert!(v.get("configuration_timeoutt").is_none(), "{v}");
    assert!(codes.iter().any(|c| c == "H005"), "{codes:?}");
}

#[test]
fn negative_durations_are_rejected_not_clamped() {
    let (v, codes) = resilient("schema = { t = Duration }\nt = \"-5s\"\n");
    assert_eq!(v["t"], "-5s", "the string stays; no 0ms value: {v}");
    assert!(codes.iter().any(|c| c == "T002"), "{codes:?}");
    assert!(!codes.iter().any(|c| c == "H007"), "{codes:?}");
    // Control: zero and positive durations still coerce.
    let (v, codes) = resilient("schema = { t = Duration }\nt = \"0s\"\n");
    assert_eq!(v["t"]["ms"], 0, "{v}");
    assert!(codes.iter().any(|c| c == "H007"), "{codes:?}");
    let (v, _c) = resilient("schema = { t = Duration }\nt = \"1.5s\"\n");
    assert_eq!(v["t"]["ms"], 1500, "{v}");
}

#[test]
fn float_coercion_accepts_only_plain_decimal_spellings() {
    // LANG-B-024: the grammar has no exponent notation, so `"1e3"` must not
    // coerce; the documented plain spellings still do.
    for (src, want) in [("2.5", 2.5f64), ("-0.25", -0.25), ("7", 7.0), ("+3.5", 3.5)] {
        let doc = format!("schema = {{ x = Float }}\nx = \"{src}\"\n");
        let (v, codes) = resilient(&doc);
        assert_eq!(v["x"], serde_json::json!(want), "{src}: {v}");
        assert!(codes.iter().any(|c| c == "H007"), "{src}: {codes:?}");
    }
    for bad in ["1e3", "1E3", "nan", "inf", "0x10", "", "1.2.3"] {
        let doc = format!("schema = {{ x = Float }}\nx = \"{bad}\"\n");
        let (v, codes) = resilient(&doc);
        assert_eq!(v["x"], bad, "{bad} must stay a string: {v}");
        assert!(codes.iter().any(|c| c == "T002"), "{bad}: {codes:?}");
        assert!(!codes.iter().any(|c| c == "H007"), "{bad}: {codes:?}");
    }
}
