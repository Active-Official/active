//! L-P14: the determinism property (D-1, spec/reproducibility.md §1.2).
//!
//! Same input tuple -> byte-identical canonical JSON, redacted JSON,
//! diagnostics, and content hash. The tuple is the module, the injected
//! env snapshot, and the engine configuration; the evaluator build is
//! fixed by the test binary.

use active_core::convert::{redact_value, value_to_pretty_json};
use active_core::provenance::content_hash;
use active_core::secret::SecretProfile;
use active_core::{parse, Engine, Module};

/// Everything D-1 promises is identical, flattened into one comparable
/// string: the value bytes, the redacted bytes, the content hash, and the
/// serialized diagnostics (codes, paths, spans, order).
fn snapshot(engine: &Engine, module: &Module) -> String {
    let (value, diagnostics, seeds) = engine
        .evaluate_with_seeds(module)
        .unwrap_or_else(|error| panic!("evaluation: {error}"));
    let profile = SecretProfile::for_engine(module, engine, &seeds);
    let redacted = redact_value(&profile, &value);
    format!(
        "{}\n--\n{}\n--\n{}\n--\n{}",
        value_to_pretty_json(&value),
        value_to_pretty_json(&redacted),
        // The content hash is over the redacted tree (spec §3): hashing the
        // raw tree would be a secret-equality oracle for a plain string
        // that came from an env read.
        content_hash(&redacted),
        serde_json::to_string(&diagnostics).expect("diagnostics serialize")
    )
}

fn parse_ok(source: &str) -> Module {
    parse(source, "determinism").unwrap_or_else(|diags| panic!("parse: {diags:?}"))
}

fn assert_deterministic(engine: &Engine, source: &str) {
    let module = parse_ok(source);
    let first = snapshot(engine, &module);
    for _ in 0..3 {
        assert_eq!(
            snapshot(engine, &module),
            first,
            "same input tuple must produce identical bytes"
        );
    }
    assert!(
        first.contains("sha256:"),
        "the hash is part of the snapshot"
    );
}

#[test]
fn deterministic_evaluation_is_the_named_property() {
    // D-1: the spec section this test pins.
    let engine = Engine::new();
    assert_deterministic(
        &engine,
        "schema = { port = Int(default = 8080), mode = Enum(\"a\", \"b\", default = \"a\") }\n\
         name = \"svc\"\n\
         list = [1, 2, 3]\n",
    );
}

#[test]
fn strict_mode_is_deterministic() {
    let engine = Engine::strict();
    assert_deterministic(
        &engine,
        "schema = { host = String, port = Int }\nhost = \"db\"\nport = 5432\n",
    );
}

#[test]
fn an_injected_env_snapshot_is_deterministic() {
    use std::collections::HashMap;
    let mut env = HashMap::new();
    env.insert("SNAP".to_string(), "value".to_string());
    let engine = Engine::new().with_env(env);
    assert_deterministic(&engine, "v = env.SNAP ?: \"missing\"\ncopy = \"{v}\"\n");
}

#[test]
fn redacted_secret_values_are_deterministic() {
    let engine = Engine::new();
    assert_deterministic(&engine, "token = secret(\"hunter2\")\ncopy = \"{token}\"\n");
}

#[test]
fn a_selected_profile_is_deterministic() {
    let engine = Engine::new();
    assert_deterministic(
        &engine,
        "active_profile = \"prod\"\n\
         base = 1\n\
         profile \"prod\":\n  base = 2\n  extra = \"x\"\n",
    );
}

#[test]
fn the_same_inputs_hash_identically_after_a_second_engine() {
    // A fresh engine with the same configuration is the same tuple.
    let source = "schema = { n = Int }\nn = 7\n";
    let module = parse_ok(source);
    let first = snapshot(&Engine::new(), &module);
    let second = snapshot(&Engine::new(), &module);
    assert_eq!(first, second, "engine construction adds no hidden input");
}

#[test]
fn the_content_hash_is_independent_of_source_key_order() {
    // §3 promises compact JSON with sorted keys. `serde_json` runs with
    // `preserve_order`, so the canonicaliser must sort explicitly.
    let one = parse_ok("a = 1\nb = 2\n");
    let two = parse_ok("b = 2\na = 1\n");
    let (one_value, _) = Engine::new().evaluate(&one).expect("evaluate");
    let (two_value, _) = Engine::new().evaluate(&two).expect("evaluate");
    assert_eq!(
        content_hash(&one_value),
        content_hash(&two_value),
        "the same table in another key order is the same content"
    );
}

#[test]
fn redacted_secret_values_share_an_identity() {
    // The hash is over the redacted tree, so two different secrets do not
    // produce two distinguishable content hashes (no equality oracle).
    let a = parse_ok("token = secret(\"alpha-secret\")\n");
    let b = parse_ok("token = secret(\"beta-secret\")\n");
    let hash_of = |module: &Module| {
        let engine = Engine::new();
        let (value, _, seeds) = engine.evaluate_with_seeds(module).expect("evaluate");
        let profile = SecretProfile::for_engine(module, &engine, &seeds);
        content_hash(&redact_value(&profile, &value))
    };
    assert_eq!(hash_of(&a), hash_of(&b));
}
