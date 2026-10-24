//! Visualizer data exporters (TOOLING §6): taint flow, budget
//! dashboard, schema view, and repair plan. Every emitter returns
//! redacted data only — labels are paths and kinds, values render
//! through the canonical redactor.

use serde_json::{json, Value as JsonValue};

use crate::ir::{Module, Value};
use crate::Budget;

/// Taint flow: secret sources, the containment edges that carry taint,
/// and the paths that would export it (all redacted on emission).
pub fn taint_data(module: &Module, value: &Value) -> JsonValue {
    let mut nodes: Vec<JsonValue> = Vec::new();
    let mut edges: Vec<JsonValue> = Vec::new();
    let mut sources: Vec<String> = Vec::new();
    let mut export_points: Vec<String> = Vec::new();
    walk_taint(value, "", None, &mut |path, node_value, parent| {
        if contains_secret(node_value) {
            let leaf = matches!(node_value, Value::Secret(_))
                || matches!(node_value, Value::Blob { tainted: true, .. });
            nodes.push(json!({
                "id": path,
                "label": if path.is_empty() { "(root)" } else { path },
                "kind": kind_name(node_value),
                "secret": leaf,
            }));
            if leaf {
                sources.push(path.to_string());
                export_points.push(path.to_string());
            }
            if let Some(parent) = parent {
                edges.push(json!({"from": parent, "to": path}));
            }
        }
    });
    let _ = module;
    json!({
        "version": 1,
        "nodes": nodes,
        "edges": edges,
        "sources": sources,
        "export_points": export_points,
    })
}

/// Budget dashboard: every limit, the static usage proxy the evaluator
/// can observe without a second walk, headroom, and the progressive
/// warnings recorded for this run (W013).
pub fn budget_data(
    module: &Module,
    value: &Value,
    budget: &Budget,
    diagnostics: &[crate::Diagnostic],
) -> JsonValue {
    let nodes = module.arena.len() as u64;
    let mut keys = 0u64;
    let mut max_array = 0u64;
    let mut depth = 0u64;
    let mut max_value_size = 0u64;
    // One post-order pass: each node's estimated serialized size is
    // computed from its children, so a large document stays O(n)
    // instead of re-serializing every subtree.
    let total_size = shape_size(
        value,
        1,
        &mut keys,
        &mut max_array,
        &mut depth,
        &mut max_value_size,
    );
    let headroom = |used: u64, limit: u64| {
        if limit == 0 {
            JsonValue::Null
        } else {
            json!(limit.saturating_sub(used))
        }
    };
    let warnings: Vec<&str> = diagnostics
        .iter()
        .filter(|d| d.code == "W013")
        .map(|d| d.code.as_str())
        .collect();
    json!({
        "version": 1,
        "mode": match module.mode {
            crate::ir::Mode::Data => "data",
            crate::ir::Mode::Config => "config",
        },
        "limits": {
            "nodes": budget.max_nodes,
            "depth": budget.max_depth,
            "array_len": budget.max_array_len,
            "keys": budget.max_keys,
            "wall_ms": budget.max_wall_ms,
            "value_size": budget.max_value_size,
            "total_size": budget.max_total_value_size,
        },
        "usage": {
            "nodes": nodes,
            "depth": depth,
            "array_len": max_array,
            "keys": keys,
            "value_size": max_value_size,
            "total_size": total_size,
        },
        "headroom": {
            "nodes": headroom(nodes, budget.max_nodes),
            "depth": headroom(depth, budget.max_depth),
            "array_len": headroom(max_array, budget.max_array_len),
            "keys": headroom(keys, budget.max_keys),
            "value_size": headroom(max_value_size, budget.max_value_size),
            "total_size": headroom(total_size, budget.max_total_value_size),
        },
        "warnings": warnings,
    })
}

