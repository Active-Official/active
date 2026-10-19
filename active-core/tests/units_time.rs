//! Week + long-alias time tests (goal: universal measurement, Phase A-4).
//! Spec: grammar.md §1.6 (week/sec/min row), specification.md Ch. 4 §4.1
//! (exact week, aliases, plural refusal).
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
fn week_is_exact_ms() {
    assert_eq!(
        eval_json("t = 1week\n")["t"],
        serde_json::json!({"ms": 604800000})
    );
    assert_eq!(
        eval_json("t = 2week\n")["t"],
        serde_json::json!({"ms": 1209600000})
    );
    assert!(codes_of("t = 1week\n").is_empty());
}

#[test]
fn week_arithmetic_inherited() {
    assert_eq!(
        eval_json("t = 1week + 1d\n")["t"],
        serde_json::json!({"ms": 691200000})
    );
    assert_eq!(eval_json("t = 1week == 7d\n")["t"], true);
    assert_eq!(eval_json("t = 1week > 6d\n")["t"], true);
}

#[test]
fn long_aliases_canonicalize() {
    assert_eq!(
        eval_json("t = 90sec\n")["t"],
        serde_json::json!({"ms": 90000})
    );
    assert_eq!(
        eval_json("t = 5min\n")["t"],
        serde_json::json!({"ms": 300000})
    );
    assert!(codes_of("t = 90sec\n").is_empty());
    assert!(codes_of("t = 5min\n").is_empty());
}

#[test]
fn aliases_respell_canonical() {
    // Canonical respell stays `s`/`m`: aliases are input sugar, not values.
    let module = parse("t = 90sec\n", "test").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml");
    assert!(toml.contains("90000ms"), "{toml}");
    let v = eval_json("t = \"wait ${x}\"\nlet x = 5min\n");
    assert_eq!(v["t"], "wait 300000ms", "{v}");
}

#[test]
fn plurals_refused() {
    // No unit in the language has a plural: guessing is silent coercion.
    for src in ["t = 2weeks\n", "t = 90secs\n", "t = 5mins\n"] {
        let diags = parse(src, "test").expect_err("must fail");
        assert!(diags.iter().any(|d| d.code == "E002"), "{src}");
    }
    // Positive controls: the singular twins evaluate silently.
    assert!(codes_of("t = 2week\n").is_empty());
    assert!(codes_of("t = 90sec\n").is_empty());
    assert!(codes_of("t = 5min\n").is_empty());
}
