//! H003 must not heal a malformed literal (LANG-L-001).
//!
//! `[1e308]` is a number glued to an identifier, not a missing comma.
//! Healing it fabricated `[1, null]` and `active-fmt` re-emitted the
//! phantom identifier; genuine missing-comma cases must still heal.

use active_core::{parse, Engine};

fn codes(src: &str) -> Vec<String> {
    match parse(src, "h003") {
        Ok(m) => m.healing.iter().map(|d| d.code.clone()).collect(),
        Err(diags) => diags.iter().map(|d| d.code.clone()).collect(),
    }
}

#[test]
fn exponent_like_array_elements_are_rejected_not_healed() {
    for src in ["x = [1e3]\n", "x = [1e308]\n", "x = [1e3, 2]\n"] {
        let c = codes(src);
        assert!(c.iter().any(|x| x == "E002"), "{src:?}: {c:?}");
        assert!(
            !c.iter().any(|x| x == "H003"),
            "{src:?} must not heal: {c:?}"
        );
    }
}

#[test]
fn genuine_missing_commas_still_heal() {
    let module = parse("x = [1 2]\n", "h003").expect("recovering parse");
    assert!(
        module.healing.iter().any(|d| d.code == "H003"),
        "genuine missing comma heals"
    );
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains('1'), "{json}");
    assert!(!json.contains("null"), "no phantom element: {json}");
}

#[test]
fn a_comma_after_a_bad_literal_still_fails_loudly() {
    // The guard consumes the glued identifier, so the rest of the array
    // parses and the document fails on the E002 rather than on a hang.
    let c = codes("x = [1e3, 2]\n");
    assert!(c.iter().any(|x| x == "E002"), "{c:?}");
}
