//! Temperature + decibel tests: ordering-only specials (Phase C4).
//! Spec: grammar.md §1.6, specification.md Ch. 4 §4.1/§4.2 (affine/log,
//! arithmetic refused).
//! Every test drives shipped entry points only: `parse` → `Engine` → JSON.

use active_core::convert::value_to_pretty_json;
use active_core::{parse, Engine};

fn eval_json(src: &str) -> serde_json::Value {
    let module = parse(src, "test").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    serde_json::from_str(&value_to_pretty_json(&value)).expect("valid json")
}

fn codes_of(src: &str) -> Vec<String> {
    let module = match parse(src, "test") {
        Ok(m) => m,
        Err(diags) => return diags.iter().map(|d| d.code.clone()).collect(),
    };
    match Engine::new().evaluate(&module) {
        Ok((_, diags)) => diags.iter().map(|d| d.code.clone()).collect(),
        Err(e) => vec![e.code().to_string()],
    }
}

#[test]
fn temp_canonical_kelvin() {
    assert_eq!(
        eval_json("t = 20Cel\n")["t"],
        serde_json::json!({"K": 293.15})
    );
    assert_eq!(
        eval_json("t = 0Cel\n")["t"],
        serde_json::json!({"K": 273.15})
    );
    assert_eq!(
        eval_json("t = 273.15K\n")["t"],
        serde_json::json!({"K": 273.15})
    );
    assert_eq!(
        eval_json("t = 68Fahr\n")["t"],
        serde_json::json!({"K": 293.15})
    );
    assert!(codes_of("t = 20Cel\n").is_empty());
}

#[test]
fn temp_cross_spellings_agree() {
    // 68Fahr == 20Cel: affine commensurability through kelvin.
    assert_eq!(eval_json("t = 68Fahr == 20Cel\n")["t"], true);
    assert_eq!(eval_json("t = 32Fahr == 0Cel\n")["t"], true);
    assert_eq!(eval_json("t = 212Fahr == 100Cel\n")["t"], true);
    // The famous crossover: unary minus folds into the literal BEFORE the
    // affine transform, so -40 really equals -40.
    assert_eq!(eval_json("t = -40Fahr == -40Cel\n")["t"], true);
    assert_eq!(eval_json("t = 100Cel > 200Fahr\n")["t"], true);
}

#[test]
fn temp_explicit_negation_preserved() {
    // `-300Cel` folds the minus into the literal BEFORE the affine
    // transform, then clamps at 0 K: below-absolute-zero magnitudes never
    // produce negative kelvin. Linear dims keep negate-preserved values.
    assert_eq!(
        eval_json("t = -300Cel\n")["t"],
        serde_json::json!({"K": 0.0})
    );
    assert!(codes_of("t = -300Cel\n").is_empty());
}

#[test]
fn temp_arithmetic_refused() {
    // Affine values do not add: 20Cel + 20Cel is not 40Cel.
    for src in [
        "x = 20Cel + 20Cel\n",
        "x = 20Cel - 10Cel\n",
        "x = 20Cel * 2\n",
        "x = 20Cel / 2\n",
        "x = 300K + 10K\n",
        "x = 20Cel / 10Cel\n",
    ] {
        let codes = codes_of(src);
        assert!(codes.iter().any(|c| c == "T001"), "{src} -> {codes:?}");
    }
}

#[test]
fn db_ordering_only() {
    assert_eq!(eval_json("l = 3dB\n")["l"], serde_json::json!({"dB": 3.0}));
    assert_eq!(eval_json("l = 3dB == 3dB\n")["l"], true);
    assert_eq!(eval_json("l = 6dB > 3dB\n")["l"], true);
    // Log values do not add: 3dB + 3dB is not 6dB.
    for src in ["x = 3dB + 3dB\n", "x = 6dB - 3dB\n", "x = 3dB * 2\n"] {
        let codes = codes_of(src);
        assert!(codes.iter().any(|c| c == "T001"), "{src} -> {codes:?}");
    }
}

#[test]
fn temp_db_cross_dimension_refused() {
    for src in ["x = 20Cel + 3dB\n", "x = 20Cel + 5s\n", "x = 3dB + 100\n"] {
        let codes = codes_of(src);
        assert!(codes.iter().any(|c| c == "T001"), "{src} -> {codes:?}");
    }
}

#[test]
fn temp_export_respells_kelvin() {
    let module = parse("t = 20Cel\n", "test").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml");
    assert!(toml.contains("293.15K"), "{toml}");
    let v = eval_json("t = \"room ${x}\"\nlet x = 20Cel\n");
    assert_eq!(v["t"], "room 293.15K", "{v}");
}

#[test]
fn temp_round_trip_through_export() {
    for src in ["x = 20Cel\n", "x = 68Fahr\n", "x = 300K\n", "x = 3dB\n"] {
        let first = eval_json(src);
        let module = parse(src, "test").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = active_core::convert::value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(first, second, "{src} via {exported:?}");
    }
}
