//! Int bounds against float bounds compare exactly (LANG-B-006).
//!
//! `n as f64` rounds above 2^53, so an out-of-range integer could pass a
//! float maximum. The bound comparison now uses the shipped exact
//! `cmp_int_float`.

use active_core::{parse, Engine};

fn codes(src: &str) -> Vec<String> {
    let module = parse(src, "bounds").expect("parse");
    let (_v, diags) = Engine::new().evaluate(&module).expect("evaluate");
    diags.iter().map(|d| d.code.clone()).collect()
}

#[test]
fn a_float_max_is_exact_beyond_two_to_the_53() {
    // 2^53 = 9007199254740992. The next integer is out of range but casts
    // to the same f64; the exact comparison must still reject it.
    let over = "schema = { n = Int(max = 9007199254740992.0) }\nn = 9007199254740993\n";
    assert!(codes(over).iter().any(|c| c == "T003"), "{:?}", codes(over));
    let ok = "schema = { n = Int(max = 9007199254740992.0) }\nn = 9007199254740992\n";
    assert!(!codes(ok).iter().any(|c| c == "T003"), "{:?}", codes(ok));
}

#[test]
fn a_float_min_is_exact_beyond_two_to_the_53() {
    let under = "schema = { n = Int(min = 9007199254740992.0) }\nn = 9007199254740991\n";
    assert!(codes(under).iter().any(|c| c == "T003"));
    let ok = "schema = { n = Int(min = 9007199254740992.0) }\nn = 9007199254740992\n";
    assert!(!codes(ok).iter().any(|c| c == "T003"));
}

#[test]
fn ordinary_fractional_bounds_still_work() {
    assert!(codes("schema = { n = Int(max = 10.5) }\nn = 11\n")
        .iter()
        .any(|c| c == "T003"));
    assert!(!codes("schema = { n = Int(max = 10.5) }\nn = 10\n")
        .iter()
        .any(|c| c == "T003"));
}
