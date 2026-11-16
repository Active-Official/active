//! Dependency-graph query tests (C-049, spec/graph.md).
//!
//! Impact/why traversals on chains, diamonds, and isolated keys;
//! unknown-key errors; format snapshots and re-parse validity;
//! byte determinism across runs.

use active_core::{graph, parse};

const GRAPH_DOC: &str = "let base = 1\n[a]\nx = base\ny = x\nw = x\n[b]\nq = 1\nsolo = 1\n";

fn module() -> active_core::Module {
    parse(GRAPH_DOC, "graph").expect("must parse")
}

#[test]
fn impact_follows_transitive_dependents() {
    let module = module();
    let base = graph::resolve_key(&module, "base").expect("base");
    let paths = graph::impact_paths(&module, base);
    // Exact: containers (tables, root) are excluded by design.
    assert_eq!(paths, ["a.w", "a.x", "a.y"]);
    let x = graph::resolve_key(&module, "a.x").expect("a.x");
    assert_eq!(graph::impact_paths(&module, x), ["a.w", "a.y"]);
}

#[test]
fn an_empty_impact_records_the_container_exclusion() {
    // LANG-G-016: `paths` stays the consumer list, but the walk records
    // that tables were excluded, so an empty answer is distinguishable
    // from "nothing depends on this".
    let module = module();
    let solo = graph::resolve_key(&module, "b.solo").expect("b.solo");
    let impact = graph::impact_paths_detailed(&module, solo);
    assert!(impact.paths.is_empty());
    assert!(impact.table_dependents_excluded);
    let base = graph::resolve_key(&module, "base").expect("base");
    let impact = graph::impact_paths_detailed(&module, base);
    assert_eq!(impact.paths, ["a.w", "a.x", "a.y"]);
    assert!(impact.table_dependents_excluded);
    assert_eq!(graph::impact_paths(&module, base), impact.paths);
}

#[test]
fn why_follows_transitive_dependencies() {
    let module = module();
    let y = graph::resolve_key(&module, "a.y").expect("a.y");
    assert_eq!(graph::why_paths(&module, y), ["a.x", "base"]);
}

#[test]
fn isolated_keys_answer_empty_and_unknown_keys_resolve_none() {
    let module = module();
    let solo = graph::resolve_key(&module, "b.solo").expect("b.solo");
    assert!(graph::impact_paths(&module, solo).is_empty());
    assert!(graph::why_paths(&module, solo).is_empty());
    assert!(graph::resolve_key(&module, "nowhere").is_none());
    assert!(graph::resolve_key(&module, "a.nowhere").is_none());
}

#[test]
fn renderings_are_deterministic_and_valid() {
    let module = module();
    let mermaid = graph::to_mermaid(&module);
    assert!(mermaid.starts_with("flowchart TD\n"), "{mermaid}");
    assert!(mermaid.contains("-->"), "{mermaid}");
    let dot = graph::to_dot(&module);
    assert!(dot.starts_with("digraph active {"), "{dot}");
    assert!(dot.contains("->"), "{dot}");
    let json = graph::to_graph_json(&module);
    assert!(json["nodes"].as_array().expect("nodes").len() > 3);
    assert!(!json["edges"].as_array().expect("edges").is_empty());
    // Same input, same bytes — twice.
    assert_eq!(mermaid, graph::to_mermaid(&module));
    assert_eq!(dot, graph::to_dot(&module));
    assert_eq!(json, graph::to_graph_json(&module));
}

#[test]
fn graph_output_never_carries_values() {
    // Labels are names only: a secret value flowing through the
    // graph must not appear in any rendering.
    let module = parse(
        "token = secret(\"s3cr3t-graph\")\nurl = \"https://x\"\n",
        "graph",
    )
    .expect("must parse");
    let flat = format!(
        "{}{}{}",
        graph::to_mermaid(&module),
        graph::to_dot(&module),
        graph::to_graph_json(&module)
    );
    assert!(!flat.contains("s3cr3t-graph"), "{flat}");
    assert!(flat.contains("token"), "{flat}");
}

#[test]
fn node_labels_prefer_paths_then_bindings() {
    let module = module();
    let base = graph::resolve_key(&module, "base").expect("base");
    assert_eq!(graph::node_label(&module, base), "base");
    let x = graph::resolve_key(&module, "a.x").expect("a.x");
    assert_eq!(graph::node_label(&module, x), "a.x");
}

#[test]
fn renderings_are_byte_identical_to_the_reference_format() {
    // LANG-K-007: the exporters now write into the buffer instead of
    // allocating a String per line. The bytes must not change, so this
    // rebuilds the documented format from the same sorted node/edge lists
    // and compares.
    let module = module();
    let json = graph::to_graph_json(&module);
    let nodes: Vec<(String, String)> = json["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|n| {
            (
                n["id"].as_str().expect("id").to_string(),
                n["label"].as_str().expect("label").to_string(),
            )
        })
        .collect();
    let edges: Vec<(String, String)> = json["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|e| {
            (
                e["from"].as_str().expect("from").to_string(),
                e["to"].as_str().expect("to").to_string(),
            )
        })
        .collect();
    let mut mermaid = String::from("flowchart TD\n");
    for (id, label) in &nodes {
        mermaid.push_str(&format!("    {id}[\"{label}\"]\n"));
    }
    for (from, to) in &edges {
        mermaid.push_str(&format!("    {from} --> {to}\n"));
    }
    assert_eq!(graph::to_mermaid(&module), mermaid);

    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut dot = String::from("digraph active {\n");
    for (id, label) in &nodes {
        dot.push_str(&format!("    \"{id}\" [label=\"{}\"];\n", escape(label)));
    }
    for (from, to) in &edges {
        dot.push_str(&format!("    \"{from}\" -> \"{to}\";\n"));
    }
    dot.push_str("}\n");
    assert_eq!(graph::to_dot(&module), dot);
}
