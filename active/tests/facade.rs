//! Facade contract tests: `active` is the crate dependents use.
//!
//! Proves the re-export surface resolves on `active::` paths end to end
//! (parse → evaluate → convert) and that `VERSION` tracks the package.

use active::{parse, Engine};

#[test]
fn facade_parses_evaluates_and_converts() {
    let module = parse("port = 8080\n", "facade").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active::convert::value_to_pretty_json(&value);
    assert!(json.contains("\"port\": 8080"), "{json}");
}

#[test]
fn facade_version_matches_package() {
    assert_eq!(active::VERSION, env!("CARGO_PKG_VERSION"));
    assert!(!active::VERSION.is_empty());
}
