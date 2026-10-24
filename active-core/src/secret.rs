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

/// The media type a consumer may see. A tainted identity redacts its
/// media type (the wire shape is shared by JSON, YAML, TOML, `Debug`,
/// and inspect).
pub fn exposed_media_type(tainted: bool, media_type: &str) -> &str {
    if tainted {
        MARKER
    } else {
        media_type
    }
}

/// Remove one path and everything below it.
fn remove_subtree(paths: &mut HashMap<String, NodeId>, path: &str) {
    if path.is_empty() {
        paths.clear();
        return;
    }
    let dotted = format!("{path}.");
    let indexed = format!("{path}[");
    paths.retain(|key, _| key != path && !key.starts_with(&dotted) && !key.starts_with(&indexed));
}

/// Move one path and everything below it from `from` to `to`.
fn rename_subtree(paths: &mut HashMap<String, NodeId>, from: &str, to: &str) {
    remove_subtree(paths, to);
    let dotted = format!("{from}.");
    let indexed = format!("{from}[");
    let mut moved: Vec<(String, NodeId)> = Vec::new();
    paths.retain(|key, value| {
        if key == from {
            moved.push((to.to_string(), *value));
            false
        } else if let Some(suffix) = key.strip_prefix(&dotted) {
            moved.push((format!("{to}.{suffix}"), *value));
            false
        } else if let Some(suffix) = key.strip_prefix(&indexed) {
            moved.push((format!("{to}[{suffix}"), *value));
            false
        } else {
            true
        }
    });
    paths.extend(moved);
}

/// Bound the path walk: a cyclic `let` container (`let t = { a = t }`)
/// would otherwise mint a new path on every hop. A bounded map is
/// fail-closed: an unregistered path can never certify a value benign.
const MAX_PATH_DEPTH: usize = 256;
const MAX_PATHS: usize = 100_000;

/// Walk a value subtree from `root` under `base`, registering every
/// table field, array element, and reference target.
///
/// * `overwrite` values replace a mapping at the same path (transforms
///   win); otherwise the first mapping wins (a table field beats a
///   `let` binding, exactly like the evaluator).
/// * `replace_subtree` clears the path's existing descendants first: a
///   patch `Set` replaces the value wholesale.
fn walk_paths(
    module: &Module,
    root: NodeId,
    base: &str,
    paths: &mut HashMap<String, NodeId>,
    overwrite: bool,
    replace_subtree: bool,
) {
    if replace_subtree {
        remove_subtree(paths, base);
    }
    let base_depth = if base.is_empty() {
        0
    } else {
        base.matches(['.', '[']).count() + 1
    };
    let mut stack = vec![(root, base.to_string(), base_depth)];
    let mut seen: HashSet<(NodeId, String)> = HashSet::new();
    while let Some((id, path, depth)) = stack.pop() {
        if depth > MAX_PATH_DEPTH || paths.len() >= MAX_PATHS {
            continue;
        }
        if !seen.insert((id, path.clone())) {
            continue;
        }
        if overwrite {
            paths.insert(path.clone(), id);
        } else {
            paths.entry(path.clone()).or_insert(id);
        }
        match module.get(id) {
            Some(Node::Table { fields }) => {
                for (key, child) in fields {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    stack.push((*child, child_path, depth + 1));
                }
            }
            Some(Node::Array { elements }) => {
                for (index, child) in elements.iter().enumerate() {
                    stack.push((*child, format!("{path}[{index}]"), depth + 1));
                }
            }
            Some(Node::Ref(target)) => stack.push((*target, path, depth)),
            _ => {}
        }
    }
}

/// Whether a value is a table after following references (profile
/// merges recurse into tables; they replace scalars).
fn is_table_like(module: &Module, id: NodeId) -> bool {
    let mut current = id;
    for _ in 0..8 {
        match module.get(current) {
            Some(Node::Table { .. }) => return true,
            Some(Node::Ref(target)) => current = *target,
            _ => return false,
        }
    }
    false
}

/// The profile selector the document carries: absent, statically
/// named, or computed (a value expression we cannot resolve).
enum ProfileSelection {
    None,
    Named(String),
    Computed,
}

