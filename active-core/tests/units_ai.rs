//! AI-age unit tests (goal: universal measurement, Phase B).
//! Spec: grammar.md §1.6 (tokens/tok/s/FLOPS rows), specification.md Ch. 4
//! §4.1/§4.2 (canonical counts, glued spelling, admission record).
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
fn tokens_canonical_counts() {
    assert_eq!(
        eval_json("c = 128ktokens\n")["c"],
        serde_json::json!({"tokens": 128000.0})
    );
    assert_eq!(
        eval_json("c = 4ktokens\n")["c"],
        serde_json::json!({"tokens": 4000.0})
    );
    assert_eq!(
        eval_json("c = 2Mtokens\n")["c"],
        serde_json::json!({"tokens": 2000000.0})
    );
    assert_eq!(
        eval_json("c = 512tokens\n")["c"],
        serde_json::json!({"tokens": 512.0})
    );
    assert!(codes_of("c = 128ktokens\n").is_empty());
}

#[test]
fn tokens_arithmetic() {
    assert_eq!(
        eval_json("c = 128ktokens + 4ktokens\n")["c"],
        serde_json::json!({"tokens": 132000.0})
    );
    assert_eq!(
        eval_json("c = 2Mtokens - 500ktokens\n")["c"],
        serde_json::json!({"tokens": 1500000.0})
    );
    assert_eq!(
        eval_json("c = 4ktokens * 2\n")["c"],
        serde_json::json!({"tokens": 8000.0})
    );
    assert_eq!(eval_json("c = 128ktokens / 4ktokens\n")["c"], 32.0);
    assert_eq!(eval_json("c = 4ktokens < 128ktokens\n")["c"], true);
    assert_eq!(eval_json("c = 128ktokens == 128000tokens\n")["c"], true);
}

#[test]
fn tokens_glued_spelling_only() {
    // `128k tokens` (spaced) is two tokens: E002, never a quantity.
    let diags = parse("c = 128k tokens\n", "test").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("span");
    assert_eq!((span.line, span.column), (1, 8), "{span:?}");
    // Positive control: the glued twin evaluates silently.
    assert!(codes_of("c = 128ktokens\n").is_empty());
}

#[test]
fn tokens_pair_form_stays_pair() {
    // `tokens:128k` is the pair form, and `128k` is not a value: E002.
    let diags = parse("tokens:128k\n", "test").expect_err("must fail E002");
    assert!(diags.iter().any(|d| d.code == "E002"));
    // Positive control: a real pair assigns silently.
    assert_eq!(eval_json("tokens:128\n")["tokens"], 128);
}

#[test]
fn tokens_leading_digit_form_announced() {
    // `128k:tokens` cannot be a quantity: it parses as `c = 128` plus a
    // pair `k:tokens` whose value is unresolved — announced W001, never
    // silent, and never a token count. Quantities and pairs never mix.
    let src = "c = 128k:tokens\n";
    let v = eval_json(src);
    assert_eq!(v["c"], 128, "{v}");
    assert_eq!(v["k"], serde_json::Value::Null, "{v}");
    let module = parse(src, "test").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    let span = w.span.clone().expect("span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
}

#[test]
fn toks_canonical_throughput() {
    assert_eq!(
        eval_json("t = 500tok/s\n")["t"],
        serde_json::json!({"tok/s": 500.0})
    );
    assert_eq!(
        eval_json("t = 500tok/s + 250tok/s\n")["t"],
        serde_json::json!({"tok/s": 750.0})
    );
    assert_eq!(eval_json("t = 100tok/s < 500tok/s\n")["t"], true);
    // Cross-dimension with plain rates stays refused.
    let codes = codes_of("t = 500tok/s + 100/req/s\n");
    assert!(codes.iter().any(|c| c == "T001"), "{codes:?}");
}

#[test]
fn flops_canonical_compute() {
    assert_eq!(
        eval_json("f = 2TFLOPS\n")["f"],
        serde_json::json!({"FLOPS": 2000000000000.0})
    );
    assert_eq!(
        eval_json("f = 15PFLOPS\n")["f"],
        serde_json::json!({"FLOPS": 15000000000000000.0})
    );
    assert_eq!(
        eval_json("f = 100FLOPS\n")["f"],
        serde_json::json!({"FLOPS": 100.0})
    );
    assert_eq!(
        eval_json("f = 2TFLOPS + 500TFLOPS\n")["f"],
        serde_json::json!({"FLOPS": 502000000000000.0})
    );
    assert_eq!(eval_json("f = 1PFLOPS == 1000TFLOPS\n")["f"], true);
    // Case discipline: `flops`/`Tflops` are not units.
    for src in ["f = 1flops\n", "f = 1Tflops\n"] {
        let diags = parse(src, "test").expect_err("must fail");
        assert!(diags.iter().any(|d| d.code == "E002"), "{src}");
    }
}

#[test]
fn ai_export_respells_canonical() {
    let module = parse("c = 128ktokens\n", "test").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml");
    assert!(toml.contains("128000tokens"), "{toml}");
    let v = eval_json("c = \"ctx ${x}\"\nlet x = 4ktokens\n");
    assert_eq!(v["c"], "ctx 4000tokens", "{v}");
}

#[test]
fn ai_round_trip_through_export() {
    for src in ["x = 128ktokens\n", "x = 500tok/s\n", "x = 2TFLOPS\n"] {
        let first = eval_json(src);
        let module = parse(src, "test").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = active_core::convert::value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(first, second, "{src} via {exported:?}");
    }
}

#[test]
fn ai_cross_dimension_refused() {
    for src in [
        "x = 128ktokens + 1s\n",
        "x = 500tok/s + 25%\n",
        "x = 2TFLOPS + 100\n",
        "x = 128ktokens + 128000\n",
    ] {
        let codes = codes_of(src);
        assert!(codes.iter().any(|c| c == "T001"), "{src} -> {codes:?}");
    }
}
