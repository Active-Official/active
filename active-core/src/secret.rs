//! Structural secret-taint analysis (TOOLING DEBUG.001, INSPECT.001-005,
//! spec/debug.md §5, spec/lsp.md §2).
//!
//! The evaluator marks `secret(...)` results with `Value::Secret`, but
//! material also flows through plain nodes: a binding used as a locator,
//! an environment lookup feeding one, an interpolation embedding either.
//! Consumers that publish values (step traces, `inspect`, hover) must
//! recognize those flows structurally, because the runtime value carries
//! no marker. This module computes, from the module alone:
//!
//! * every node reachable from a `secret(...)` locator, plus every node
//!   derived from one (an ancestor container, an interpolation part, a
//!   reference);
//! * every literal string inside that set, so renderings can redact
//!   *substrings* (an interpolated prefix/suffix hides the literal);
//! * value paths for the document tree, including array elements, so a
//!   caller can ask whether one path is secret material.
//!
//! All redaction is deterministic: known secrets are applied
//! longest-first, then lexicographically.

use std::collections::{HashMap, HashSet};

use serde_json::Value as JsonValue;

use crate::ir::{InterpPart, Module, Node, NodeId, PatchOp, Value};

/// Replacement text for redacted material. Shared with the evaluator's
/// JSON mapping so every surface emits the same marker.
pub const MARKER: &str = "***SECRET***";

/// Direct value-producing children of a node. Schema nodes are
/// excluded: they describe contracts and never evaluate to document
/// data, so following them would taint unrelated values.
pub fn node_children(node: &Node) -> Vec<NodeId> {
    match node {
        Node::Literal(_) | Node::Ident(_) | Node::PatchDirective { .. } => Vec::new(),
        Node::Env { default, .. } => default.iter().copied().collect(),
        Node::Secret { locator } => vec![*locator],
        Node::ContentAddressed { media_type, data } => vec![*media_type, *data],
        Node::Ref(target) => vec![*target],
        Node::Interpolate { parts } => parts
            .iter()
            .filter_map(|part| match part {
                InterpPart::Expr(id) => Some(*id),
                InterpPart::Literal(_) => None,
            })
            .collect(),
        Node::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
        Node::Unary { expr, .. } => vec![*expr],
        Node::Array { elements } => elements.clone(),
        Node::Table { fields } => fields.values().copied().collect(),
        Node::Quantity { value, .. } => vec![*value],
        Node::ApplyConstraint { target, .. } => vec![*target],
        Node::SchemaField { .. } | Node::SchemaTable { .. } | Node::SchemaArray { .. } => {
            Vec::new()
        }
        Node::Semantic { inner, .. } => vec![*inner],
    }
}

/// Direct value children by module lookup (`NodeId`s dereference
/// through `Module::get`).
pub fn value_children(module: &Module, id: NodeId) -> Vec<NodeId> {
    module.get(id).map(node_children).unwrap_or_default()
}

/// Insert `root` and every value node reachable from it into `set`.
pub fn secret_subtree(module: &Module, root: NodeId, set: &mut HashSet<NodeId>) {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !set.insert(id) {
            continue;
        }
        stack.extend(value_children(module, id));
    }
}

/// A deterministic set of secret strings. Insertion keeps the list
/// sorted longest-first so overlapping secrets redact consistently.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretRedactor {
    values: Vec<String>,
}

impl SecretRedactor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, text: impl Into<String>) {
        let text = text.into();
        // Empty strings would match every position; they are not
        // material.
        if text.is_empty() || self.values.iter().any(|known| known == &text) {
            return;
        }
        self.values.push(text);
        self.values
            .sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn values(&self) -> &[String] {
        &self.values
    }

    pub fn merge(&mut self, other: &SecretRedactor) {
        for value in &other.values {
            self.insert(value.clone());
        }
    }

