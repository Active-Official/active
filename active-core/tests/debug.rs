//! Debugger tests (TOOLING DEBUG.001-006): step traces redact secret
//! material, breakpoints select deterministically, replay compares
//! hashes and traces, and reduction preserves the target diagnostic.

use active_core::debug;
use active_core::{parse, Engine};

fn module(src: &str) -> active_core::Module {
    parse(src, "debug").expect("parse")
}

#[test]
fn steps_redact_secret_locator_literals() {
    let src = "token = secret(\"hunter2-step\")\nport = 8080\n";
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("hunter2"),
        "raw secret in steps: {rendered}"
    );
    assert!(steps.iter().any(|s| s.tainted), "taint recorded");
    assert!(steps.iter().any(|s| s.path.as_deref() == Some("token")));
}

#[test]
fn breakpoints_select_by_taint_path_and_code() {
    let src = "let base = 1\nport = base\nsecret_value = secret(\"x\")\n";
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let taint = debug::breakpoint_index(&steps, "taint").expect("taint breakpoint");
    assert!(steps[taint].tainted);
    let by_path = debug::breakpoint_index(&steps, "path:port").expect("path breakpoint");
    assert_eq!(steps[by_path].path.as_deref(), Some("port"));
    let by_node = debug::breakpoint_index(&steps, "node:0").expect("node breakpoint");
    assert_eq!(by_node, 0);
    assert!(debug::breakpoint_index(&steps, "path:nope").is_none());
    assert!(debug::breakpoint_index(&steps, "bogus").is_none());
}

#[test]
fn replay_is_deterministic_and_honors_expected_hash() {
    let src = "port = 8080\nname = \"svc\"\n";
    let module = module(src);
    let report = debug::replay(&module, None).expect("replay");
    assert_eq!(report["ok"], true);
    assert_eq!(report["steps_match"], true);
    let hash = report["hash"].as_str().expect("hash").to_string();
    let expected = debug::replay(&module, Some(&hash)).expect("replay");
    assert_eq!(expected["expected_match"], true);
    let mismatch = debug::replay(&module, Some("sha256:00")).expect("replay");
    assert_eq!(mismatch["ok"], false);
    assert_eq!(mismatch["expected_match"], false);
}

#[test]
fn reduction_keeps_the_target_diagnostic_and_shrinks() {
    let src = "schema = { port = Int(max = 10) }\nport = 999\nunrelated = 1\n# comment\n";
    let (reduced, attempts) = debug::reduce(src, "T003", 100);
    assert!(attempts > 0);
    assert!(reduced.len() < src.len(), "reduced: {reduced:?}");
    assert!(
        reduced.contains("port = 999"),
        "target line kept: {reduced:?}"
    );
    assert!(!reduced.contains("unrelated"), "noise removed: {reduced:?}");
    let (_, diagnostics) = Engine::new()
        .evaluate(&parse(&reduced, "reduced").expect("reduced parses"))
        .expect("reduced evaluates");
    assert!(
        diagnostics.iter().any(|d| d.code == "T003"),
        "target survives: {diagnostics:?}"
    );
}

#[test]
fn minimal_case_reports_sizes_and_keeps_source_shape() {
    let src = "schema = { port = Int(max = 10) }\nport = 999\nunrelated = 1\n";
    let report = debug::minimal_case(src, "T003", 100);
    assert_eq!(report["target_code"], "T003");
    assert!(report["reduced_bytes"].as_u64().unwrap() < report["original_bytes"].as_u64().unwrap());
    assert!(report["reduced"].as_str().unwrap().contains("port = 999"));
}

#[test]
fn step_trace_redacts_locators_through_bindings_and_env() {
    use std::collections::HashMap;
    let mut env = HashMap::new();
    env.insert("SUPER_PASS".to_string(), "ENVSECRET-xyz789".to_string());
    let module = parse("p = env.SUPER_PASS\nk = secret(p)\n", "debug-env").expect("parse");
    let (_, _, steps) = Engine::hermetic()
        .with_env(env)
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("ENVSECRET"),
        "secret through a binding leaked: {rendered}"
    );
    assert!(
        steps.iter().any(|s| s.kind == "table" && s.tainted),
        "container step must be tainted: {steps:?}"
    );
}

#[test]
fn step_trace_redacts_literal_locator_behind_a_binding() {
    let module = parse("loc = \"hunter2-via-let\"\nk = secret(loc)\n", "debug-ref").expect("parse");
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(!rendered.contains("hunter2"), "{rendered}");
}

#[test]
fn taint_breakpoint_matches_a_tainted_blob() {
    let module = parse("f = blob(\"text/plain\", secret(\"x\"))\n", "debug-blob").expect("parse");
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let index = debug::breakpoint_index(&steps, "taint").expect("taint breakpoint");
    assert!(steps[index].tainted);
    assert!(
        steps.iter().any(|s| s.kind == "blob" && s.tainted),
        "tainted blob step missing: {steps:?}"
    );
}

#[test]
fn minimal_case_redacts_authored_secret_literals() {
    let src = "schema = { token = Int }\ntoken = secret(\"hunter2-min\")\nnoise = 1\n";
    let report = debug::minimal_case(src, "T002", 100);
    let rendered = report.to_string();
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert!(
        report["reduced"]
            .as_str()
            .unwrap()
            .contains("secret(\"***SECRET***\")"),
        "{}",
        report["reduced"]
    );
}

#[test]
fn minimal_case_redacts_every_secret_spelling_without_duplication() {
    let cases = [
        "schema = { token = Int }\ntoken = secret (\"hunter2-a\")\nnoise = 1\n",
        "schema = { token = Int }\ntoken = secret((\"hunter2-b\"))\nnoise = 1\n",
        "schema = { token = Int }\ntoken = secret(\"\"\"hunter2-c\"\"\")\nnoise = 1\n",
        "schema = { token = Int }\nlet p = \"hunter2-d\"\ntoken = secret(p)\nnoise = 1\n",
        "schema = { token = Int }\ntoken = secret(\"hunter2-e\nnoise = 1\n",
    ];
    for src in cases {
        let report = debug::minimal_case(src, "T002", 100);
        let reduced = report["reduced"].as_str().expect("reduced");
        assert!(!reduced.contains("hunter2"), "leak for {src:?}: {reduced}");
        assert_eq!(
            reduced.matches("schema = {").count(),
            1,
            "duplicated content for {src:?}: {reduced}"
        );
    }
}
