//! Dependency-graph queries (C-049, spec/graph.md).
//!
//! Pure functions over the IR DAG (`Module.graph`, edges
//! dependency → dependent): impact/why traversals, node labels,
//! and Mermaid/DOT/JSON renderings. Labels are names only — values
//! (and secrets) never appear in graph output.

use crate::ir::{Module, Node, NodeId};
use petgraph::Direction;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// Resolve a user key like the evaluator does: table fields win
/// ties, single segments also see `let` bindings.
pub fn resolve_key(module: &Module, key: &str) -> Option<NodeId> {
    module.resolve_value_path(key)
}

/// Display label for a node: dotted path, binding name, or
/// `node{id}` for unmapped internals.
/// Stable label for a node (`node{id}` for unmapped internals, as
/// spec/graph.md states).
pub fn node_label(module: &Module, id: NodeId) -> String {
    label_or_id(&label_map(module), id)
}

/// One label map for a whole query. `value_paths()` is O(V); building it
/// per dependent made impact assembly quadratic.
fn label_map(module: &Module) -> HashMap<NodeId, String> {
    let mut paths: HashMap<NodeId, String> = HashMap::new();
    for (name, binding) in &module.bindings {
        paths.insert(*binding, name.clone());
    }
    for (node, path) in module.value_paths() {
        paths.insert(node, path);
    }
    paths
}

fn label_or_id(paths: &HashMap<NodeId, String>, id: NodeId) -> String {
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

/// Result of an impact walk (LANG-G-016): the consumer paths plus whether
/// any dependent table was excluded by design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Impact {
    pub paths: Vec<String>,
    /// True when at least one dependent was a table node and therefore
    /// filtered: an empty `paths` then means "only containers depend on
    /// it", never "nothing depends on it".
    pub table_dependents_excluded: bool,
}

/// Sorted dotted paths of a node's transitive dependents, excluding the
/// node itself and table nodes: a changed leaf trivially rebuilds its
/// parents, which would otherwise put every table and the root in every
/// answer. Patch and value nodes stay — those are the consumers that
/// matter. The exclusion is recorded so callers can say why an empty
/// answer is empty.
pub fn impact_paths_detailed(module: &Module, id: NodeId) -> Impact {
    let mut paths: Vec<String> = Vec::new();
    let mut table_dependents_excluded = false;
    let labels = label_map(module);
    for node in transitive_dependents(module, id) {
        if matches!(module.get(node), Some(Node::Table { .. })) {
            table_dependents_excluded = true;
        } else {
            paths.push(label_or_id(&labels, node));
        }
    }
    paths.sort();
    paths.dedup();
    Impact {
        paths,
        table_dependents_excluded,
    }
}

/// Sorted dotted paths of a node's transitive dependents (see
/// `impact_paths_detailed` for the exclusion record).
pub fn impact_paths(module: &Module, id: NodeId) -> Vec<String> {
    impact_paths_detailed(module, id).paths
}

/// Sorted dotted paths of a node's transitive dependencies.
pub fn why_paths(module: &Module, id: NodeId) -> Vec<String> {
    let labels = label_map(module);
    let mut paths: Vec<String> = transitive_dependencies(module, id)
        .into_iter()
        .map(|node| label_or_id(&labels, node))
        .collect();
    paths.sort();
    paths
}

/// Shortest directed path between two nodes (BFS over the dependency
/// DAG, edges dependency → dependent). Returns node labels from
/// source to target, or None when unreachable. Deterministic:
/// neighbors visit in parse order. Used by `graph query
/// shortest-path` for agent impact scoping.
pub fn shortest_path(module: &Module, from: NodeId, to: NodeId) -> Option<Vec<String>> {
    use std::collections::VecDeque;
    let (&start, &goal) = (
        module.node_to_graph.get(&from)?,
        module.node_to_graph.get(&to)?,
    );
    let mut prev: HashMap<petgraph::graph::NodeIndex, petgraph::graph::NodeIndex> = HashMap::new();
    let mut seen = HashSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some(idx) = queue.pop_front() {
        if idx == goal {
            let mut rev = vec![goal];
            while let Some(p) = prev.get(rev.last().expect("non-empty")) {
                rev.push(*p);
            }
            rev.reverse();
            return Some(
                rev.into_iter()
                    .map(|n| node_label(module, module.graph[n]))
                    .collect(),
            );
        }
        let mut neighbors: Vec<_> = module
            .graph
            .neighbors_directed(idx, Direction::Outgoing)
            .collect();
        neighbors.sort_by_key(|n| n.index());
        for neighbor in neighbors {
            if seen.insert(neighbor) {
                prev.insert(neighbor, idx);
                queue.push_back(neighbor);
            }
        }
    }
    None
}

