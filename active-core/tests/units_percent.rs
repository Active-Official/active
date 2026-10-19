//! Percent dimension tests (goal: universal measurement, Phase A-1).
//! Spec: grammar.md §1.6 (`%` row), specification.md Ch. 4 §4.1/§4.2
//! (percent canonical ratio, arithmetic matrix, percent-form export).
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
fn pct_basic_canonical_ratio() {
    let v = eval_json("p = 25%\n");
    assert_eq!(v["p"], serde_json::json!({"%": 0.25}), "{v}");
    assert!(codes_of("p = 25%\n").is_empty());
}

#[test]
fn pct_spelled_forms_agree() {
    // `25%`, `25.0%`... all spell the same canonical ratio.
    assert_eq!(eval_json("p = 25%\n")["p"], eval_json("p = 25.0%\n")["p"]);
    assert_eq!(
        eval_json("p = 12.5%\n")["p"],
        serde_json::json!({"%": 0.125})
    );
}

#[test]
fn pct_export_is_percent_form() {
    // TOML respell + interpolation render `25%`, never `0.25%`.
    let module = parse("p = 25%\n", "test").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml");
    assert!(toml.contains("25%"), "{toml}");
    assert!(!toml.contains("0.25%"), "{toml}");
    let v = eval_json("p = \"load ${x}\"\nlet x = 25%\n");
    assert_eq!(v["p"], "load 25%", "{v}");
}

#[test]
fn pct_same_dimension_arithmetic() {
    assert_eq!(
        eval_json("p = 25% + 25%\n")["p"],
        serde_json::json!({"%": 0.5})
    );
    assert_eq!(
        eval_json("p = 50% - 25%\n")["p"],
        serde_json::json!({"%": 0.25})
    );
    assert_eq!(
        eval_json("p = 25% * 2\n")["p"],
        serde_json::json!({"%": 0.5})
    );
    assert_eq!(
        eval_json("p = 50% / 2\n")["p"],
        serde_json::json!({"%": 0.25})
    );
    assert_eq!(eval_json("p = 50% / 25%\n")["p"], 2.0);
    assert_eq!(eval_json("p = 25% < 50%\n")["p"], true);
    assert_eq!(eval_json("p = 50% >= 50%\n")["p"], true);
}

#[test]
fn pct_cross_dimension_is_type_error() {
    for src in [
        "x = 25% + 5s\n",
        "x = 25% + 1\n",
        "x = 25% < 5s\n",
        "x = 25% / 0\n",
        "x = 25% + 1m\n",
    ] {
        let codes = codes_of(src);
        assert!(codes.iter().any(|c| c == "T001"), "{src} -> {codes:?}");
    }
}

#[test]
fn pct_equality_never_coerces() {
    assert_eq!(eval_json("p = 25% == 25%\n")["p"], true);
    // The ratio is not the number: `25% == 0.25` is FALSE, never an error.
    assert_eq!(eval_json("p = 25% == 0.25\n")["p"], false);
    assert_eq!(eval_json("p = 25% == 25\n")["p"], false);
}

#[test]
fn pct_no_clamp() {
    // Over-100 and negative percents are meaningful (growth, discounts).
    assert_eq!(eval_json("p = 150%\n")["p"], serde_json::json!({"%": 1.5}));
    assert_eq!(eval_json("p = -5%\n")["p"], serde_json::json!({"%": -0.05}));
}

#[test]
fn pct_spaced_suffix_refused() {
    // A space means two tokens: `25 %` is E005, never a quantity.
    let diags = parse("p = 25 %\n", "test").expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    let span = e.span.clone().expect("span");
    assert_eq!((span.line, span.column), (1, 8), "{span:?}");
    // Positive control: the glued twin evaluates silently.
    assert!(codes_of("p = 25%\n").is_empty());
}

#[test]
fn rates_same_suffix_arithmetic() {
    // Same mechanism as percent: same-suffix rates compute.
    let v = eval_json("r = 100/req/s + 50/req/s\n");
    assert_eq!(v["r"], serde_json::json!({"req/s": 150.0}), "{v}");
    assert_eq!(
        eval_json("r = 100/req/s - 25/req/s\n")["r"],
        serde_json::json!({"req/s": 75.0})
    );
    assert_eq!(eval_json("r = 50/req/s < 100/req/s\n")["r"], true);
    // Cross-suffix stays a dimensional TypeError (class T001).
    let codes = codes_of("r = 100/req/s + 1/req/m\n");
    assert!(codes.iter().any(|c| c == "T001"), "{codes:?}");
}

#[test]
fn pct_schema_kind_is_quantity_percent() {
    // Closed-table schema diagnostic paths use the quantity kind with the
    // canonical suffix; a percent value validates its own kind silently.
    let src = "p = 25%\n";
    assert!(codes_of(src).is_empty());
    let v = eval_json(src);
    assert_eq!(v["p"], serde_json::json!({"%": 0.25}), "{v}");
}
