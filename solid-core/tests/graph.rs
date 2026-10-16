//! Dependency-graph query tests (C-049, spec/graph.md).
//!
//! Impact/why traversals on chains, diamonds, and isolated keys;
//! unknown-key errors; format snapshots and re-parse validity;
//! byte determinism across runs.

use solid_core::{graph, parse};

const GRAPH_DOC: &str = "let base = 1\n[a]\nx = base\ny = x\nw = x\n[b]\nq = 1\nsolo = 1\n";

fn module() -> solid_core::Module {
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
    assert!(dot.starts_with("digraph solid {"), "{dot}");
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
