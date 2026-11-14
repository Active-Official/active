//! Cross-table reference resolution (LANG-A-002).
//!
//! References resolve sibling-first, then through enclosing tables
//! (parent fallback), and dotted paths read a key from a named table.
//! A missing name still warns W001 with a null value; evaluation of a
//! cyclic shape terminates.

use active_core::{parse, Engine};

fn eval(src: &str) -> (serde_json::Value, Vec<String>) {
    let module = parse(src, "refs").expect("parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("evaluate");
    let json =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    (json, diags.iter().map(|d| d.code.clone()).collect())
}

#[test]
fn a_nested_table_reads_a_root_key() {
    let (v, codes) = eval("a = 42\n[t]\nx = a\n");
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(v["t"]["x"], 42);
}

#[test]
fn interpolations_read_enclosing_keys() {
    let (v, codes) = eval("host = \"h\"\nport = 1\n[t]\nu = \"${host}:${port}\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(v["t"]["u"], "h:1");
}

#[test]
fn dotted_paths_read_a_named_table() {
    let (v, codes) = eval("server = { port = 8080 }\np = server.port\n");
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(v["p"], 8080);
}

#[test]
fn a_dotted_path_through_a_header_table_resolves() {
    let (v, codes) = eval("[server]\nport = 9090\n\n[client]\np = server.port\n");
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(v["client"]["p"], 9090);
}

#[test]
fn a_missing_dotted_segment_still_warns_and_yields_null() {
    let (v, codes) = eval("server = { port = 1 }\np = server.host\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
    assert_eq!(v["p"], serde_json::Value::Null);
}

#[test]
fn a_cyclic_dotted_reference_terminates() {
    let src = "a = { b = c.self }\nc = { self = a.b }\n";
    let module = parse(src, "refs").expect("parse");
    // Either a cycle diagnostic or a null result is fine; the contract is
    // that evaluation terminates without hanging.
    let _ = Engine::new().evaluate(&module);
}
