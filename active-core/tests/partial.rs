//! Partial evaluation cones + incremental memo (PROPOSAL §2.2–§2.3,
//! CHECKLIST PE.001–PE.005). Drives the real entry points: project on
//! evaluated trees, CLI-equivalent engine paths, and the public memo.

use active_core::convert::project_path;
use active_core::{parse, Engine};

fn evaluated(src: &str) -> active_core::Value {
    let module = parse(src, "partial").expect("must parse");
    Engine::new()
        .evaluate(&module)
        .map(|(v, _)| v)
        .unwrap_or_else(|e| panic!("must evaluate: {e:?}"))
}

fn json(v: &active_core::Value) -> serde_json::Value {
    active_core::convert::value_to_json(v)
}

#[test]
fn path_selects_nested_and_indexed_cones() {
    let v = evaluated("server = { host = \"h\", ports = [80, 443] }\n");
    let host = project_path(&v, "server.host").expect("cone");
    assert_eq!(json(host), serde_json::json!("h"));
    let port = project_path(&v, "server.ports[1]").expect("cone");
    assert_eq!(json(port), serde_json::json!(443));
}

#[test]
fn unknown_or_malformed_paths_fail_closed() {
    let v = evaluated("server = { host = \"h\" }\n");
    assert!(project_path(&v, "server.missing").is_err());
    assert!(project_path(&v, "server.host.deeper").is_err());
    assert!(project_path(&v, "").is_err());
}

#[test]
fn budgets_guard_the_whole_graph_behind_a_cone() {
    // A pathological undemanded subtree still trips its budget: the
    // cone never hides it.
    let mut src = String::from("want = 1\n");
    for i in 0..2000 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let module = parse(&src, "partial").expect("must parse");
    let mut engine = Engine::new();
    engine.budget = active_core::Budget {
        max_keys: 100,
        ..active_core::Budget::default()
    };
    let err = engine.evaluate(&module).expect_err("must trip X005");
    assert_eq!(err.code(), "X005");
    // And the cone itself still resolves on a within-budget document.
    let v = evaluated("want = 1\n");
    assert_eq!(
        json(project_path(&v, "want").expect("cone")),
        serde_json::json!(1)
    );
}

#[test]
fn memo_reuses_identical_evaluations() {
    // PE.003, behaviorally: the second identical evaluation is a memo
    // hit, not a re-walk.
    let module = parse("port = 8080\n", "partial").expect("must parse");
    let engine = Engine::new();
    let mut memo = active_core::IncrementalMemo::default();
    let (v1, _) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("first eval");
    assert_eq!(memo.last_memo_hits, 0);
    let (v2, _) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("second eval");
    assert!(memo.last_memo_hits > 0, "second eval must hit memo");
    assert_eq!(
        active_core::convert::value_to_json(&v1),
        active_core::convert::value_to_json(&v2)
    );
}

#[test]
fn memo_separates_materialise_modes() {
    // The materialise flag is part of the fingerprint: flipping it
    // clears the store instead of serving stale identities.
    let module = parse("f = blob(\"text/plain\", \"hello\")\n", "partial").expect("must parse");
    let mut engine = Engine::new();
    let mut memo = active_core::IncrementalMemo::default();
    let (v1, _) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("identity eval");
    assert!(!active_core::convert::value_to_json(&v1)
        .to_string()
        .contains("payload_b64"));
    engine.materialise_blobs = true;
    let (v2, _) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("materialised eval");
    assert!(active_core::convert::value_to_json(&v2)
        .to_string()
        .contains("payload_b64"));
}

#[test]
fn blob_aliases_charge_per_clone() {
    // PE.004: N materialised aliases charge N payloads (no silent 1GB).
    let mut src = String::from("f = blob(\"text/plain\", \"hello\")\n");
    for i in 0..10 {
        src.push_str(&format!("a{i} = f\n"));
    }
    let module = parse(&src, "partial").expect("must parse");
    let mut engine = Engine::new();
    engine.materialise_blobs = true;
    engine.budget = active_core::Budget {
        max_total_value_size: 20,
        ..active_core::Budget::default()
    };
    let err = engine.evaluate(&module).expect_err("must trip total size");
    assert_eq!(err.code(), "X006");
}

#[test]
fn cone_projection_never_leaks_secrets() {
    // A demanded cone containing secrets still redacts on export.
    let v = evaluated("token = secret(\"s3cr3t-cone\")\n[out]\npass = token\n");
    let cone = project_path(&v, "out").expect("cone");
    let text = serde_json::to_string(&json(cone)).expect("valid json");
    assert!(!text.contains("s3cr3t-cone"), "leak: {text}");
    assert_eq!(json(cone)["pass"], serde_json::Value::Null);
}
