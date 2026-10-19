//! Domain SI tests I: pressure, electrical, energy, force (Phase C1).
//! Spec: grammar.md §1.6, specification.md Ch. 4 §4.1/§4.2 (SI canonicals).
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
fn pressure_canonical_pa() {
    assert_eq!(
        eval_json("p = 101325Pa\n")["p"],
        serde_json::json!({"Pa": 101325.0})
    );
    assert_eq!(
        eval_json("p = 1bar\n")["p"],
        serde_json::json!({"Pa": 100000.0})
    );
    assert_eq!(
        eval_json("p = 14.7psi\n")["p"]["Pa"],
        serde_json::json!(101352.9322095696)
    );
    assert!(codes_of("p = 1bar\n").is_empty());
}

#[test]
fn pressure_cross_spellings_agree() {
    assert_eq!(eval_json("p = 1bar == 100000Pa\n")["p"], true);
    assert_eq!(
        eval_json("p = 2bar + 50000Pa\n")["p"],
        serde_json::json!({"Pa": 250000.0})
    );
    assert_eq!(eval_json("p = 1bar > 50000Pa\n")["p"], true);
}

#[test]
fn electrical_dimensions_distinct() {
    assert_eq!(eval_json("v = 12V\n")["v"], serde_json::json!({"V": 12.0}));
    assert_eq!(eval_json("i = 2A\n")["i"], serde_json::json!({"A": 2.0}));
    assert_eq!(eval_json("w = 60W\n")["w"], serde_json::json!({"W": 60.0}));
    // Volts + amps is the mistake that burns hardware: TypeError, always.
    for src in [
        "x = 12V + 2A\n",
        "x = 60W + 12V\n",
        "x = 2A + 60W\n",
        "x = 12V + 5\n",
    ] {
        let codes = codes_of(src);
        assert!(codes.iter().any(|c| c == "T001"), "{src} -> {codes:?}");
    }
}

#[test]
fn electrical_same_dimension_arithmetic() {
    assert_eq!(
        eval_json("v = 12V + 12V\n")["v"],
        serde_json::json!({"V": 24.0})
    );
    assert_eq!(
        eval_json("w = 100W - 40W\n")["w"],
        serde_json::json!({"W": 60.0})
    );
    assert_eq!(
        eval_json("i = 2A * 3\n")["i"],
        serde_json::json!({"A": 6.0})
    );
    assert_eq!(eval_json("v = 9V < 12V\n")["v"], true);
}

#[test]
fn energy_canonical_joules() {
    assert_eq!(
        eval_json("e = 100J\n")["e"],
        serde_json::json!({"J": 100.0})
    );
    assert_eq!(
        eval_json("e = 1kWh\n")["e"],
        serde_json::json!({"J": 3600000.0})
    );
    assert_eq!(eval_json("e = 1kWh == 3600000J\n")["e"], true);
    assert_eq!(
        eval_json("e = 2kWh + 500000J\n")["e"],
        serde_json::json!({"J": 7700000.0})
    );
}

#[test]
fn force_canonical_newtons() {
    assert_eq!(eval_json("f = 9.8N\n")["f"], serde_json::json!({"N": 9.8}));
    assert_eq!(
        eval_json("f = 9.8N + 0.2N\n")["f"],
        serde_json::json!({"N": 10.0})
    );
    // Force + energy refuses: different dimensions, no matter the intuition.
    let codes = codes_of("x = 9.8N + 100J\n");
    assert!(codes.iter().any(|c| c == "T001"), "{codes:?}");
}

#[test]
fn si_export_respells_canonical() {
    let module = parse("p = 1bar\n", "test").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml");
    assert!(toml.contains("100000Pa"), "{toml}");
    let v = eval_json("e = \"batt ${x}\"\nlet x = 1kWh\n");
    assert_eq!(v["e"], "batt 3600000J", "{v}");
}

#[test]
fn si_round_trip_through_export() {
    for src in [
        "x = 1bar\n",
        "x = 12V\n",
        "x = 1kWh\n",
        "x = 9.8N\n",
        "x = 14.7psi\n",
    ] {
        let first = eval_json(src);
        let module = parse(src, "test").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = active_core::convert::value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(first, second, "{src} via {exported:?}");
    }
}