/// Schema view: the versioned introspection document plus mode.
pub fn schema_data(module: &Module) -> Result<JsonValue, String> {
    let mut inspected = crate::convert::schema_inspect(module)?;
    inspected["mode"] = json!(match module.mode {
        crate::ir::Mode::Data => "data",
        crate::ir::Mode::Config => "config",
    });
    Ok(inspected)
}

/// Repair plan view: the shipped multi-step planner's outcome without
/// `repaired_source` (source text can contain authored secrets).
pub fn repair_data(
    source: &str,
    goal: crate::diag::RepairGoal,
    preserve: &[String],
    max_steps: usize,
) -> JsonValue {
    let outcome = crate::convert::repair_document(source, goal, preserve, max_steps);
    let mut data =
        serde_json::to_value(outcome).unwrap_or_else(|_| json!({"error": "unserializable plan"}));
    if let Some(object) = data.as_object_mut() {
        object.remove("repaired_source");
    }
    data
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn contains_secret(value: &Value) -> bool {
    match value {
        Value::Secret(_) => true,
        Value::Blob { tainted, .. } => *tainted,
        Value::Array(items) => items.iter().any(contains_secret),
        Value::Table(map) => map.values().any(contains_secret),
        _ => false,
    }
}

fn kind_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "Null",
        Value::Bool(_) => "Bool",
        Value::Int(_) => "Int",
        Value::Float(_) => "Float",
        Value::String(_) => "String",
        Value::DurationMs(_) => "Duration",
        Value::Quantity(_, _) => "Quantity",
        Value::Array(_) => "Array",
        Value::Table(_) => "Table",
        Value::Secret(_) => "Secret",
        Value::Blob { .. } => "Blob",
    }
}

/// Depth-first walk carrying the dotted path; `visit` receives the node
/// and its parent path. The parent is the container we descended from,
/// never re-derived from the child path: `a[0]` is contained in `a`.
fn walk_taint<'a>(
    value: &'a Value,
    path: &str,
    parent: Option<&str>,
    visit: &mut dyn FnMut(&str, &'a Value, Option<&str>),
) {
    visit(path, value, parent);
    // The document root has no path, so its children have no parent
    // edge to a named node.
    let child_parent = if path.is_empty() { None } else { Some(path) };
    match value {
        Value::Table(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                walk_taint(child, &child_path, child_parent, visit);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let child_path = format!("{path}[{index}]");
                walk_taint(child, &child_path, child_parent, visit);
            }
        }
        _ => {}
    }
}

/// Post-order size/shape walk. Returns the estimated serialized byte
/// size of this subtree (keys, separators, and scalar widths), while
/// recording global maxima. O(nodes) with saturating arithmetic.
fn shape_size(
    value: &Value,
    level: u64,
    keys: &mut u64,
    max_array: &mut u64,
    depth: &mut u64,
    max_value_size: &mut u64,
) -> u64 {
    *depth = (*depth).max(level);
    let size = match value {
        Value::Null => 4,
        Value::Bool(_) => 5,
        Value::Int(_) | Value::Float(_) | Value::DurationMs(_) => 8,
        Value::String(text) => text.len() as u64 + 2,
        Value::Quantity(_, _) => 16,
        Value::Secret(_) => "***SECRET***".len() as u64 + 2,
        Value::Blob { .. } => 96,
        Value::Array(items) => {
            *max_array = (*max_array).max(items.len() as u64);
            let mut total = 2u64;
            for child in items {
                total = total.saturating_add(
                    shape_size(child, level + 1, keys, max_array, depth, max_value_size)
                        .saturating_add(2),
                );
            }
            total
        }
        Value::Table(map) => {
            *keys = keys.saturating_add(map.len() as u64);
            let mut total = 2u64;
            for (key, child) in map {
                total = total
                    .saturating_add(key.len() as u64 + 4)
                    .saturating_add(shape_size(
                        child,
                        level + 1,
                        keys,
                        max_array,
                        depth,
                        max_value_size,
                    ))
                    .saturating_add(1);
            }
            total
        }
    };
    *max_value_size = (*max_value_size).max(size);
    size
}
