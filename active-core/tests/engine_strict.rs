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

#[test]
fn a_zeroed_budget_does_not_disable_limits() {
    // LANG-C-015: zero means "shipped default", so a zeroed struct cannot
    // silently lift a ceiling; disabling one takes the explicit sentinel.
    use active_core::eval::{Budget, UNLIMITED};
    let zeroed = Budget {
        max_nodes: 0,
        max_depth: 0,
        max_array_len: 0,
        max_keys: 0,
        max_wall_ms: 0,
        max_value_size: 0,
        max_total_value_size: 0,
    };
    let resolved = zeroed.resolve();
    let shipped = Budget::default();
    assert_eq!(resolved, shipped, "zeros resolve to the shipped profile");

    let lifted = Budget::unlimited();
    assert_eq!(lifted.max_wall_ms, UNLIMITED);
    assert_eq!(lifted.max_nodes, UNLIMITED);
    assert!(Budget::is_unlimited(lifted.max_total_value_size));
    assert!(!Budget::is_unlimited(shipped.max_total_value_size));

    // End to end: the zeroed struct evaluates under the shipped clock, so a
    // long walk still trips X002 instead of running unbounded.
    let mut src = String::from("s = \"");
    src.push_str(&"x".repeat(4096));
    src.push_str("\"\n");
    let mut prev = "s".to_string();
    for i in 0..14 {
        src.push_str(&format!("a{i} = \"${{{prev}}}${{{prev}}}\"\n"));
        prev = format!("a{i}");
    }
    let module = parse(&src, "zeroed").expect("parse");
    let mut engine = active_core::Engine::hermetic();
    engine.budget = active_core::eval::Budget {
        // Only the clock is lowered; every other zero must NOT mean
        // unlimited, so the size ceilings must still be the shipped ones.
        max_wall_ms: 1,
        ..zeroed
    };
    let err = engine.evaluate(&module).expect_err("the walk must trip");
    assert!(
        matches!(err.code(), "X002" | "X006"),
        "a zeroed budget keeps its ceilings: {err:?}"
    );
}
