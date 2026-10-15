//! Healing tests (C-016).
//!
//! Drives the real parse path in both modes and asserts the confidence
//! policy: safe repairs (≥ 0.95) apply in both strict and resilient modes
//! with Info severity; medium repairs (0.70–0.94) apply only in resilient
//! mode, and strict mode turns them into failures. Also unit-tests the
//! phase-2 similarity engine and phase-3 coercion functions.

use solid_core::diag::Severity;
use solid_core::heal::{
    best_key_match, coerce_string_to, key_similarity, keyboard_similarity, normalized_levenshtein,
    phonetic_similarity, sentinel_meaning, try_coerce_duration, try_coerce_int,
};
use solid_core::types::SolidType;
use solid_core::{parse_with, ParseOptions};

const STRICT: ParseOptions = ParseOptions { resilient: false };
const RESILIENT: ParseOptions = ParseOptions { resilient: true };

fn codes(src: &str, options: ParseOptions) -> Vec<String> {
    let module = parse_with(src, "healing", options).expect("must parse");
    module.healing.iter().map(|d| d.code.clone()).collect()
}

// --- Phase 1: syntactic repairs -------------------------------------------

#[test]
fn unclosed_string_is_healed_in_both_modes() {
    // H001 is a safe repair: applied even in strict mode.
    for options in [STRICT, RESILIENT] {
        let module = parse_with("x = \"abc\n", "healing", options).expect("must parse");
        let d = module
            .healing
            .iter()
            .find(|d| d.code == "H001")
            .expect("H001 expected");
        assert_eq!(d.severity, Severity::Info, "safe repair must be Info");
        assert!(d.confidence >= 0.95);
        assert!(d.repair.is_some());
    }
}

#[test]
fn trailing_comma_is_healed_in_both_modes() {
    for options in [STRICT, RESILIENT] {
        let module = parse_with("x = [1, 2,]\n", "healing", options).expect("must parse");
        let d = module
            .healing
            .iter()
            .find(|d| d.code == "H002")
            .expect("H002 expected");
        assert_eq!(d.severity, Severity::Info);
    }
}

#[test]
fn missing_comma_is_healed_in_both_modes_with_a_warning() {
    // H003 sits in the 0.85–0.94 band: applied in every mode, but as a
    // Warning — which makes strict mode exit non-zero at the CLI level.
    for options in [STRICT, RESILIENT] {
        let module = parse_with("x = [1 2]\n", "healing", options).expect("must parse");
        let d = module
            .healing
            .iter()
            .find(|d| d.code == "H003")
            .expect("H003 expected");
        assert_eq!(d.severity, Severity::Warning);
        assert!((0.85..0.95).contains(&d.confidence));
    }
}

#[test]
fn unexpected_character_is_a_hard_error_in_both_modes() {
    // Below the medium band, so it is never auto-applied — the input is
    // rejected rather than silently altered.
    for options in [STRICT, RESILIENT] {
        let err = parse_with("x = 1 @ 2\n", "healing", options)
            .expect_err("unknown characters must be rejected");
        assert!(err.iter().any(|d| d.code == "E005"), "got {err:?}");
    }
}

#[test]
fn dangling_escape_backslash_is_preserved() {
    // `x = "abc\` — the trailing escape has nothing to escape; the
    // backslash must survive into the healed value.
    let module = parse_with("x = \"abc\\\n", "healing", STRICT).expect("must parse");
    assert!(
        module.healing.iter().any(|d| d.code == "H001"),
        "H001 expected"
    );
    let (value, _) = solid_core::Engine::new()
        .evaluate(&module)
        .expect("must evaluate");
    let json = solid_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("abc\\\\"), "backslash preserved, got {json}");
}

#[test]
fn microsecond_suffix_is_lexable() {
    // The whitelist documents `µs`; the scanner must collect it.
    let module = parse_with("x = 1µs\n", "healing", STRICT).expect("must parse");
    let (value, _) = solid_core::Engine::new()
        .evaluate(&module)
        .expect("must evaluate");
    let json = solid_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("0"), "1µs normalizes to sub-ms, got {json}");
}

#[test]
fn secrets_never_appear_in_type_error_messages() {
    // Security invariant: secret material must not leak through
    // diagnostics, and type errors are a diagnostic surface.
    let module =
        parse_with("x = secret(\"TOPSECRET\") + 1\n", "healing", STRICT).expect("must parse");
    let err = solid_core::Engine::new()
        .evaluate(&module)
        .expect_err("type error expected");
    let rendered = format!("{}", err);
    assert!(
        !rendered.contains("TOPSECRET"),
        "secret leaked into the error message: {rendered}"
    );
    assert!(rendered.contains("***"), "expected a redaction marker");
}

#[test]
fn bracket_mismatch_fails_strict_and_heals_resilient() {
    // Strict: mismatched closer is an error.
    let err = parse_with("x = [1}\n", "healing", STRICT).expect_err("strict must fail");
    assert!(err.iter().any(|d| d.code == "E001"));
    // Resilient: repaired at medium confidence (resilient-only band).
    let module = parse_with("x = [1}\n", "healing", RESILIENT).expect("resilient must parse");
    let d = module
        .healing
        .iter()
        .find(|d| d.code == "H004")
        .expect("H004 expected");
    assert_eq!(d.severity, Severity::Warning);
    assert!((0.70..0.85).contains(&d.confidence), "0.70–0.84 band");
}

