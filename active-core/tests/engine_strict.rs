//! `Engine::new()` is resilient by design; `Engine::strict()` is the
//! fail-closed constructor (LANG-C-014).
//!
//! `Ok` from a resilient evaluation means "no fatal error", not "valid": the
//! Error-severity diagnostics are in the returned vector and the host must
//! inspect them. The strict constructor turns the same diagnostics into
//! `Err(ValidationFailed)`, which is what a service host wants.

use active_core::{parse, Engine, EvalError};

/// A blob over its schema `max_size`: B002 is Error severity.
const OVER_MAX: &str = "schema = { closed = false, id = Blob(max_size = 2) }\n\
                        id = blob(\"text/plain\", \"hello\")\n";

#[test]
fn the_resilient_default_returns_ok_with_the_diagnostics() {
    let module = parse(OVER_MAX, "c014").expect("parse");
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .expect("resilient evaluation returns Ok");
    assert!(
        diags.iter().any(|d| d.code == "B002"),
        "the Error-severity diagnostic travels in the vector: {diags:?}"
    );
    // The over-max blob is still in the tree: Ok alone does not mean valid.
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("$active.blob"), "{json}");
}

#[test]
fn the_strict_constructor_fails_closed_on_the_same_document() {
    let module = parse(OVER_MAX, "c014").expect("parse");
    let err = Engine::strict()
        .evaluate(&module)
        .expect_err("strict must refuse the error-severity diagnostic");
    match err {
        EvalError::ValidationFailed { diagnostics } => assert!(
            diagnostics.iter().any(|d| d.code == "B002"),
            "{diagnostics:?}"
        ),
        other => panic!("expected a validation refusal, got {other:?}"),
    }

    // A clean document still evaluates under the strict constructor.
    let clean = parse(
        "schema = { closed = false, id = Blob(max_size = 9) }\nid = blob(\"text/plain\", \"hi\")\n",
        "clean",
    )
    .expect("parse");
    let (_, diags) = Engine::strict()
        .evaluate(&clean)
        .expect("clean document passes strict");
    assert!(diags.is_empty(), "{diags:?}");
}
