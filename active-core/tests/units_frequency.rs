//! Frequency dimension tests (goal: universal measurement, Phase A-2).
//! Spec: grammar.md §1.6 (Hz/kHz/MHz/GHz/THz row), specification.md Ch. 4
//! §4.1/§4.2 (f64 hertz canonical, arithmetic matrix, Hz-form export).
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
fn freq_canonical_hertz() {
    assert_eq!(
        eval_json("f = 60Hz\n")["f"],
        serde_json::json!({"Hz": 60.0})
    );
    assert_eq!(
        eval_json("f = 1kHz\n")["f"],
        serde_json::json!({"Hz": 1000.0})
    );
    assert_eq!(
        eval_json("f = 2.4GHz\n")["f"],
        serde_json::json!({"Hz": 2400000000.0})
    );
    assert_eq!(
        eval_json("f = 1MHz\n")["f"],
        serde_json::json!({"Hz": 1000000.0})
    );
    assert_eq!(
        eval_json("f = 1THz\n")["f"],
        serde_json::json!({"Hz": 1000000000000.0})
    );
    assert!(codes_of("f = 1kHz\n").is_empty());
}

#[test]
fn freq_fractional_kept() {
    // Fractional hertz are real (audio, drift): no int coercion.
    assert_eq!(
        eval_json("f = 0.5Hz\n")["f"],
        serde_json::json!({"Hz": 0.5})
    );
    assert!(!eval_json("f = 0.5Hz\n")["f"]["Hz"].is_i64());
}

#[test]
fn freq_export_is_hz_form() {
    // All spellings respell to canonical Hz-form, always re-parseable.
    let module = parse("f = 1kHz\n", "test").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml");
    assert!(toml.contains("1000Hz"), "{toml}");
    let v = eval_json("f = \"clock ${x}\"\nlet x = 2MHz\n");
    assert_eq!(v["f"], "clock 2000000Hz", "{v}");
}

#[test]
fn freq_same_dimension_arithmetic() {
    assert_eq!(
        eval_json("f = 1kHz + 500Hz\n")["f"],
        serde_json::json!({"Hz": 1500.0})
    );
    assert_eq!(
        eval_json("f = 2GHz - 500MHz\n")["f"],
        serde_json::json!({"Hz": 1500000000.0})
    );
    assert_eq!(
        eval_json("f = 100Hz * 2\n")["f"],
        serde_json::json!({"Hz": 200.0})
    );
    assert_eq!(
        eval_json("f = 1kHz / 2\n")["f"],
        serde_json::json!({"Hz": 500.0})
    );
    assert_eq!(eval_json("f = 2kHz / 1kHz\n")["f"], 2.0);
    assert_eq!(eval_json("f = 500Hz < 1kHz\n")["f"], true);
    assert_eq!(eval_json("f = 1MHz >= 1000kHz\n")["f"], true);
}

#[test]
fn freq_cross_dimension_is_type_error() {
    for src in [
        "x = 1kHz + 1s\n",
        "x = 1kHz + 25%\n",
        "x = 1kHz + 100\n",
        "x = 1kHz < 1m\n",
    ] {
        let codes = codes_of(src);
        assert!(codes.iter().any(|c| c == "T001"), "{src} -> {codes:?}");
    }
}

#[test]
fn freq_commensurate_spellings_equal() {
    // `1kHz == 1000Hz`: commensurability holds through canonicalization.
    assert_eq!(eval_json("f = 1kHz == 1000Hz\n")["f"], true);
    assert_eq!(eval_json("f = 1MHz == 1000kHz\n")["f"], true);
    // ...but hertz are never bare numbers.
    assert_eq!(eval_json("f = 1kHz == 1000\n")["f"], false);
}

#[test]
fn freq_case_sensitive() {
    // SI case discipline: `KHZ`/`hz`/`Mhz` are not units (E002), never Hz.
    for src in ["f = 1KHZ\n", "f = 1hz\n", "f = 1Mhz\n"] {
        let diags = parse(src, "test").expect_err("must fail");
        assert!(diags.iter().any(|d| d.code == "E002"), "{src}");
    }
    // Positive control: exact-case twins evaluate silently.
    assert!(codes_of("f = 1kHz\n").is_empty());
    assert!(codes_of("f = 1MHz\n").is_empty());
}

#[test]
fn freq_explicit_negation_preserved() {
    // Literal magnitudes clamp at zero, but explicit unary minus is kept.
    assert_eq!(
        eval_json("f = -5Hz\n")["f"],
        serde_json::json!({"Hz": -5.0})
    );
    // Positive control: plain literals canonicalize silently.
    assert!(codes_of("f = 5Hz\n").is_empty());
}
