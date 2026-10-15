//! Regression tests for fixed bugs. Each test names the bug it guards.

use solid_core::{parse, Engine};

fn eval_json(src: &str) -> String {
    let module = parse(src, "regression").expect("must parse");
    let engine = Engine::new();
    let (value, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    solid_core::convert::value_to_pretty_json(&value)
}

#[test]
fn sibling_self_reference_falls_through_to_let_binding() {
    // Bug: `[app] name = name` resolved the key's own value to its own field,
    // creating a self-loop cycle. It must resolve to the `let` binding.
    let json = eval_json("let name = \"outer\"\n[app]\nname = name\n");
    assert!(
        json.contains("\"outer\""),
        "app.name must equal the let binding, got: {json}"
    );
}

#[test]
fn sibling_self_reference_without_let_yields_null_not_cycle() {
    // No let binding exists; the self-reference must be a warning + null,
    // not a cycle error.
    let module = parse("[app]\nname = name\n", "regression").expect("must parse");
    let engine = Engine::new();
    let (value, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(
        !diags.is_empty(),
        "unresolved self-reference should produce a diagnostic"
    );
    let json = solid_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("null"), "expected null value, got: {json}");
}

#[test]
fn sibling_reference_to_other_key_still_resolves() {
    // Regression guard for the fix above: normal sibling references must
    // keep working, including forward references.
    let json = eval_json("[limits]\nmax = 10\ncan_scale = !is_prod\nis_prod = false\n");
    assert!(
        json.contains("\"can_scale\": true") && json.contains("\"max\": 10"),
        "sibling references must resolve, got: {json}"
    );
}