    /// Replace every known secret substring. Returns the redacted text,
    /// or None when nothing matched (so callers can keep their fast
    /// path and report taint honestly).
    ///
    /// Existing markers are opaque: the text is split on [`MARKER`] and
    /// only the non-marker segments are scanned. A secret containing
    /// the marker itself cannot be expressed without ambiguity; the
    /// marker wins, deterministically.
    pub fn redact_text(&self, text: &str) -> Option<String> {
        if self.values.is_empty() || text == MARKER {
            return None;
        }
        let mut out = String::with_capacity(text.len());
        let mut hit = false;
        for (index, segment) in text.split(MARKER).enumerate() {
            if index > 0 {
                out.push_str(MARKER);
            }
            let mut cursor = 0usize;
            while cursor < segment.len() {
                let rest = &segment[cursor..];
                match self
                    .values
                    .iter()
                    .find(|secret| rest.starts_with(secret.as_str()))
                {
                    Some(secret) => {
                        out.push_str(MARKER);
                        cursor += secret.len();
                        hit = true;
                    }
                    None => {
                        let ch = rest.chars().next().expect("non-empty remainder");
                        out.push(ch);
                        cursor += ch.len_utf8();
                    }
                }
            }
        }
        hit.then_some(out)
    }

    /// Replace secret substrings inside every string of a rendered JSON
    /// tree. Returns true when anything was replaced.
    pub fn redact(&self, value: &mut JsonValue) -> bool {
        match value {
            JsonValue::String(text) => match self.redact_text(text) {
                Some(redacted) => {
                    *text = redacted;
                    true
                }
                None => false,
            },
            JsonValue::Array(items) => {
                let mut any = false;
                for item in items {
                    any |= self.redact(item);
                }
                any
            }
            JsonValue::Object(map) => {
                let mut any = false;
                for item in map.values_mut() {
                    any |= self.redact(item);
                }
                any
            }
            _ => false,
        }
    }
}

/// Map schema-default paths to their default expression nodes. Runs
/// after explicit values, so a document value always wins.
fn collect_default_paths(
    module: &Module,
    id: NodeId,
    prefix: &str,
    paths: &mut HashMap<String, NodeId>,
) {
    match module.get(id) {
        Some(Node::SchemaTable { fields, .. }) => {
            for (key, child) in fields {
                let child_path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_default_paths(module, *child, &child_path, paths);
            }
        }
        Some(Node::SchemaField { field }) => {
            if let Some(default) = field.default {
                paths.entry(prefix.to_string()).or_insert(default);
            }
        }
        Some(Node::SchemaArray { element }) => {
            collect_default_paths(module, *element, prefix, paths);
        }
        _ => {}
    }
}

/// Structural view of the secret material in one module.
#[derive(Debug, Clone, Default)]
pub struct SecretProfile {
    nodes: HashSet<NodeId>,
    paths: HashMap<String, NodeId>,
    literals: SecretRedactor,
}

