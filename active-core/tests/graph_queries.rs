//! Decidable graph query surface (PROPOSAL §2.6, CHECKLIST Phase GQ).
//!
//! Drives the real shipped functions on parsed + evaluated documents:
//! shortest paths, secret-tainted cones, and content-addressed size
//! queries — all value-free in output.

use active_core::graph::{blobs_above_size, secret_tainted_paths, shortest_path};
use active_core::{parse, Engine};

fn evaluated(src: &str) -> (active_core::Module, active_core::Value) {
    let module = parse(src, "gq").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    (module, value)
}

#[test]
fn secret_tainted_cone_includes_plain_locator_sources() {
    // A plain string used as a secret locator and an interpolation over
    // it are material without a `Value::Secret` marker.
    let (module, value) = evaluated("x = \"hunter2-gq\"\nmsg = \"prefix ${x}\"\nk = secret(x)\n");
    let paths = active_core::graph::secret_tainted_paths_with(&module, &value);
    for p in ["x", "msg", "k"] {
        assert!(paths.contains(&p.to_string()), "{paths:?}");
    }
    assert!(!paths.iter().any(|p| p.contains("hunter2")), "{paths:?}");
}

#[test]
fn shortest_path_follows_dependency_direction() {
    let (module, _) = evaluated("a = 1\nb = a\nc = b\n");
    let id = |k: &str| active_core::graph::resolve_key(&module, k).expect("key");
    assert_eq!(
        shortest_path(&module, id("a"), id("c")),
        Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
    );
    // No backward path: edges run dependency → dependent.
    assert_eq!(shortest_path(&module, id("c"), id("a")), None);
}

#[test]
fn secret_tainted_cone_lists_paths_not_values() {
    // Same-scope secret alias plus a nested direct secret: the cone
    // names every tainted path and its containers, never values.
    let (_, value) = evaluated(
        "token = secret(\"s3cr3t-gq\")\npass = token\n[svc]\nkey = secret(\"k\")\nport = 1\n",
    );
    let paths = secret_tainted_paths(&value);
    for p in ["token", "pass", "svc", "svc.key"] {
        assert!(paths.contains(&p.to_string()), "{paths:?}");
    }
    assert!(!paths.iter().any(|p| p == "svc.port"), "{paths:?}");
    // The evaluated JSON redacts; the cone carries no raw material.
    let text = serde_json::to_string(&active_core::convert::value_to_json(&value)).expect("json");
    assert!(!text.contains("s3cr3t-gq"), "{text}");
}

#[test]
fn blobs_above_size_filters_by_floor() {
    let (_, value) = evaluated(
        "small = blob(\"text/plain\", \"hi\")\nbig = blob(\"text/plain\", \"hello world, this is a blob\")\n",
    );
    let all = blobs_above_size(&value, 1);
    assert_eq!(all.len(), 2);
    let big = blobs_above_size(&value, 10);
    assert_eq!(big.len(), 1);
    assert_eq!(big[0].0, "big");
    assert!(big[0].1 >= 10, "{big:?}");
}