/// Follow the root `active_profile` node. A literal (or a reference to
/// one) is statically named; any other expression is computed.
fn profile_selection(module: &Module) -> ProfileSelection {
    let Some(root) = module.root else {
        return ProfileSelection::None;
    };
    let Some(Node::Table { fields }) = module.get(root) else {
        return ProfileSelection::None;
    };
    let Some(id) = fields.get("active_profile") else {
        return ProfileSelection::None;
    };
    let mut current = *id;
    for _ in 0..8 {
        match module.get(current) {
            Some(Node::Literal(Value::String(name))) => {
                return ProfileSelection::Named(name.clone())
            }
            Some(Node::Ref(target)) => current = *target,
            _ => return ProfileSelection::Computed,
        }
    }
    ProfileSelection::Computed
}

/// Public helper for diagnostics: the selector's literal value when the
/// document spells one (never a computed value, which may be material).
pub fn active_profile_name(module: &Module) -> Option<String> {
    match profile_selection(module) {
        ProfileSelection::Named(name) => Some(name),
        _ => None,
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
                walk_paths(module, default, prefix, paths, false, false);
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
    /// Paths from **all** profiles, used only when the selector is
    /// computed: we cannot know which profile applies, so taint fails
    /// closed (value rendering still follows the evaluated tree).
    fallback_paths: HashMap<String, NodeId>,
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

        // A variable that feeds a secret anywhere is secret everywhere
        // (spec/reproducibility.md §2): every `env` read of the same
        // name is material, even a later, unrelated-looking one.
        let mut env_names: HashSet<String> = HashSet::new();
        for id in &nodes {
            if let Some(Node::Env { name, .. }) = module.get(*id) {
                env_names.insert(name.clone());
            }
        }
        if !env_names.is_empty() {
            for (index, node) in module.arena.iter().enumerate() {
                if let Node::Env { name, .. } = node {
                    if env_names.contains(name) {
                        nodes.insert(NodeId(index as u32));
                    }
                }
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
        // against a `let` binding, exactly like the evaluator. Binding
        // subtrees are walked too, so `t.a` inside a `let t = {…}` is
        // addressable and taint-checkable.
        let mut paths: HashMap<String, NodeId> = HashMap::new();
        if let Some(root) = module.root {
            walk_paths(module, root, "", &mut paths, true, false);
        }
        for (name, id) in &module.bindings {
            walk_paths(module, *id, name, &mut paths, false, false);
        }
        // Evaluation applies the selected profile first, then patches.
        // Profile tables merge (their fields override per key); a
        // profile scalar replaces. Patches run in source order:
        // `Set` replaces, `Remove` deletes, `Rename` moves the
        // subtree. A computed selector cannot be resolved, so every
        // profile's paths join a taint-only fallback map.
        let mut fallback_paths: HashMap<String, NodeId> = HashMap::new();
        match profile_selection(module) {
            ProfileSelection::Named(name) => {
                if let Some(id) = module.profiles.get(&name) {
                    if let Some(Node::Table { fields }) = module.get(*id) {
                        for (key, child) in fields {
                            let merge = is_table_like(module, *child);
                            walk_paths(module, *child, key, &mut paths, true, !merge);
                        }
                    }
                }
            }
            ProfileSelection::Computed => {
                for id in module.profiles.values() {
                    if let Some(Node::Table { fields }) = module.get(*id) {
                        for (key, child) in fields {
                            let merge = is_table_like(module, *child);
                            walk_paths(module, *child, key, &mut fallback_paths, true, !merge);
                        }
                    }
                }
            }
            ProfileSelection::None => {}
        }
        for id in &module.patches {
            match module.get(*id) {
                Some(Node::PatchDirective {
                    op: PatchOp::Set { path, value },
                }) => {
                    walk_paths(module, *value, path, &mut paths, true, true);
                    walk_paths(module, *value, path, &mut fallback_paths, true, true);
                }
                Some(Node::PatchDirective {
                    op: PatchOp::Remove { path },
                }) => {
                    remove_subtree(&mut paths, path);
                    remove_subtree(&mut fallback_paths, path);
                }
                Some(Node::PatchDirective {
                    op: PatchOp::Rename { from, to },
                }) => {
                    rename_subtree(&mut paths, from, to);
                    rename_subtree(&mut fallback_paths, from, to);
                }
                _ => {}
            }
        }
        // Schema defaults fill absent paths during evaluation; explicit
        // values and transforms (inserted above) win.
        if let Some(schema) = module.schema {
            collect_default_paths(module, schema, "", &mut paths);
        }

        Self {
            nodes,
            paths,
            fallback_paths,
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
        let primary = self
            .paths
            .get(path)
            .is_some_and(|id| self.nodes.contains(id));
        primary
            || self
                .fallback_paths
                .get(path)
                .is_some_and(|id| self.nodes.contains(id))
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
