//! L-P44: units as types — dimensional analysis in the type step.
//!
//! The rules are written against the B-011 pin (heal.rs:258-263):
//! negative durations are REJECTED, never clamped. `eval_binary` is the
//! only layer that sees operand dimensions, so the rules live there and
//! surface as T001; the schema layer owns value-vs-bound dimensions
//! (T003). Spec: specification.md Ch. 4 §4.3.

use active_core::convert::value_to_pretty_json;
use active_core::{parse, Engine};

fn eval_json(src: &str) -> serde_json::Value {
    let module = parse(src, "dimensions").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    serde_json::from_str(&value_to_pretty_json(&value)).expect("valid json")
}

fn eval_error(src: &str) -> (String, String) {
    let module = parse(src, "dimensions").expect("must parse");
    match Engine::new().evaluate(&module) {
        Ok((_, diags)) => panic!("expected a fatal type error, got diagnostics {diags:?}"),
        Err(error) => (error.code().to_string(), error.to_string()),
    }
}

fn eval_raw(src: &str) -> String {
    let module = parse(src, "dimensions").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    value_to_pretty_json(&value)
}

#[test]
fn same_dimension_arithmetic_is_unchanged() {
    // The values a dimension-correct document produced before L-P44
    // produce exactly the same JSON after it.
    let json =
        eval_json("a = 2s + 3s\nb = 2s - 1s\nc = 10s / 2s\nd = 3 * 2s\ne = 2s * 3\nf = 12V + 2V\n");
    assert_eq!(
        json["a"],
        serde_json::json!({"$active.duration": {"ms": 5000}})
    );
    assert_eq!(
        json["b"],
        serde_json::json!({"$active.duration": {"ms": 1000}})
    );
    assert_eq!(json["c"], 5);
    assert_eq!(
        json["d"],
        serde_json::json!({"$active.duration": {"ms": 6000}})
    );
    assert_eq!(
        json["e"],
        serde_json::json!({"$active.duration": {"ms": 6000}})
    );
    assert_eq!(
        json["f"],
        serde_json::json!({"$active.quantity": {"unit": "V", "value": 14.0}})
    );
}

#[test]
fn scalar_divided_by_time_is_a_per_second_rate() {
    // Bytes are Int at evaluation (LANG-B-005), so bytes / duration is
    // the scalar / time case: a per-second rate as Float. 2 KiB over one
    // second is 2048 per second.
    assert_eq!(eval_json("r = 2048B / 1s\n")["r"], 2048.0);
    assert_eq!(eval_json("r = 1 / 2s\n")["r"], 0.5);
    assert_eq!(eval_json("r = 1.5 / 1s\n")["r"], 1.5);
}

#[test]
fn negative_duration_results_are_rejected_not_clamped() {
    // The B-011 pin on the arithmetic surface (L-P44). Each used to
    // silently produce 0ms ("expires immediately").
    for (src, message) in [
        (
            "v = 1s - 2s\n",
            "duration subtraction produced a negative result",
        ),
        (
            "v = 1s * -1\n",
            "scaling a duration by a negative factor produced a negative duration",
        ),
        (
            "v = -1 * 1s\n",
            "scaling a duration by a negative factor produced a negative duration",
        ),
        (
            "v = 1s / -2\n",
            "dividing a duration by a negative factor produced a negative duration",
        ),
        (
            "v = 0.5ms * -0.5\n",
            "scaling a duration by a negative factor produced a negative duration",
        ),
    ] {
        let (code, text) = eval_error(src);
        assert_eq!(code, "T001", "{src}");
        assert!(text.contains(message), "{src}: {text}");
    }
    // Zero remains a valid duration, including a negative zero and a
    // zero magnitude scaled by a negative factor (the rule rejects
    // negative RESULTS, not negative factors: 0 * -1 is still 0).
    assert_eq!(
        eval_json("v = 1s * -0\n")["v"],
        serde_json::json!({"$active.duration": {"ms": 0}})
    );
    assert_eq!(
        eval_json("v = 0s - 0s\n")["v"],
        serde_json::json!({"$active.duration": {"ms": 0}})
    );
    assert_eq!(
        eval_json("v = 0s * -1\n")["v"],
        serde_json::json!({"$active.duration": {"ms": 0}})
    );
    assert_eq!(
        eval_json("v = 0s / -2\n")["v"],
        serde_json::json!({"$active.duration": {"ms": 0}})
    );
    assert_eq!(
        eval_json("v = 1s * -0.0\n")["v"],
        serde_json::json!({"$active.duration": {"ms": 0}})
    );
    // Negating a zero duration is negating zero.
    assert_eq!(
        eval_json("v = -0s\n")["v"],
        serde_json::json!({"$active.duration": {"ms": 0}})
    );
}