impl SecretProfile {
    /// Analyze a module. Costs one pass over the arena plus one walk of
    /// the value tree; callers analyze once per document.
    pub fn analyze(module: &Module) -> Self {
        // Seeds: every node reachable from a `secret(...)` locator.
        let mut nodes: HashSet<NodeId> = HashSet::new();
        for node in &module.arena {
            if let Node::Secret { locator } = node {
                secret_subtree(module, *locator, &mut nodes);
            }
        }

        // Derived: an ancestor container, an interpolation, a reference
        // or an operator whose operand is material. Build the reverse
        // (child → parents) map once and walk upward from the seeds:
        // linear in the arena and exact, with no fixpoint rounds.
        if !nodes.is_empty() {
            let mut parents: Vec<Vec<NodeId>> = vec![Vec::new(); module.arena.len()];
            for (index, node) in module.arena.iter().enumerate() {
                let parent = NodeId(index as u32);
                for child in node_children(node) {
                    if let Some(slot) = parents.get_mut(child.0 as usize) {
                        slot.push(parent);
                    }
                }
            }
            let mut stack: Vec<NodeId> = nodes.iter().copied().collect();
            while let Some(id) = stack.pop() {
                if let Some(list) = parents.get(id.0 as usize) {
                    for parent in list {
                        if nodes.insert(*parent) {
                            stack.push(*parent);
                        }
                    }
                }
            }
        }

        // Authored strings inside material nodes: the locator literal,
        // a literal referenced by the locator, and so on. Interpolation
        // literal parts are deliberately absent — they are the benign
        // scaffolding, not the material.
        let mut literals = SecretRedactor::new();
        for id in &nodes {
            if let Some(Node::Literal(Value::String(text))) = module.get(*id) {
                literals.insert(text.clone());
            }
        }

        // Paths for every value node reachable from the root: tables,
        // array elements, and references (which keep the referrer's
        // path). The root walk runs FIRST: a table field wins ties
        // against a `let` binding, exactly like the evaluator.
        let mut paths: HashMap<String, NodeId> = HashMap::new();
        if let Some(root) = module.root {
            let mut stack = vec![(root, String::new())];
            let mut seen: HashSet<(NodeId, String)> = HashSet::new();
            while let Some((id, path)) = stack.pop() {
                if !seen.insert((id, path.clone())) {
                    continue;
                }
                paths.entry(path.clone()).or_insert(id);
                match module.get(id) {
                    Some(Node::Table { fields }) => {
                        for (key, child) in fields {
                            let child_path = if path.is_empty() {
                                key.clone()
                            } else {
                                format!("{path}.{key}")
                            };
                            stack.push((*child, child_path));
                        }
                    }
                    Some(Node::Array { elements }) => {
                        for (index, child) in elements.iter().enumerate() {
                            stack.push((*child, format!("{path}[{index}]")));
                        }
                    }
                    Some(Node::Ref(target)) => stack.push((*target, path)),
                    _ => {}
                }
            }
        }
        for (name, id) in &module.bindings {
            paths.entry(name.clone()).or_insert(*id);
        }
        // Transforms replace the evaluated tree, so their paths map to
        // the transform's expression: `@patch("out", p)` and a selected
        // profile field both carry material into the final document.
        for id in &module.patches {
            if let Some(Node::PatchDirective {
                op: PatchOp::Set { path, value },
            }) = module.get(*id)
            {
                paths.insert(path.clone(), *value);
            }
        }
        for id in module.profiles.values() {
            if let Some(Node::Table { fields }) = module.get(*id) {
                for (key, child) in fields {
                    paths.insert(key.clone(), *child);
                }
            }
        }
        // Schema defaults fill absent paths during evaluation; explicit
        // values (inserted above) win.
        if let Some(schema) = module.schema {
            collect_default_paths(module, schema, "", &mut paths);
        }

        Self {
            nodes,
            paths,
            literals,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// True when the node's value is secret material or derives from it.
    pub fn node_is_secret(&self, id: NodeId) -> bool {
        self.nodes.contains(&id)
    }

    /// Node addressed by a dotted/indexed path (`a.b[0]`), when known.
    pub fn path_node(&self, path: &str) -> Option<NodeId> {
        self.paths.get(path).copied()
    }

    /// True when the path's value is secret material or derives from it.
    pub fn path_is_secret(&self, path: &str) -> bool {
        self.path_node(path)
            .is_some_and(|id| self.nodes.contains(&id))
    }

    /// Authored literal strings inside material nodes.
    pub fn literals(&self) -> &SecretRedactor {
        &self.literals
    }

    /// Redact secret substrings in a rendered tree.
    pub fn redact_json(&self, value: &mut JsonValue) -> bool {
        self.literals.redact(value)
    }

    /// Redact secret substrings in text.
    pub fn redact_text(&self, text: &str) -> Option<String> {
        self.literals.redact_text(text)
    }
}
