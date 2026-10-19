//! Mechanical SI tests: angle, rotation, length, mass (Phase C2).
//! Spec: grammar.md §1.6, specification.md Ch. 4 §4.1/§4.2 (rad/meter/kg
//! canonicals, rpm retained, long-form-only meters).
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
fn angle_canonical_radians() {
    assert_eq!(
        eval_json("a = 180deg\n")["a"],
        serde_json::json!({"rad": std::f64::consts::PI})
    );
    assert_eq!(
        eval_json("a = 1rad\n")["a"],
        serde_json::json!({"rad": 1.0})
    );
    assert_eq!(eval_json("a = 180deg == 3.141592653589793rad\n")["a"], true);
    assert!(codes_of("a = 180deg\n").is_empty());
}

#[test]
fn angle_arithmetic() {
    assert_eq!(
        eval_json("a = 90deg + 90deg\n")["a"]["rad"],
        serde_json::json!(3.141592653589793)
    );
    assert_eq!(
        eval_json("a = 180deg - 90deg\n")["a"]["rad"],
        serde_json::json!(1.5707963267948966)
    );
    assert_eq!(eval_json("a = 90deg < 180deg\n")["a"], true);
}

#[test]
fn rotation_retained_rpm() {
    assert_eq!(
        eval_json("r = 60rpm\n")["r"],
        serde_json::json!({"rpm": 60.0})
    );
    assert_eq!(
        eval_json("r = 60rpm + 60rpm\n")["r"],
        serde_json::json!({"rpm": 120.0})
    );
    // Rotation is a rate dimension: rpm + deg refuses.
    let codes = codes_of("x = 60rpm + 1deg\n");
    assert!(codes.iter().any(|c| c == "T001"), "{codes:?}");
}

#[test]
fn length_longform_meters() {
    assert_eq!(
        eval_json("l = 1km\n")["l"],
        serde_json::json!({"meter": 1000.0})
    );
    assert_eq!(
        eval_json("l = 500meter\n")["l"],
        serde_json::json!({"meter": 500.0})
    );
    assert_eq!(eval_json("l = 1km == 1000meter\n")["l"], true);
    assert_eq!(
        eval_json("l = 1km + 500meter\n")["l"],
        serde_json::json!({"meter": 1500.0})
    );
    // `m` stays minutes: 1m is a duration, never a length.
    assert_eq!(eval_json("t = 1m\n")["t"], serde_json::json!({"ms": 60000}));
}

#[test]
fn mass_canonical_kg() {
    assert_eq!(eval_json("m = 1kg\n")["m"], serde_json::json!({"kg": 1.0}));
    assert_eq!(eval_json("m = 500g\n")["m"], serde_json::json!({"kg": 0.5}));
    assert_eq!(eval_json("m = 1kg == 1000g\n")["m"], true);
    assert_eq!(
        eval_json("m = 2kg - 500g\n")["m"],
        serde_json::json!({"kg": 1.5})
    );
}

#[test]
fn mech_cross_dimension_refused() {
    for src in [
        "x = 1km + 1kg\n",
        "x = 90deg + 1meter\n",
        "x = 60rpm + 60\n",
        "x = 1kg + 1s\n",
    ] {
        let codes = codes_of(src);
        assert!(codes.iter().any(|c| c == "T001"), "{src} -> {codes:?}");
    }
}

#[test]
fn mech_export_respells_canonical() {
    let module = parse("l = 1km\n", "test").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml");
    assert!(toml.contains("1000meter"), "{toml}");
    let v = eval_json("a = \"turn ${x}\"\nlet x = 90deg\n");
    assert!(v["a"].as_str().expect("string").contains("rad"), "{v}");
}

#[test]
fn mech_round_trip_through_export() {
    for src in ["x = 180deg\n", "x = 60rpm\n", "x = 1km\n", "x = 500g\n"] {
        let first = eval_json(src);
        let module = parse(src, "test").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = active_core::convert::value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(first, second, "{src} via {exported:?}");
    }
}
