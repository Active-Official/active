//! Playground MVP examples (L-002): the four curated fixtures keep their
//! documented diagnostics. Guards the 5-minute stranger test at the source.

use active_core::{parse, Diagnostic, Engine};
use std::path::PathBuf;

fn example_diags(name: &str) -> Vec<Diagnostic> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("playground")
        .join("examples")
        .join(format!("{name}.active"));
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let module = parse(&src, name).expect("example must parse");
    match Engine::new().evaluate(&module) {
        Ok((_, diags)) => diags,
        Err(_) => {
            // Hard errors (S004, budgets) carry the code on the error value;
            // re-check the code through the dedicated assertion below.
            Vec::new()
        }
    }
}

fn codes_of(diags: &[Diagnostic]) -> Vec<&str> {
    diags.iter().map(|d| d.code.as_str()).collect()
}

#[test]
fn norway_contrast_warns_w001() {
    let diags = example_diags("norway-contrast");
    assert!(codes_of(&diags).contains(&"W001"), "{diags:?}");
}

#[test]
fn units_mismatch_errors_t001() {
    // T001 here aborts evaluation, so assert through the CLI-proven path:
    // parse then expect the typed error code.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("playground")
        .join("examples")
        .join("units-mismatch.active");
    let src = std::fs::read_to_string(&path).expect("fixture readable");
    let module = parse(&src, "units").expect("parses");
    let err = Engine::new().evaluate(&module).expect_err("must refuse");
    assert_eq!(err.code(), "T001");
}

#[test]
fn secret_interp_refuses_s004() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("playground")
        .join("examples")
        .join("secret-interp-refusal.active");
    let src = std::fs::read_to_string(&path).expect("fixture readable");
    let module = parse(&src, "secret").expect("parses");
    let err = Engine::new().evaluate(&module).expect_err("must refuse");
    assert_eq!(err.code(), "S004");
}

#[test]
fn schema_failure_errors_t003() {
    let diags = example_diags("schema-failure");
    assert!(codes_of(&diags).contains(&"T003"), "{diags:?}");
}
