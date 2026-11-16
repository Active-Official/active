//! Environment name folding (LANG-H-003): `env.NAME` is case-insensitive
//! on every platform, so a document authored on Windows resolves the same
//! variable on Linux.

use active_core::{parse, Engine};
use std::collections::HashMap;

fn evaluated(src: &str, env: &[(&str, &str)]) -> String {
    let module = parse(src, "env-case").expect("must parse");
    let map: HashMap<String, String> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let (value, _) = Engine::hermetic()
        .with_env(map)
        .evaluate(&module)
        .expect("must evaluate");
    active_core::convert::value_to_pretty_json(&value)
}

#[test]
fn env_names_fold_case_insensitively() {
    let json = evaluated(
        "a = env.hk_fold_foo\nb = env.HK_FOLD_FOO\nc = env.Hk_Fold_Foo\n",
        &[("HK_FOLD_FOO", "hello")],
    );
    assert_eq!(
        json.matches("\"hello\"").count(),
        3,
        "every spelling resolves the same variable: {json}"
    );
    assert!(!json.contains("null"), "{json}");
}

#[test]
fn colliding_spellings_resolve_deterministically() {
    // On a case-sensitive filesystem an environment can hold both
    // spellings. The canonical map sorts (name, value) pairs and keeps the
    // last, so the winner never depends on hash iteration order.
    let json = evaluated("a = env.dup\n", &[("DUP", "lower"), ("dup", "upper")]);
    assert!(
        json.contains("\"upper\""),
        "lexicographically last value wins: {json}"
    );
}

#[test]
fn the_fallback_receives_the_canonical_name() {
    // Host lookups see one spelling (uppercase), matching the snapshot.
    let module = parse("a = env.some_host_value\n", "env-case").expect("must parse");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = seen.clone();
    let engine = Engine::hermetic().with_env_fallback(std::sync::Arc::new(move |name: &str| {
        sink.lock().expect("lock").push(name.to_string());
        Some("host".to_string())
    }));
    let (value, _) = engine.evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("\"host\""), "{json}");
    assert_eq!(*seen.lock().expect("lock"), ["SOME_HOST_VALUE"]);
}
