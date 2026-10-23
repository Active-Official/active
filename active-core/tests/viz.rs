//! Visualizer data tests (TOOLING VIZ.008): stable JSON shapes and
//! redaction on every emitter.

use active_core::viz;
use active_core::{parse, Budget, Engine};

fn fixture() -> (
    active_core::Module,
    active_core::Value,
    Vec<active_core::Diagnostic>,
) {
    let src = "schema = { port = Int(min = 1), token = String }\nport = 8080\ntoken = secret(\"hunter2-viz\")\n";
    let module = parse(src, "viz").expect("parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("evaluate");
    (module, value, diags)
}

#[test]
fn taint_data_lists_sources_without_values() {
    let (module, value, _) = fixture();
    let data = viz::taint_data(&module, &value);
    let sources: Vec<&str> = data["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(sources.contains(&"token"), "{sources:?}");
    let rendered = serde_json::to_string(&data).expect("json");
    assert!(!rendered.contains("hunter2"), "raw secret in taint data");
    assert!(!data["edges"].as_array().expect("edges").is_empty());
}

#[test]
fn budget_data_reports_limits_usage_and_headroom() {
    let (module, value, diags) = fixture();
    let budget = Budget::strict();
    let data = viz::budget_data(&module, &value, &budget, &diags);
    assert_eq!(data["limits"]["nodes"], budget.max_nodes);
    assert_eq!(data["limits"]["depth"], budget.max_depth);
    assert_eq!(data["usage"]["nodes"], module.arena.len());
    let used = data["usage"]["nodes"].as_u64().expect("nodes");
    let headroom = data["headroom"]["nodes"].as_u64().expect("headroom");
    assert_eq!(headroom, budget.max_nodes - used);
    assert!(data["usage"]["depth"].as_u64().unwrap() >= 2);
    assert!(data["warnings"].as_array().expect("warnings").is_empty());
}

#[test]
fn schema_data_matches_introspection() {
    let (module, _, _) = fixture();
    let data = viz::schema_data(&module).expect("schema");
    assert_eq!(data["version"], 2);
    let fields = data["fields"].as_array().expect("fields");
    assert!(fields.iter().any(|f| f["path"] == "port"));
    assert_eq!(data["mode"], "config");
}

#[test]
fn repair_data_exposes_the_plan() {
    let source = "schema = { port = Int(default = 8080) }\n";
    let data = viz::repair_data(source, active_core::diag::RepairGoal::Validate, &[], 8);
    let steps = data["steps"].as_array().expect("steps");
    assert!(!steps.is_empty(), "{data}");
    assert_eq!(steps[0]["kind"], "fill");
    assert!(data["repaired_source"]
        .as_str()
        .expect("source")
        .contains("@patch"));
}

#[test]
fn taint_data_surfaces_secret_bearing_blobs() {
    let src = "f = blob(\"text/plain\", secret(\"hunter2-taint\"))\n";
    let module = parse(src, "viz-blob").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let data = viz::taint_data(&module, &value);
    let sources: Vec<&str> = data["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(sources.contains(&"f"), "{sources:?}");
    assert!(!data.to_string().contains("hunter2"));
}
