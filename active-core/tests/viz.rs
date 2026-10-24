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
    // A flat document has no containment edges, and the root (no path)
    // is never spelled as an empty-string parent.
    assert!(
        data["edges"]
            .as_array()
            .expect("edges")
            .iter()
            .all(|edge| edge["from"].as_str() != Some("")),
        "anonymous parent edge: {data}"
    );
}

#[test]
fn taint_edges_use_containment_for_array_elements() {
    let src = "a = [secret(\"x\"), { b = secret(\"y\") }]\n";
    let module = parse(src, "viz-edges").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let data = viz::taint_data(&module, &value);
    let edges: Vec<(String, String)> = data["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|edge| {
            (
                edge["from"].as_str().unwrap_or("").to_string(),
                edge["to"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert!(
        edges.contains(&("a".to_string(), "a[0]".to_string())),
        "array element edge: {edges:?}"
    );
    assert!(
        edges.contains(&("a".to_string(), "a[1]".to_string())),
        "nested table edge: {edges:?}"
    );
    assert!(
        edges.iter().all(|(from, _)| !from.is_empty()),
        "no anonymous parent edges: {edges:?}"
    );
}

#[test]
fn taint_data_includes_plain_locator_sources() {
    let src = "x = \"hunter2-loc\"\nmsg = \"prefix ${x}\"\nk = secret(x)\n";
    let module = parse(src, "viz-source").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let data = viz::taint_data(&module, &value);
    let sources: Vec<&str> = data["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(sources.contains(&"x"), "{sources:?}");
    assert!(sources.contains(&"msg"), "{sources:?}");
    assert!(sources.contains(&"k"), "{sources:?}");
    assert!(!data.to_string().contains("hunter2"), "{data}");
}

#[test]
fn taint_edges_never_dangle_to_unemitted_parents() {
    use std::collections::HashSet;
    let src = "let p = env.R13V\nk = secret(p)\n@patch(\"out.a\", p)\nout = { a = \"orig\" }\n";
    let module = parse(src, "viz-dangling").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("R13V".to_string(), "ENVSECRET-viz".to_string());
    let (value, _) = Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let data = viz::taint_data(&module, &value);
    let ids: HashSet<String> = data["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter_map(|node| node["id"].as_str().map(str::to_string))
        .collect();
    for edge in data["edges"].as_array().expect("edges") {
        let from = edge["from"].as_str().unwrap_or("");
        let to = edge["to"].as_str().unwrap_or("");
        assert!(ids.contains(from), "dangling from '{from}': {data}");
        assert!(ids.contains(to), "dangling to '{to}': {data}");
    }
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
    assert!(
        data.get("repaired_source").is_none(),
        "display plan must not carry source: {data}"
    );
    let secret_source = "schema = { token = Int }\ntoken = secret(\"hunter2-viz-repair\")\n";
    let secret_data = viz::repair_data(
        secret_source,
        active_core::diag::RepairGoal::Validate,
        &[],
        8,
    );
    assert!(!secret_data.to_string().contains("hunter2"));
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