/// Dotted paths of the secret-tainted cone in an evaluated value:
/// every path whose subtree contains secret material or a tainted
/// blob, including all ancestors (taint flows outward through
/// containers). Labels only — values never enter the output
/// (H-001 class guarantee for graph queries). Operates on the
/// evaluated tree, so it reflects actual data flow rather than
/// static edges.
///
/// Structural only: a copy of a runtime secret and a payload-resident
/// secret are invisible without evaluator seeds and retained payloads.
/// Prefer [`secret_tainted_paths_with_seeds`] when the document was
/// evaluated.
pub fn secret_tainted_paths(value: &crate::ir::Value) -> Vec<String> {
    fn walk(value: &crate::ir::Value, path: String, out: &mut Vec<String>) -> bool {
        use crate::ir::Value;
        let tainted = match value {
            Value::Secret(_) => true,
            Value::Blob { tainted: true, .. } => true,
            Value::Blob { .. } => false,
            Value::Array(items) => {
                let mut any = false;
                for (i, item) in items.iter().enumerate() {
                    let child = if path.is_empty() {
                        format!("[{i}]")
                    } else {
                        format!("{path}[{i}]")
                    };
                    any |= walk(item, child, out);
                }
                any
            }
            Value::Table(map) => {
                let mut any = false;
                for (key, val) in map {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    any |= walk(val, child, out);
                }
                any
            }
            _ => false,
        };
        if tainted && !path.is_empty() {
            out.push(path);
        }
        tainted
    }
    let mut out = Vec::new();
    walk(value, String::new(), &mut out);
    out.sort();
    out.dedup();
    out
}

/// Structural variant of [`secret_tainted_paths`]: a plain value used
/// as a `secret(...)` locator (and anything built from it) is material
/// even without a `Value::Secret` marker. The same profile backs
/// `inspect` and step traces, so the query agrees with them.
///
/// Structural only: a runtime copy or a payload-resident secret is
/// invisible. Callers that evaluated the document use
/// [`secret_tainted_paths_with_seeds`].
pub fn secret_tainted_paths_with(
    module: &Module,
    value: &crate::ir::Value,
    override_profile: Option<&str>,
) -> Vec<String> {
    let profile = crate::secret::SecretProfile::analyze_with_profile(module, override_profile);
    secret_tainted_paths_from_profile(value, &profile)
}

/// Structural taint plus the evaluator's runtime secrets: a copy of a
/// `let`-bound or computed secret is reported even though no path makes
/// it structurally material. `env` and `seeds` are the snapshot and
/// seeds that produced `value`; retained payloads are inspected too.
pub fn secret_tainted_paths_with_seeds(
    module: &Module,
    value: &crate::ir::Value,
    override_profile: Option<&str>,
    env: Option<&std::collections::HashMap<String, String>>,
    seeds: Option<&crate::secret::SecretRedactor>,
) -> Vec<String> {
    let profile = crate::secret::SecretProfile::with_context(module, override_profile, env, seeds);
    secret_tainted_paths_from_profile(value, &profile)
}

fn secret_tainted_paths_from_profile(
    value: &crate::ir::Value,
    profile: &crate::secret::SecretProfile,
) -> Vec<String> {
    profile.begin_redaction_pass();
    fn walk(
        value: &crate::ir::Value,
        path: String,
        profile: &crate::secret::SecretProfile,
        out: &mut Vec<String>,
        payload_cache: &mut std::collections::HashMap<[u8; 32], bool>,
    ) -> bool {
        use crate::ir::Value;
        let structural = profile.path_is_secret(&path);
        let tainted = match value {
            Value::Secret(_) => true,
            Value::Blob { tainted: true, .. } => true,
            // A payload-resident secret is invisible without the
            // retained bytes; callers that need it evaluate with
            // materialisation and these seeds.
            Value::Blob {
                media_type,
                payload,
                ..
            } => {
                let payload_material = payload.as_ref().is_some_and(|bytes| {
                    // Keyed by the scanned bytes, never the claimed hash:
                    // two payloads can share a forged hash.
                    let key = crate::convert::payload_scan_key(bytes);
                    if let Some(known) = payload_cache.get(&key) {
                        return *known;
                    }
                    let material = crate::convert::blob_payload_is_material(
                        profile.literals(),
                        profile,
                        Some(bytes),
                    );
                    payload_cache.insert(key, material);
                    material
                });
                structural || profile.blob_identity_is_material(media_type) || payload_material
            }
            // A copy of a runtime secret has no structural path taint;
            // its text is the only signal.
            Value::String(text) => structural || profile.redact_text(text).is_some(),
            Value::Array(items) => {
                let mut any = structural;
                for (i, item) in items.iter().enumerate() {
                    let child = if path.is_empty() {
                        format!("[{i}]")
                    } else {
                        format!("{path}[{i}]")
                    };
                    any |= walk(item, child, profile, out, payload_cache);
                }
                any
            }
            Value::Table(map) => {
                let mut any = structural;
                for (key, val) in map {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    any |= walk(val, child, profile, out, payload_cache);
                }
                any
            }
            _ => structural,
        };
        if tainted && !path.is_empty() {
            // Paths are display text built from authored keys; each key
            // segment is redacted like any other whole-value copy (a
            // short material key matches exactly at its segment).
            out.push(profile.redact_path(&path));
        }
        tainted
    }
    let mut out = Vec::new();
    let mut payload_cache = std::collections::HashMap::new();
    walk(value, String::new(), profile, &mut out, &mut payload_cache);
    out.sort();
    out.dedup();
    out
}