#[test]
fn negation_below_absolute_zero_is_rejected() {
    // The 0 K clamp is for literals; an explicit negation must not slip
    // below it. Folding keeps the valid negative Celsius/Fahrenheit
    // readings (they are above 0 K).
    let (code, text) = eval_error("v = -5K\n");
    assert_eq!(code, "T001");
    assert!(text.contains("below absolute zero"), "{text}");
    assert_eq!(
        eval_json("v = -5Cel\n")["v"],
        serde_json::json!({"$active.quantity": {"unit": "K", "value": 268.15}})
    );
    let fahr = eval_json("v = -40Fahr\n")["v"]["$active.quantity"]["value"]
        .as_f64()
        .expect("value");
    assert!((fahr - 233.15).abs() < 1e-9, "{fahr}");
    // A negated zero is still zero kelvin, and the canonical export keeps
    // the plus sign of zero so the content hash matches 0 K.
    let raw = eval_raw("v = -0K\n");
    assert!(raw.contains("\"value\": 0.0"), "{raw}");
    assert!(!raw.contains("-0.0"), "{raw}");
    assert_eq!(
        eval_json("v = -0K\n")["v"],
        serde_json::json!({"$active.quantity": {"unit": "K", "value": 0.0}})
    );
}

#[test]
fn cross_dimension_arithmetic_names_both_dimensions() {
    for (src, left, right) in [
        ("v = 12V + 2A\n", "quantity:V", "quantity:A"),
        ("v = 5s + 1\n", "time (duration)", "scalar (number)"),
        ("v = 5s + 1B\n", "time (duration)", "scalar (number)"),
        ("v = 5V < 2A\n", "quantity:V", "quantity:A"),
    ] {
        let (code, text) = eval_error(src);
        assert_eq!(code, "T001", "{src}");
        assert!(text.contains("dimension mismatch"), "{src}: {text}");
        assert!(text.contains(left), "{src} names {left}: {text}");
        assert!(text.contains(right), "{src} names {right}: {text}");
    }
}

#[test]
fn non_arithmetic_quantities_refuse_arithmetic_but_compare() {
    let (code, text) = eval_error("v = 20Cel + 20Cel\n");
    assert_eq!(code, "T001");
    assert!(text.contains("absolute temperature"), "{text}");
    assert!(text.contains("affine"), "{text}");

    let (code, text) = eval_error("v = 3dB + 3dB\n");
    assert_eq!(code, "T001");
    assert!(text.contains("decibels"), "{text}");

    let (code, text) = eval_error("v = 20Cel * 2\n");
    assert_eq!(code, "T001");
    assert!(text.contains("absolute temperature"), "{text}");

    // The affine rule wins over division by zero: arithmetic on a
    // temperature is undefined regardless of the divisor (review F5).
    let (code, text) = eval_error("v = 20Cel / 0\n");
    assert_eq!(code, "T001");
    assert!(text.contains("absolute temperature"), "{text}");
    let (code, text) = eval_error("v = 20Cel / 0K\n");
    assert_eq!(code, "T001");
    assert!(text.contains("absolute temperature"), "{text}");

    // Equality and ordering still work through the canonical form.
    assert_eq!(eval_json("v = 20Cel < 30Cel\n")["v"], true);
    assert_eq!(eval_json("v = 3dB == 3dB\n")["v"], true);
    // Negation is not arithmetic composition: -3 dB is attenuation and
    // stays a meaningful value (review F3).
    assert_eq!(
        eval_json("v = -3dB\n")["v"],
        serde_json::json!({"$active.quantity": {"unit": "dB", "value": -3.0}})
    );
}

#[test]
fn division_by_zero_names_itself() {
    for src in [
        "v = 1s / 0\n",
        "v = 1s / 0.0\n",
        "v = 1s / 0ms\n",
        "v = 1V / 0\n",
        "v = 1V / 0.0\n",
        "v = 1 / 0s\n",
        "v = 1.0 / 0s\n",
        "v = 1B / 0s\n",
    ] {
        let (code, text) = eval_error(src);
        assert_eq!(code, "T001", "{src}");
        assert!(text.contains("division by zero"), "{src}: {text}");
    }
}

#[test]
fn unary_negation_of_a_duration_stays_rejected() {
    let (code, text) = eval_error("v = -5s\n");
    assert_eq!(code, "T001");
    assert!(
        text.contains("unsupported unary op Neg on duration"),
        "{text}"
    );
}

#[test]
fn negative_zero_is_canonicalized_in_every_numeric_dimension() {
    // IEEE has two zeros; the language and its content hash have one
    // (review round 3). Every creation path normalizes the sign.
    for src in ["v = -0V\n", "v = 0V * -0.0\n", "v = -0.0\n", "v = -0%\n"] {
        let raw = eval_raw(src);
        assert!(!raw.contains("-0.0"), "{src}: {raw}");
    }
}

#[test]
fn integral_duration_scaling_is_exact_beyond_f64_precision() {
    // Build an exact odd millisecond count above 2^53 through duration
    // arithmetic (2^53 is exactly representable), then double it: the
    // integral-factor fast path must stay exact, where the f64 product
    // would lose a millisecond (review round 3).
    let value = eval_json("v = (9007199254740992ms + 1ms) * 2\n");
    assert_eq!(
        value["v"]["$active.duration"]["ms"], 18014398509481986u64,
        "{value}"
    );
}

#[test]
fn sub_millisecond_magnitudes_are_zero_at_the_resolution_boundary() {
    // Durations have millisecond resolution: 1ns canonicalizes to 0ms
    // before any sign rule, so scaling it is zero. Pinned, not silent.
    assert_eq!(
        eval_json("v = 1ns * -1\n")["v"],
        serde_json::json!({"$active.duration": {"ms": 0}})
    );
    assert_eq!(
        eval_json("v = -1ns\n")["v"],
        serde_json::json!({"$active.duration": {"ms": 0}})
    );
}
