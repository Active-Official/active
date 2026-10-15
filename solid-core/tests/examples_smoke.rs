//! Smoke tests: the shipped examples must parse and evaluate cleanly.
//! TODO(solid C-010): replace with the full positive/negative/spec/fuzz
//! test corpus (first 20 positive and 10 negative `.solid` files).

use solid_core::{parse, Engine, Value};

fn evaluate_example(rel: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let module = parse(&src, rel).expect("example must parse without errors");
    let engine = Engine::new();
    let (value, diags) = engine
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{rel} failed to evaluate: {e}"));
    assert!(diags.is_empty(), "{rel} produced diagnostics: {diags:?}");
    assert!(
        !matches!(value, Value::Null),
        "{rel} evaluated to the null root"
    );
}

#[test]
fn demo_example_evaluates() {
    evaluate_example("examples/demo.solid");
}

#[test]
fn complete_example_evaluates() {
    evaluate_example("examples/complete.solid");
}

#[test]
fn secret_is_redacted_in_json_export() {
    let module = parse("password = secret(\"prod/db/password\")\n", "t").expect("must parse");
    let engine = Engine::new();
    let (value, _) = engine.evaluate(&module).expect("must evaluate");
    let json = solid_core::convert::value_to_pretty_json(&value);
    assert!(
        json.contains("***SECRET***"),
        "secret must be redacted in export, got: {json}"
    );
    assert!(
        !json.contains("prod/db/password"),
        "raw secret material must never appear in export"
    );
}