/// Blobs at or above a size floor in an evaluated value: `(path,
/// size, blake3-hex)` triples. Identity metadata only — payloads
/// never enter the output. Raw: callers that hold a `SecretProfile`
/// must use [`blobs_above_size_with`] so an identity the export hides
/// (path materiality, a material media type, a payload-resident
/// secret) is reported as the synthetic redacted digest.
pub fn blobs_above_size(value: &crate::ir::Value, min_size: u64) -> Vec<(String, u64, String)> {
    fn walk(
        value: &crate::ir::Value,
        path: String,
        min_size: u64,
        out: &mut Vec<(String, u64, String)>,
    ) {
        use crate::ir::Value;
        match value {
            Value::Blob {
                hash,
                size,
                tainted,
                media_type,
                payload,
            } if *size >= min_size => {
                let identity =
                    crate::blob::verified_identity(hash, *tainted, media_type, payload.as_ref());
                out.push((path, *size, crate::blob::hash_hex(&identity)));
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    let child = if path.is_empty() {
                        format!("[{i}]")
                    } else {
                        format!("{path}[{i}]")
                    };
                    walk(item, child, min_size, out);
                }
            }
            Value::Table(map) => {
                for (key, val) in map {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(val, child, min_size, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, String::new(), min_size, &mut out);
    out.sort();
    out
}

/// Profile-aware `blobs_above_size`: an identity the export would redact
/// (a material media type, a payload-resident secret, or a material
/// serving path such as a profile override) is reported with
/// [`crate::blob::redacted_identity`] instead of its real digest, so this
/// query cannot publish a hash the export hides. A clean blob and an
/// evaluated tainted blob keep their real identity (the latter is already
/// a salted v2 digest).
pub fn blobs_above_size_with(
    value: &crate::ir::Value,
    min_size: u64,
    profile: &crate::secret::SecretProfile,
) -> Vec<(String, u64, String)> {
    fn walk(
        value: &crate::ir::Value,
        path: String,
        min_size: u64,
        profile: &crate::secret::SecretProfile,
        out: &mut Vec<(String, u64, String)>,
    ) {
        use crate::ir::Value;
        match value {
            Value::Blob {
                hash,
                size,
                tainted,
                media_type,
                payload,
            } if *size >= min_size => {
                let identity_material = profile.blob_identity_is_material(media_type)
                    || payload.as_ref().is_some_and(|bytes| {
                        crate::convert::blob_payload_is_material(
                            profile.literals(),
                            profile,
                            Some(bytes),
                        )
                    })
                    || (!*tainted && profile.path_is_secret(&path));
                let identity = if identity_material {
                    crate::blob::redacted_identity()
                } else {
                    crate::blob::verified_identity(hash, *tainted, media_type, payload.as_ref())
                };
                let shown = profile.redact_path(&path);
                out.push((shown, *size, crate::blob::hash_hex(&identity)));
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    let child = if path.is_empty() {
                        format!("[{i}]")
                    } else {
                        format!("{path}[{i}]")
                    };
                    walk(item, child, min_size, profile, out);
                }
            }
            Value::Table(map) => {
                for (key, val) in map {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(val, child, min_size, profile, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, String::new(), min_size, profile, &mut out);
    out.sort();
    out
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
        // Write into the buffer: no throwaway String per node (LANG-K-007).
        let _ = writeln!(out, "    {id}[\"{label}\"]");
    }
    for (from, to) in sorted_edges(module) {
        let _ = writeln!(out, "    {from} --> {to}");
    }
    out
}

/// DOT digraph of the full graph (deterministic; labels escaped).
pub fn to_dot(module: &Module) -> String {
    /// Escaping matches `str::replace('\\',"\\\\").replace('"',"\\\"")`
    /// exactly, without the intermediate allocation per label.
    fn write_escaped(out: &mut String, label: &str) {
        for ch in label.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                _ => out.push(ch),
            }
        }
    }
    let mut out = String::from("digraph active {\n");
    for (id, label) in sorted_nodes(module) {
        let _ = write!(out, "    \"{id}\" [label=\"");
        write_escaped(&mut out, &label);
        out.push_str("\"];\n");
    }
    for (from, to) in sorted_edges(module) {
        let _ = writeln!(out, "    \"{from}\" -> \"{to}\";");
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
