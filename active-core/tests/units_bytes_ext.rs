//! Large binary byte tests: PiB EiB (Phase C3).
//! Spec: grammar.md §1.6, specification.md Ch. 4 §4.1 (saturating large bytes).
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
fn pebibytes_exact() {
    assert_eq!(eval_json("b = 1PiB\n")["b"], 1125899906842624_i64);
    assert_eq!(eval_json("b = 2PiB\n")["b"], 2251799813685248_i64);
    assert!(codes_of("b = 1PiB\n").is_empty());
}

#[test]
fn exbibytes_exact() {
    assert_eq!(eval_json("b = 1EiB\n")["b"], 1152921504606846976_i64);
    assert!(codes_of("b = 1EiB\n").is_empty());
}

#[test]
fn beyond_i64_saturates_never_wraps() {
    // 16EiB overflows even u64: saturation to i64::MAX, zero diagnostics.
    assert_eq!(eval_json("b = 16EiB\n")["b"], 9223372036854775807_i64);
    assert!(codes_of("b = 16EiB\n").is_empty());
    // Positive control: TiB-scale stays exact beside it.
    assert_eq!(eval_json("b = 1TiB\n")["b"], 1099511627776_i64);
}

#[test]
fn large_bytes_plain_int_ops() {
    assert_eq!(eval_json("b = 1PiB + 1PiB\n")["b"], 2251799813685248_i64);
    assert_eq!(eval_json("b = 2EiB > 1EiB\n")["b"], true);
}

#[test]
fn large_bytes_vs_bits_refused() {
    // Int bytes vs Quantity bits: different representations, TypeError.
    // (Int+Int stays valid saturating arithmetic — control in large_bytes.)
    for src in ["x = 1PiB + 8b\n", "x = 16EiB + 1Kbps\n"] {
        let codes = codes_of(src);
        assert!(codes.iter().any(|c| c == "T001"), "{src} -> {codes:?}");
    }
}
