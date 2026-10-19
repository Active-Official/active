//! Bits and data-rate dimension tests (goal: universal measurement, Phase A-3).
//! Spec: grammar.md §1.6 (bits/rates rows), specification.md Ch. 4 §4.1/§4.2
//! (canonical bits / bits-sec, MB/s conversion, case discipline).
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
fn bits_canonical_bits() {
    assert_eq!(eval_json("b = 8b\n")["b"], serde_json::json!({"b": 8.0}));
    assert_eq!(
        eval_json("b = 1Kb\n")["b"],
        serde_json::json!({"b": 1000.0})
    );
    assert_eq!(
        eval_json("b = 2Mb\n")["b"],
        serde_json::json!({"b": 2000000.0})
    );
    assert_eq!(
        eval_json("b = 1Gb\n")["b"],
        serde_json::json!({"b": 1000000000.0})
    );
    assert!(codes_of("b = 1Kb\n").is_empty());
}

#[test]
fn rates_canonical_bps() {
    assert_eq!(
        eval_json("r = 1bps\n")["r"],
        serde_json::json!({"bps": 1.0})
    );
    assert_eq!(
        eval_json("r = 1Kbps\n")["r"],
        serde_json::json!({"bps": 1000.0})
    );
    assert_eq!(
        eval_json("r = 100Mbps\n")["r"],
        serde_json::json!({"bps": 100000000.0})
    );
    assert_eq!(
        eval_json("r = 1Gbps\n")["r"],
        serde_json::json!({"bps": 1000000000.0})
    );
    // MB/s converts through megabytes: 1 MB/s = 8,000,000 bps.
    assert_eq!(
        eval_json("r = 1MB/s\n")["r"],
        serde_json::json!({"bps": 8000000.0})
    );
    assert!(codes_of("r = 1MB/s\n").is_empty());
}

#[test]
fn cross_spelled_rates_compare_equal() {
    // 1MB/s == 8Mbps: the conversion network engineers do constantly.
    assert_eq!(eval_json("r = 1MB/s == 8Mbps\n")["r"], true);
    assert_eq!(eval_json("r = 1Gbps == 1000Mbps\n")["r"], true);
    assert_eq!(eval_json("r = 1Kbps == 1000bps\n")["r"], true);
}

#[test]
fn bits_rates_arithmetic() {
    assert_eq!(
        eval_json("b = 1Kb + 500b\n")["b"],
        serde_json::json!({"b": 1500.0})
    );
    assert_eq!(
        eval_json("r = 100Mbps + 50Mbps\n")["r"],
        serde_json::json!({"bps": 150000000.0})
    );
    assert_eq!(
        eval_json("r = 1Gbps - 250Mbps\n")["r"],
        serde_json::json!({"bps": 750000000.0})
    );
    assert_eq!(
        eval_json("r = 100Mbps * 2\n")["r"],
        serde_json::json!({"bps": 200000000.0})
    );
    assert_eq!(eval_json("r = 1Gbps / 1Mbps\n")["r"], 1000.0);
    assert_eq!(eval_json("r = 500Mbps < 1Gbps\n")["r"], true);
}

#[test]
fn bits_bytes_dimensions_refuse() {
    // Networks are sold in bits, storage in bytes: never confused.
    for src in [
        "x = 8b + 1B\n",
        "x = 1Mbps + 1MB\n",
        "x = 8b + 100\n",
        "x = 1Gbps + 1s\n",
    ] {
        let codes = codes_of(src);
        assert!(codes.iter().any(|c| c == "T001"), "{src} -> {codes:?}");
    }
}

#[test]
fn case_discipline_bits_vs_bytes() {
    // `Kb` (kilobits) and `KB` (kilobytes) coexist without confusion.
    assert_eq!(
        eval_json("b = 1Kb\n")["b"],
        serde_json::json!({"b": 1000.0})
    );
    assert_eq!(eval_json("b = 1KB\n")["b"], 1000);
    assert_eq!(
        eval_json("b = 1Mb\n")["b"],
        serde_json::json!({"b": 1000000.0})
    );
    assert_eq!(eval_json("b = 1MB\n")["b"], 1000000);
    // ...and never equal each other: 1KB (8000 bits) != 1Kb (1000 bits).
    assert_eq!(eval_json("b = 1KB == 1Kb\n")["b"], false);
}

#[test]
fn bits_equality_never_coerces() {
    assert_eq!(eval_json("b = 8b == 8b\n")["b"], true);
    assert_eq!(eval_json("b = 8b == 8\n")["b"], false);
    assert_eq!(eval_json("r = 1Kbps == 1000\n")["r"], false);
}

#[test]
fn bits_export_respells_canonical() {
    let module = parse("r = 1Kbps\n", "test").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml");
    assert!(toml.contains("1000bps"), "{toml}");
    let v = eval_json("r = \"link ${x}\"\nlet x = 1MB/s\n");
    assert_eq!(v["r"], "link 8000000bps", "{v}");
}

#[test]
fn bits_round_trip_through_export() {
    for src in ["x = 8b\n", "x = 1Kbps\n", "x = 1MB/s\n", "x = 100Mbps\n"] {
        let first = eval_json(src);
        let module = parse(src, "test").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = active_core::convert::value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(first, second, "{src} via {exported:?}");
    }
}
