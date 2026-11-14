//! Diagnostic-message near-miss tests (LANG-A-007).
//!
//! Codes are unchanged; only the text names the real cause. Each case is
//! driven through the shipped parser so a message regression fails here.

use active_core::parse;

fn first(src: &str) -> (String, String) {
    let diags = parse(src, "msg").expect_err("must fail");
    (diags[0].code.clone(), diags[0].message.clone())
}

#[test]
fn exponent_looking_literals_name_exponent_notation() {
    let (code, msg) = first("a = 1e5\n");
    assert_eq!(code, "E002");
    assert!(msg.contains("exponent notation"), "{msg}");
}

#[test]
fn hex_looking_literals_name_hex() {
    let (code, msg) = first("a = 0x10\n");
    assert_eq!(code, "E002");
    assert!(msg.contains("hex literals"), "{msg}");
}

#[test]
fn detached_units_name_attachment() {
    let (code, msg) = first("a = 25 %\n");
    assert_eq!(code, "E005");
    assert!(msg.contains("attach directly"), "{msg}");
    let (code, msg) = first("a = 5S\n");
    assert_eq!(code, "E002");
    assert!(msg.contains("unknown unit suffix `S`"), "{msg}");
}

#[test]
fn division_by_zero_names_itself() {
    // Integer, float, and mixed division all reach the same message; the
    // code stays T001 (no renumbering).
    for src in ["a = 1 / 0\n", "a = 1.0 / 0.0\n", "a = 1 / 0.0\n"] {
        let module = parse(src, "msg").expect("parse");
        let err = active_core::Engine::new()
            .evaluate(&module)
            .expect_err("must fail");
        let text = format!("{err}");
        assert!(text.contains("division by zero"), "{src}: {text}");
        assert_eq!(err.code(), "T001");
    }
}