#[test]
fn healed_documents_still_evaluate() {
    // The whole point: a healed document produces a usable value tree.
    let module = parse_with("x = [1, 2,]\ny = 5\n", "healing", STRICT).expect("must parse");
    let (value, _) = solid_core::Engine::new()
        .evaluate(&module)
        .expect("must evaluate");
    let json = solid_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("\"y\": 5"), "got {json}");
}

#[test]
fn clean_documents_produce_no_healing() {
    let module = parse_with("x = [1, 2]\ny = 5\n", "healing", STRICT).expect("must parse");
    assert!(module.healing.is_empty(), "clean input must not be healed");
}

// --- Phase 2: similarity engine -------------------------------------------

#[test]
fn levenshtein_similarity_is_normalized_and_case_insensitive() {
    assert_eq!(normalized_levenshtein("host", "host"), 1.0);
    assert_eq!(normalized_levenshtein("Host", "HOST"), 1.0);
    assert!(normalized_levenshtein("host", "hots") > 0.7);
    assert!(normalized_levenshtein("host", "xyzzy") < 0.2);
}

#[test]
fn keyboard_similarity_rewards_adjacent_keys() {
    assert_eq!(keyboard_similarity("host", "host"), 1.0);
    assert!(keyboard_similarity("host", "gist") > 0.5, "adjacent keys");
    // h/p and s/r are not adjacent; o/o and t/t match → 2 of 4.
    assert_eq!(keyboard_similarity("host", "port"), 0.5);
    assert_eq!(
        keyboard_similarity("host", "hosting"),
        0.0,
        "length mismatch"
    );
}

#[test]
fn phonetic_similarity_matches_sound_alikes() {
    assert_eq!(phonetic_similarity("framework", "framwork"), 1.0);
    assert_eq!(phonetic_similarity("host", "host"), 1.0);
    assert_eq!(phonetic_similarity("host", "mispelled"), 0.0);
}

#[test]
fn best_key_match_finds_the_typo_target() {
    let legal: Vec<String> = ["host", "port", "timeout", "replicas"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let (m, score) = best_key_match("hst", &legal, 0.70).expect("match expected");
    assert_eq!(m, "host");
    assert!(score > 0.7);
    // Transposed characters.
    let (m, _) = best_key_match("prot", &legal, 0.70).expect("match expected");
    assert_eq!(m, "port");
    // Nothing close enough.
    assert!(best_key_match("zzzzzzz", &legal, 0.85).is_none());
    // High self-similarity.
    assert!(key_similarity("timeout", "timeout") > 0.95);
}

// --- Phase 3: safe coercions ----------------------------------------------

#[test]
fn integer_coercion_is_strict() {
    assert_eq!(try_coerce_int("42"), Some(42));
    assert_eq!(try_coerce_int(" 7 "), Some(7));
    assert_eq!(try_coerce_int("4.5"), None);
    assert_eq!(try_coerce_int("$40"), None);
    assert_eq!(try_coerce_int("abc"), None);
}

#[test]
fn duration_coercion_parses_unit_suffixes() {
    assert_eq!(try_coerce_duration("5s"), Some(5000));
    assert_eq!(try_coerce_duration("300ms"), Some(300));
    assert_eq!(try_coerce_duration("1.5h"), Some(5_400_000));
    assert_eq!(try_coerce_duration("5x"), None);
    assert_eq!(try_coerce_duration("seconds"), None);
}

#[test]
fn sentinel_words_map_to_intent() {
    assert_eq!(sentinel_meaning("unlimited"), Some("limit"));
    assert_eq!(sentinel_meaning("None"), Some("absent"));
    assert_eq!(sentinel_meaning("inf"), Some("infinite"));
    assert_eq!(sentinel_meaning("42"), None);
}

#[test]
fn coerce_string_to_target_type() {
    use solid_core::Value;
    assert!(matches!(
        coerce_string_to("42", &SolidType::Int),
        Some(Value::Int(42))
    ));
    assert!(matches!(
        coerce_string_to("yes", &SolidType::Bool),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        coerce_string_to("5s", &SolidType::Duration),
        Some(Value::DurationMs(5000))
    ));
    assert!(coerce_string_to("nope", &SolidType::Int).is_none());
    assert!(coerce_string_to("42", &SolidType::String).is_none());
}

// --- Policy summary -------------------------------------------------------

#[test]
fn confidence_policy_is_reflected_in_severity() {
    // Info  ⇔ confidence ≥ 0.95 (auto-apply, both modes)
    // Warning ⇔ 0.70–0.94 (mode-gated)
    let module = parse_with("x = [1, 2,]\n", "healing", STRICT).expect("must parse");
    for d in &module.healing {
        if d.severity == Severity::Info {
            assert!(d.confidence >= 0.95, "{d:?}");
        } else if d.severity == Severity::Warning {
            assert!((0.70..0.95).contains(&d.confidence), "{d:?}");
        }
    }
    let _ = codes("x = 1\n", STRICT); // smoke: helper compiles and returns
}
