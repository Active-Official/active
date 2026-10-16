//! Dependency-graph queries (C-049, spec/graph.md).
//!
//! Pure functions over the IR DAG (`Module.graph`, edges
//! dependency → dependent): impact/why traversals, node labels,
//! and Mermaid/DOT/JSON renderings. Labels are names only — values
//! (and secrets) never appear in graph output.

use crate::ir::{Module, Node, NodeId};
use petgraph::Direction;
use std::collections::{HashMap, HashSet};

/// Resolve a user key like the evaluator does: table fields win
/// ties, single segments also see `let` bindings.
pub fn resolve_key(module: &Module, key: &str) -> Option<NodeId> {
    module.resolve_value_path(key)
}

/// Display label for a node: dotted path, binding name, or
/// `node{id}` for unmapped internals.
pub fn node_label(module: &Module, id: NodeId) -> String {
    let mut paths: HashMap<NodeId, String> = HashMap::new();
    for (name, binding) in &module.bindings {
        paths.insert(*binding, name.clone());
    }
    for (node, path) in module.value_paths() {
        paths.insert(node, path);
    }
    paths
        .get(&id)
        .cloned()
        .unwrap_or_else(|| format!("node{}", id.0))
}

/// Transitive dependents of a node (outgoing edges), excluding the
/// node itself, in first-visit order. Cycles terminate via the
/// visited set (queries never evaluate, so cyclic graphs answer).
pub fn transitive_dependents(module: &Module, id: NodeId) -> Vec<NodeId> {
    walk(module, id, Direction::Outgoing)
}

/// Transitive dependencies of a node (incoming edges), excluding
/// the node itself, in first-visit order.
pub fn transitive_dependencies(module: &Module, id: NodeId) -> Vec<NodeId> {
    walk(module, id, Direction::Incoming)
}

fn walk(module: &Module, start: NodeId, direction: Direction) -> Vec<NodeId> {
    let Some(&start_idx) = module.node_to_graph.get(&start) else {
        return Vec::new();
    };
    let mut seen = HashSet::from([start]);
    let mut out = Vec::new();
    let mut stack = vec![start_idx];
    while let Some(idx) = stack.pop() {
        // Petgraph neighbor order follows edge insertion, which is
        // parse order — deterministic for a given document. Callers
        // needing path order sort by label themselves.
        let mut neighbors: Vec<_> = module.graph.neighbors_directed(idx, direction).collect();
        neighbors.reverse();
        for neighbor in neighbors {
            let id = module.graph[neighbor];
            if seen.insert(id) {
                out.push(id);
                stack.push(neighbor);
            }
        }
    }
    out
}

/// Sorted dotted paths of a node's transitive dependents,
/// excluding the node itself and table nodes: a changed leaf
/// trivially rebuilds its parents, which would otherwise put every
/// table and the root in every answer. Patch and value nodes stay —
/// those are the consumers that matter.
pub fn impact_paths(module: &Module, id: NodeId) -> Vec<String> {
    let mut paths: Vec<String> = transitive_dependents(module, id)
        .into_iter()
        .filter(|node| !matches!(module.get(*node), Some(Node::Table { .. })))
        .map(|node| node_label(module, node))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Sorted dotted paths of a node's transitive dependencies.
pub fn why_paths(module: &Module, id: NodeId) -> Vec<String> {
    let mut paths: Vec<String> = transitive_dependencies(module, id)
        .into_iter()
        .map(|node| node_label(module, node))
        .collect();
    paths.sort();
    paths
}

/// Arena-stable node id for renderings (`n<index>`).
fn render_id(id: NodeId) -> String {
    format!("n{}", id.0)
}

fn sorted_nodes(module: &Module) -> Vec<(String, String)> {
    let mut nodes: Vec<(String, String)> = module
        .node_to_graph
        .keys()
        .map(|id| (render_id(*id), node_label(module, *id)))
        .collect();
    nodes.sort();
    nodes
}

fn sorted_edges(module: &Module) -> Vec<(String, String)> {
    let mut edges = Vec::new();
    for edge in module.graph.raw_edges() {
        let from = render_id(module.graph[edge.source()]);
        let to = render_id(module.graph[edge.target()]);
        edges.push((from, to));
    }
    edges.sort();
    edges
}

/// Mermaid flowchart of the full graph (deterministic).
pub fn to_mermaid(module: &Module) -> String {
    let mut out = String::from("flowchart TD\n");
    for (id, label) in sorted_nodes(module) {
        out.push_str(&format!("    {id}[\"{label}\"]\n"));
    }
    for (from, to) in sorted_edges(module) {
        out.push_str(&format!("    {from} --> {to}\n"));
    }
    out
}

/// DOT digraph of the full graph (deterministic; labels escaped).
pub fn to_dot(module: &Module) -> String {
    fn escape(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }
    let mut out = String::from("digraph solid {\n");
    for (id, label) in sorted_nodes(module) {
        out.push_str(&format!("    \"{id}\" [label=\"{}\"];\n", escape(&label)));
    }
    for (from, to) in sorted_edges(module) {
        out.push_str(&format!("    \"{from}\" -> \"{to}\";\n"));
    }
    out.push_str("}\n");
    out
}

/// JSON model of the full graph: nodes with labels, edges as
/// id pairs (deterministic).
pub fn to_graph_json(module: &Module) -> serde_json::Value {
    let nodes: Vec<serde_json::Value> = sorted_nodes(module)
        .into_iter()
        .map(|(id, label)| serde_json::json!({"id": id, "label": label}))
        .collect();
    let edges: Vec<serde_json::Value> = sorted_edges(module)
        .into_iter()
        .map(|(from, to)| serde_json::json!({"from": from, "to": to}))
        .collect();
    serde_json::json!({"nodes": nodes, "edges": edges})
}
