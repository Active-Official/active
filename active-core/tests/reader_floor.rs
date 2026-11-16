//! Reader-version floors (LANG-B-021).
//!
//! A host that declares its reader version gets a hard T007 refusal when a
//! document's `min_reader` is above it; a host that declares nothing keeps
//! the documented advisory behaviour. `min_writer` stays advisory: it
//! constrains writers, not readers.

use active_core::{parse, Engine};

const DOC: &str = "schema = { min_reader = \"9.9.9\", host = String }\nhost = \"x\"\n";

fn eval_with(reader: Option<&str>) -> Result<serde_json::Value, active_core::EvalError> {
    let module = parse(DOC, "floor").expect("parse");
    let mut engine = Engine::new();
    engine.reader_version = reader.map(str::to_string);
    let (value, _diags) = engine.evaluate(&module)?;
    Ok(serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json"))
}

#[test]
fn an_undeclared_reader_uses_the_running_library_version() {
    // LANG-F-006: the floors are enforced at read time. With no host
    // declaration the running library (CARGO_PKG_VERSION) is the reader, so
    // a 9.9.9 floor refuses exactly as a declared 1.x reader does.
    let err = eval_with(None).expect_err("the running reader is below 9.9.9");
    assert_eq!(err.code(), "T007", "{err}");

    // A floor the running reader satisfies still evaluates with no flag.
    let low = "schema = { min_reader = \"1.0.0\", host = String }\nhost = \"x\"\n";
    let module = parse(low, "floor").expect("parse");
    let (v, _) = Engine::hermetic()
        .evaluate(&module)
        .expect("a satisfied floor evaluates");
    let json = active_core::convert::value_to_pretty_json(&v);
    assert!(json.contains("\"host\": \"x\""), "{json}");
}

#[test]
fn an_older_declared_reader_is_refused_with_t007() {
    let err = eval_with(Some("1.4.0")).expect_err("must refuse");
    assert_eq!(err.code(), "T007", "{err}");
    assert!(format!("{err}").contains("validation failed"), "{err}");
}

#[test]
fn a_newer_or_equal_declared_reader_evaluates() {
    assert!(eval_with(Some("9.9.9")).is_ok());
    assert!(eval_with(Some("10.0.0")).is_ok());
    // Component-wise, not string order: 10.0.0 > 9.9.9.
    assert!(eval_with(Some("1.10.0")).is_err());
}

#[test]
fn min_writer_does_not_gate_reading() {
    let src = "schema = { min_writer = \"9.9.9\", host = String }\nhost = \"x\"\n";
    let module = parse(src, "floor").expect("parse");
    let mut engine = Engine::new();
    engine.reader_version = Some("1.0.0".to_string());
    let (_v, _d) = engine.evaluate(&module).expect("writer floor is advisory");
}
