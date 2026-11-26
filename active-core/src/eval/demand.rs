//! L-P40 — demand-driven evaluation: the closure of one value path.
//!
//! `Demand` is an explicit, deterministic input to the evaluator. `All`
//! is today's whole-document walk; `Cone(P)` materialises only the nodes
//! that shape `P` and everything `P` depends on, so an undemanded
//! pathological subtree cannot deny service to the demanded cone
//! (LANG-C-013). The whole-document static phase and every safety gate
//! run before any demand walk, so a cone never hides a budget trip
//! (see `spec/budgets.md` §2).

use std::collections::{BTreeSet, HashMap};

use crate::diag::Diagnostic;
use crate::ir::{AnnotationKind, Module, Node, NodeId, PatchOp, Value};
use crate::secret::{node_children, SecretRedactor};

/// What an evaluation is asked to materialise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Demand {
    /// Every arena node the full walk reaches today. The default for
    /// every existing entry point; output stays byte-identical.
    All,
    /// The closure of one value path (or top-level `let` binding name):
    /// the node at the path, its value dependencies, the ancestor
    /// containers that shape it, and the transform inputs that write
    /// into the closure.
    Cone(String),
}

/// The materialised node set for one demand: an arena-indexed bit set
/// for `Cone`, or "everything" for `All`.
#[derive(Debug, Clone)]
pub enum Closure {
    All,
    Cone {
        path: String,
        members: Vec<bool>,
        count: usize,
        /// Extra demanded paths a §2.2.5 check reads that no value node
        /// carries yet — an absent root key a schema default would fill.
        /// Validation of such a field must still run so the check sees
        /// the value the full run sees. Sorted and deduped.
        paths: Vec<String>,
    },
}

impl Closure {
    /// Every arena node (the default demand).
    pub fn all() -> Self {
        Closure::All
    }

    /// Whether `id` is materialised under this closure.
    pub fn contains(&self, id: NodeId) -> bool {
        match self {
            Closure::All => true,
            Closure::Cone { members, .. } => members.get(id.0 as usize).copied().unwrap_or(false),
        }
    }

    /// The demanded path, or `None` for `All`.
    pub fn path(&self) -> Option<&str> {
        match self {
            Closure::All => None,
            Closure::Cone { path, .. } => Some(path.as_str()),
        }
    }

    /// Whether `path` is an extra demanded path (`§2.2.5`): a check in
    /// the closure reads it but no value node carries it, so the field
    /// must be validated anyway (a schema default fill can then supply
    /// the operand the check needs). `All` demands every path.
    pub fn demands_path(&self, path: &str) -> bool {
        match self {
            Closure::All => true,
            Closure::Cone { paths, .. } => paths.binary_search_by(|p| p.as_str().cmp(path)).is_ok(),
        }
    }

    /// The number of materialised nodes, or `None` for `All`.
    pub fn member_count(&self) -> Option<usize> {
        match self {
            Closure::All => None,
            Closure::Cone { count, .. } => Some(*count),
        }
    }
}

/// True when `path` is the cone root `cone`, an ancestor of it, or a
/// descendant of it — the paths a cone materialises (`§2.2`). Comparing
/// on segment boundaries (`.`, `[`, end of string) keeps `a.b` from
/// matching the unrelated key `a.bc`, and lets an array element `a[0]`
/// sit inside the cone `a`.
pub fn path_in_cone(path: &str, cone: &str) -> bool {
    path == cone || starts_with_segment(path, cone) || starts_with_segment(cone, path)
}

/// True when `long` begins with `short` at a segment boundary: the byte
/// after `short` is `.`, `[`, or the end of the string.
fn starts_with_segment(long: &str, short: &str) -> bool {
    long.len() > short.len()
        && long.as_bytes().starts_with(short.as_bytes())
        && matches!(long.as_bytes().get(short.len()), Some(b'.' | b'['))
}

/// Compute the closure of `path` over `module` (`§2.2`). Returns `None`
/// when `path` is not a dotted path through the document root (a binding
/// name or an indexed path): the caller then keeps its existing
/// projection fallback, so those spellings behave as they always have.
///
/// Membership is the set of nodes whose value path lies in the cone set
/// (the demanded subtree, its ancestors, and every value under it), plus
/// the document root, plus the transform and structural inputs the
/// demanded paths read (`§2.2.3`-`§2.2.4`), closed over the value
/// dependencies (`node_children` and a `PatchOp::Set` value). A table
/// reached as a value — an array element, an alias target, a profile
/// body — has no rooted paths, so its whole content is brought in; a
/// table on the demanded spine contributes only its path members, which
/// keeps an undemanded sibling's value out.
pub fn cone_closure(module: &Module, path: &str) -> Option<Closure> {
    let root = module.root?;
    // Resolve through the root table only: a cone is a document subtree.
    {
        let mut id = root;
        for segment in path.split('.') {
            let Some(Node::Table { fields }) = module.get(id) else {
                return None;
            };
            id = *fields.get(segment)?;
        }
    }
    let n = module.arena.len();
    let paths = module.value_paths();
    let mut members = vec![false; n];
    // Subtree expansion is tracked apart from membership so a subtree whose
    // root is already a path member still has every child walked (§2.2.3).
    let mut expanded = vec![false; n];
    let mut stack: Vec<NodeId> = Vec::new();
    // The root shapes every cone and carries no value path of its own.
    mark(root, &mut members, &mut stack);
    // Path members: the demanded subtree, its ancestors, and every value
    // under it. A sibling's path is not in the cone set, so it stays out.
    for (id, p) in &paths {
        if path_in_cone(p, path) {
            mark(*id, &mut members, &mut stack);
        }
    }
    // Structural and transform inputs the demanded paths read regardless
    // of their own paths: the envelope the post-walk verification reads,
    // the profile selector and bodies the merge reads, a patch whose
    // target is in the cone, and the candidates of an in-cone merge.
    if let Some(envelope) = crate::envelope::envelope_root(module) {
        include_subtree(module, envelope, &mut members, &mut expanded, &mut stack);
    }
    // Document-level selector keys are always demanded. They are a handful
    // of tiny nodes a whole-document diagnostic or the envelope gate reads
    // (`active_profile` drives W002, and `effective_profile` is derived
    // from the evaluated `active_profile`), so dropping them under a cone
    // lost `W002` and left the gate seeing no profile.
    if let Some(Node::Table { fields }) = module.get(root) {
        for key in ["active_mode", "active_capabilities", "active_profile"] {
            if let Some(&selector) = fields.get(key) {
                include_subtree(module, selector, &mut members, &mut expanded, &mut stack);
            }
        }
    }
    // The profile bodies are the merge inputs; a selector with no bodies
    // still reports W002 above, so this loop is empty in that case.
    for &body in module.profiles.values() {
        include_subtree(module, body, &mut members, &mut expanded, &mut stack);
    }
    // §2.2.6: the runtime secret seeds are material on every demand. The
    // seed text is computed by a `secret(...)` locator expression, so the
    // locator's whole subtree is a closure member whatever the demanded
    // path; otherwise the demand-scoped walk never evaluates an undemanded
    // locator and `SecretProfile::for_engine` never learns its text, so a
    // demanded value that copies that text exports in the clear
    // (`cone_value(P) != project(full_value, P)`). The set is a subset of
    // the full walk's, so the cone can never refuse where the full run
    // succeeds. One linear scan of the arena, the same order the closure
    // already pays for `value_paths`.
    for index in 0..n {
        let id = NodeId(index as u32);
        if matches!(module.get(id), Some(Node::Secret { .. })) {
            include_subtree(module, id, &mut members, &mut expanded, &mut stack);
        }
    }
    for &patch in &module.patches {
        let Some(Node::PatchDirective { op }) = module.get(patch) else {
            continue;
        };
        // A rename writes its destination as well as its source, so it is
        // a member when either path lies in the cone; when only the
        // destination does, the source subtree comes along so the moved
        // value materialises (§2.2.3). Any other directive is a member
        // only when its target is in the cone. A directive that writes the
        // envelope is always a member: the envelope subtree is always part
        // of the cone (it gates E035), so a write into it must run, or the
        // post-transform gate would read an unpatched envelope (§2.2.4).
        let demanded = match op {
            PatchOp::Set { path: target, .. } | PatchOp::Remove { path: target } => {
                path_in_cone(target, path) || crate::envelope::is_envelope_path(target)
            }
            PatchOp::Rename { from, to } => {
                let dest = crate::ir::rename_destination(from, to);
                if path_in_cone(from, path) {
                    true
                } else if path_in_cone(&dest, path)
                    || crate::envelope::is_envelope_path(from)
                    || crate::envelope::is_envelope_path(&dest)
                {
                    if let Some((source, _)) =
                        paths.iter().find(|(_, p)| p.as_str() == from.as_str())
                    {
                        include_subtree(module, *source, &mut members, &mut expanded, &mut stack);
                    }
                    true
                } else {
                    false
                }
            }
        };
        if demanded {
            mark(patch, &mut members, &mut stack);
        }
    }
    for (scope, pending) in &module.merge_pending {
        for entry in pending {
            // The cone names a document path, so membership is tested
            // against `path`, not the scope-relative `local`. A merge into
            // the envelope is always a member for the same reason a patch
            // that writes it is (§2.2.4).
            if path_in_cone(&entry.path, path) || crate::envelope::is_envelope_path(&entry.path) {
                mark(*scope, &mut members, &mut stack);
                for candidate in &entry.candidates {
                    include_subtree(
                        module,
                        candidate.node,
                        &mut members,
                        &mut expanded,
                        &mut stack,
                    );
                }
            }
        }
    }
    // Schema-side validation inputs (§2.2.3): a field whose document
    // path lies in the cone is validated, and `validate_*` reads its
    // constraint arguments from the evaluated node map (`default`,
    // `requires`, the bounds, `pattern`, `format`, `enum_descriptions`,
    // and the blob constraints). An argument outside the cone reads as
    // absent — a lost default fill, or a spurious `T004`/`T003` — so
    // every in-cone field's arguments are materialised.
    if let Some(schema) = module.schema {
        // §2.2.3: every in-cone field's arguments are materialised. The
        // envelope subtree is always a member (it gates E035), so its
        // schema arguments are materialised too: a default under the
        // envelope must fill under a cone exactly as in the full run, or
        // the gate would read an unfilled payload and miss the E035 (§2.2.4).
        for cone in [path, "envelope"] {
            include_schema_inputs(
                module,
                schema,
                "",
                cone,
                &mut members,
                &mut expanded,
                &mut stack,
            );
        }
    }
    // An annotation predicate on an in-cone key is validated against the
    // cone tree and reads its operands through the evaluated node map
    // (`predicate_value`). The predicate names its operands through `Ref`
    // nodes — `build_graph` resolves every binding `Ident` to a `Ref` —
    // so an operand outside the cone would read `None` and drop an
    // in-cone `T006` the full run emits.
    let binding_nodes: std::collections::HashSet<NodeId> =
        module.bindings.values().copied().collect();
    for (node_id, anns) in &module.annotations {
        for ann in anns {
            let AnnotationKind::Constraint { expr, .. } = &ann.kind else {
                continue;
            };
            // The gate `validate_annotations` applies through the result
            // tree: a `let`-binding annotation runs only when its node was
            // materialised; a key annotation when its path is in the cone.
            let demanded = if binding_nodes.contains(node_id) {
                members.get(node_id.0 as usize).copied().unwrap_or(false)
            } else {
                path_in_cone(&ann.path, path)
            };
            if !demanded {
                continue;
            }
            // `build_graph` rewrites every `Ident` that names a binding to
            // a `Ref` (and `value` to the annotated node), so the operand
            // subgraph reached from `expr` carries every `Ref` target the
            // predicate reads.
            include_subtree(module, *expr, &mut members, &mut expanded, &mut stack);
        }
    }
    // §2.2.5: a check whose predicate reads a materialised value must be
    // evaluated for the cone; its `T009` belongs to the cone's report,
    // and §5 forbids fetching around a check, so bring in the check node
    // and its dependencies. One check's operand can be another check's
    // subject, so inclusion iterates to a fixpoint. The driver is a
    // worklist keyed on the two events that can pull a check in: a node
    // becoming a member, and a name becoming demanded. Every node is
    // pushed on `stack` once when it is first marked, so draining the
    // stack visits each new member once, and a check already in the
    // closure is skipped: each footprint edge is examined a bounded
    // number of times, linear in the closure edges (design §2.2), rather
    // than O(passes x arena). Every value dependency of a new member (a
    // `Ref` target, an inline expression part, an array element) is
    // expanded as it drains, so the result stays a pure function of the
    // module and the path.
    let footprints: Vec<(NodeId, Vec<NodeId>, Vec<String>)> = module
        .schema_checks
        .values()
        .flatten()
        .map(|rule| {
            let (ids, names) = check_footprint(module, rule.expr);
            (rule.expr, ids, names)
        })
        .collect();
    // Reverse indexes: a node id, or a demanded name, to the checks that
    // read it. Built once from the footprints, so an event reaches only
    // the checks it can actually pull in.
    let mut node_readers: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut name_readers: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, (_, ids, names)) in footprints.iter().enumerate() {
        for id in ids {
            node_readers.entry(id.0).or_default().push(index);
        }
        for name in names {
            name_readers.entry(name.clone()).or_default().push(index);
        }
    }
    let n_checks = footprints.len();
    let mut state = CheckDemand {
        footprints,
        included: vec![false; n_checks],
        demanded: BTreeSet::new(),
        name_queue: Vec::new(),
    };
    while !stack.is_empty() || !state.name_queue.is_empty() {
        // A new member: expand its value dependencies and pull in every
        // check whose predicate ranges over it.
        while let Some(id) = stack.pop() {
            expand_node(module, &paths, id, &mut members, &mut expanded, &mut stack);
            if let Some(readers) = node_readers.get(&id.0) {
                for &index in readers {
                    include_check(&mut state, index, &mut members, &mut stack);
                }
            }
        }
        // A newly demanded name: carry its schema default and pull in
        // every check whose predicate reads it.
        while let Some(name) = state.name_queue.pop() {
            include_demanded_default(module, &name, &mut members, &mut expanded, &mut stack);
            if let Some(readers) = name_readers.get(name.as_str()) {
                for &index in readers {
                    include_check(&mut state, index, &mut members, &mut stack);
                }
            }
        }
    }
    let count = members.iter().filter(|member| **member).count();
    Some(Closure::Cone {
        path: path.to_string(),
        members,
        count,
        paths: state.demanded.into_iter().collect(),
    })
}

/// Mark `id` a member (idempotent) and queue it for dependency expansion.
fn mark(id: NodeId, members: &mut [bool], stack: &mut Vec<NodeId>) {
    let index = id.0 as usize;
    if index < members.len() && !members[index] {
        members[index] = true;
        stack.push(id);
    }
}

/// Mark `root` and every node `node_children` reaches from it as members,
/// pushing each newly marked node onto `stack` so the caller's dependency
/// closure expands them too. Used for a whole subtree whose value paths
/// must not gate membership (the envelope and profile selector inputs, and
/// any table reached as a value).
///
/// Expansion is tracked by `expanded`, separately from membership: a node
/// already a member (an ancestor on the demanded spine) still needs its
/// whole subtree walked, or a value table whose root is a path member would
/// be marked alone and its named children dropped, so `Demand::Cone` would
/// disagree with the full run. `expanded` is set once per node, so the
/// whole closure is built in linear time.
fn include_subtree(
    module: &Module,
    root: NodeId,
    members: &mut [bool],
    expanded: &mut [bool],
    stack: &mut Vec<NodeId>,
) {
    let mut work = vec![root];
    while let Some(id) = work.pop() {
        let index = id.0 as usize;
        if index >= members.len() || expanded[index] {
            continue;
        }
        expanded[index] = true;
        mark(id, members, stack);
        if let Some(node) = module.get(id) {
            work.extend(node_children(node));
        }
    }
}

/// Mark the schema default value node of one `§2.2.5`-demanded name that
/// names a top-level schema field with a default. An absent key a check
/// reads is filled from its default (`§2.3`), and that fill needs the
/// default node evaluated, so the cone must carry it; otherwise the cone
/// would report `T004`/`T009` the full run never emits. A name that
/// matches no top-level field, or a field with no default, marks nothing.
fn include_demanded_default(
    module: &Module,
    name: &str,
    members: &mut [bool],
    expanded: &mut [bool],
    stack: &mut Vec<NodeId>,
) {
    let Some(schema_id) = module.schema else {
        return;
    };
    let Some(Node::SchemaTable { fields, .. }) = module.get(schema_id) else {
        return;
    };
    let Some(&field_id) = fields.get(name) else {
        return;
    };
    if let Some(Node::SchemaField { field }) = module.get(field_id) {
        if let Some(default_id) = field.default {
            include_subtree(module, default_id, members, expanded, stack);
        }
    }
}

/// The `§2.2.5` check-inclusion fixpoint state: each check's static read
/// footprint (the node ids and `Ident` names its predicate ranges over),
/// the per-check "already in the closure" flags, the demanded names, and
/// the queue of newly demanded names still to process.
struct CheckDemand {
    footprints: Vec<(NodeId, Vec<NodeId>, Vec<String>)>,
    included: Vec<bool>,
    demanded: BTreeSet<String>,
    name_queue: Vec<String>,
}

/// Include the schema check at `index` in the closure (`§2.2.5`): mark
/// its expression a member (queueing its dependencies) and record the
/// names it reads, queueing each newly demanded name. Idempotent, so the
/// worklist driver examines each check at most once and the closure stays
/// a pure function of the module and the path.
fn include_check(
    state: &mut CheckDemand,
    index: usize,
    members: &mut [bool],
    stack: &mut Vec<NodeId>,
) {
    if state.included[index] {
        return;
    }
    state.included[index] = true;
    let (expr, names) = {
        let (expr, _, names) = &state.footprints[index];
        (*expr, names.clone())
    };
    for name in names {
        if state.demanded.insert(name.clone()) {
            state.name_queue.push(name);
        }
    }
    mark(expr, members, stack);
}

/// Mark the constraint-argument nodes of every schema field whose
/// document path lies in the cone `cone` (§2.2.3). Mirrors
/// `secret::collect_default_paths`: the schema tree is walked in parallel
/// with the document path, where a `SchemaArray` element contributes a
/// `[]` wildcard. A `one_of` branch shares its field's path, so it is
/// visited with the same prefix and contributes its own arguments.
fn include_schema_inputs(
    module: &Module,
    id: NodeId,
    prefix: &str,
    cone: &str,
    members: &mut [bool],
    expanded: &mut [bool],
    stack: &mut Vec<NodeId>,
) {
    match module.get(id) {
        Some(Node::SchemaTable { fields, .. }) => {
            for (key, child) in fields {
                let child_path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                include_schema_inputs(module, *child, &child_path, cone, members, expanded, stack);
            }
        }
        Some(Node::SchemaField { field }) => {
            // `path_in_cone` compares on segment boundaries, so an element
            // schema path (`a[].f`) sits under the array cone `a`; a cone
            // only ever names a dotted path through tables (`cone_closure`
            // resolves segments through `Node::Table` fields), so it never
            // carries a bracket to match against the wildcard.
            if path_in_cone(prefix, cone) {
                for arg in [
                    field.min,
                    field.max,
                    field.min_len,
                    field.max_len,
                    field.pattern,
                    field.format,
                    field.enum_descriptions,
                    field.requires,
                    field.default,
                    field.blob_media_type,
                    field.blob_max_size,
                ]
                .into_iter()
                .flatten()
                {
                    include_subtree(module, arg, members, expanded, stack);
                }
            }
            for branch in &field.one_of {
                include_schema_inputs(module, *branch, prefix, cone, members, expanded, stack);
            }
        }
        Some(Node::SchemaArray { element }) => {
            let element_prefix = if prefix.is_empty() {
                "[]".to_string()
            } else {
                format!("{prefix}[]")
            };
            include_schema_inputs(
                module,
                *element,
                &element_prefix,
                cone,
                members,
                expanded,
                stack,
            );
        }
        _ => {}
    }
}

/// Expand one member node's value dependencies (`§2.2`): a `Ref` to a
/// table outside the cone, an inline expression part, an array element,
/// or the value child of a `PatchOp::Set`. A value dependency is included
/// whole (its own subtree too); a spine ancestor keeps only its path
/// members, so an undemanded sibling of the demanded path stays out.
fn expand_node(
    module: &Module,
    paths: &HashMap<NodeId, String>,
    id: NodeId,
    members: &mut [bool],
    expanded: &mut [bool],
    stack: &mut Vec<NodeId>,
) {
    let Some(node) = module.get(id) else {
        return;
    };
    match node {
        Node::Table { fields } => {
            // A table member that is a spine ancestor keeps only its
            // path members (marked by the cone-set loop); a table
            // reached as a value has no rooted paths, so its whole
            // content comes in.
            for child in fields.values() {
                if !paths.contains_key(child) {
                    include_subtree(module, *child, members, expanded, stack);
                }
            }
        }
        _ => {
            // A value dependency is needed in full: a `Ref` target,
            // an array element, or an expression operand materialises
            // whole, so its own subtree comes in too. A named table
            // reached this way has rooted paths, but they lie outside
            // the cone, so `mark` alone would leave its fields
            // unmaterialised.
            for child in node_children(node) {
                include_subtree(module, child, members, expanded, stack);
            }
        }
    }
    if let Node::PatchDirective {
        op: PatchOp::Set { value, .. },
    } = node
    {
        include_subtree(module, *value, members, expanded, stack);
    }
}

/// The static read footprint of one check predicate: every node in its
/// expression subgraph (so the fixpoint can ask whether any of them is a
/// closure member) and every `Ident` name it reads (so an absent key a
/// default fills counts as demanded, `§2.2.5`). Pure in the module, so
/// the check fixpoint computes it once per rule instead of re-walking the
/// expression on every pass.
fn check_footprint(module: &Module, expr: NodeId) -> (Vec<NodeId>, Vec<String>) {
    let mut ids: Vec<NodeId> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut work = vec![expr];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = work.pop() {
        if !seen.insert(id) {
            continue;
        }
        ids.push(id);
        if let Some(node) = module.get(id) {
            if let Node::Ident(name) = node {
                names.push(name.clone());
            }
            work.extend(node_children(node));
            if let Node::PatchDirective {
                op: PatchOp::Set { value, .. },
            } = node
            {
                work.push(*value);
            }
        }
    }
    (ids, names)
}

/// What one evaluation cost, in the DP units `spec/budgets.md` already
/// defines (`§3.4`). `total` is the seed for the demand (the closure's
/// DP total for a cone, the whole graph's for `All`) plus the measured
/// dynamic growth the walk charged; `limit` is the effective
/// `max_total_value_size` ceiling (`Budget::UNLIMITED` when disabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChargeReport {
    pub total: u64,
    pub limit: u64,
}

/// One demand-driven evaluation: the value tree (cone-shaped or whole),
/// its diagnostics, the runtime secret seeds, and the charge report.
#[derive(Debug, Clone)]
pub struct DemandOutcome {
    pub value: Value,
    pub diagnostics: Vec<Diagnostic>,
    pub seeds: SecretRedactor,
    pub charge: ChargeReport,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn module(source: &str) -> Module {
        parser::parse(source, "test.active").expect("parse")
    }

    fn member_paths(module: &Module, cone: &str) -> Vec<String> {
        let closure = cone_closure(module, cone).expect("closure");
        let paths = module.value_paths();
        let mut out: Vec<String> = (0..module.arena.len())
            .filter_map(|index| {
                let id = NodeId(index as u32);
                if closure.contains(id) {
                    paths.get(&id).cloned()
                } else {
                    None
                }
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn cone_includes_spine_and_subtree_but_not_siblings() {
        let m = module("a:\n  b:\n    c: 1\n  sib: 9\nother: 7\n");
        let got = member_paths(&m, "a.b.c");
        // The spine a, a.b and the demanded leaf a.b.c are in; the
        // sibling a.sib and the unrelated root key other are out.
        assert!(got.contains(&"a".to_string()), "{got:?}");
        assert!(got.contains(&"a.b".to_string()), "{got:?}");
        assert!(got.contains(&"a.b.c".to_string()), "{got:?}");
        assert!(!got.contains(&"a.sib".to_string()), "{got:?}");
        assert!(!got.contains(&"other".to_string()), "{got:?}");
    }

    #[test]
    fn cone_pulls_in_referenced_binding() {
        // `c` references the top-level `let base`; the cone of a.c must
        // materialise `base` even though its path is outside the cone
        // set, because it is a value dependency.
        let m = module("let base = 5\na:\n  c: base\nother: 7\n");
        let closure = cone_closure(&m, "a.c").expect("closure");
        let target = m.resolve_value_path("a.c").unwrap();
        let base = m.bindings.get("base").copied().unwrap();
        assert!(closure.contains(target));
        assert!(closure.contains(base));
        let other = m.resolve_value_path("other").unwrap();
        assert!(!closure.contains(other));
    }

    #[test]
    fn cone_carries_secret_locator_subtree() {
        // Negative control (§2.2.6): the demanded path never references
        // `s`, so without the secret-locator rule the closure would drop
        // the locator and the demand walk would not seed the runtime text
        // `secret(env.TOK)` computes. The locator subtree is therefore a
        // member of every cone, and an unrelated root key stays out.
        let m = module("let s = secret(env.TOK)\na:\n  keep: 1\nother: 7\n");
        let closure = cone_closure(&m, "a.keep").expect("closure");
        let secret = (0..m.arena.len() as u32)
            .map(NodeId)
            .find(|id| matches!(m.get(*id), Some(Node::Secret { .. })))
            .expect("secret node");
        assert!(closure.contains(secret), "secret node is not a member");
        for child in node_children(m.get(secret).unwrap()) {
            assert!(
                closure.contains(child),
                "locator child {} is not a member",
                child.0
            );
        }
        let other = m.resolve_value_path("other").unwrap();
        assert!(!closure.contains(other), "undemanded sibling leaked in");
    }

    #[test]
    fn cone_of_missing_path_is_none() {
        let m = module("a:\n  b: 1\n");
        assert!(cone_closure(&m, "a.missing").is_none());
    }

    #[test]
    fn path_in_cone_respects_segment_boundaries() {
        assert!(path_in_cone("a.b", "a"));
        assert!(path_in_cone("a", "a.b"));
        assert!(path_in_cone("a.b", "a.b"));
        assert!(!path_in_cone("a.bc", "a.b"));
        assert!(!path_in_cone("a.b", "a.bc"));
        // An array element sits inside its array's cone.
        assert!(path_in_cone("a[0]", "a"));
        assert!(path_in_cone("a", "a[0]"));
        assert!(!path_in_cone("a[0]", "a[1]"));
    }

    #[test]
    fn cone_of_value_table_includes_its_fields() {
        // An array element is a value container with no rooted paths, so
        // the whole element and its fields must be materialised.
        let m = module("arr: [{ a = 1, b = 2 }]\nother: 7\n");
        let arr = m.resolve_value_path("arr").unwrap();
        let element = match m.get(arr) {
            Some(Node::Array { elements }) => elements[0],
            _ => panic!("arr is not an array"),
        };
        let field_a = match m.get(element) {
            Some(Node::Table { fields }) => fields["a"],
            _ => panic!("element is not a table"),
        };
        let closure = cone_closure(&m, "arr").expect("closure");
        assert!(closure.contains(element), "element table missing");
        assert!(closure.contains(field_a), "element field missing");
        assert!(!closure.contains(m.resolve_value_path("other").unwrap()));
    }

    #[test]
    fn cone_of_named_table_via_ref_includes_its_fields() {
        // Review-F1: `t` is a reference to the named table `a`, which has
        // rooted paths of its own (`a.x`, `a.y`) lying outside the cone.
        // A spine walk that keeps only a member's rooted path members
        // would leave those fields unmaterialised, so the projection of
        // `t` came back empty. The value dependency must bring the whole
        // referenced table in.
        let m = module("a:\n  x: 1\n  y: 2\nt: a\n");
        let got = member_paths(&m, "t");
        assert_eq!(
            got,
            vec![
                "a".to_string(),
                "a.x".to_string(),
                "a.y".to_string(),
                "t".to_string(),
            ],
            "{got:?}"
        );
    }

    #[test]
    fn cone_excludes_undemanded_array_and_its_elements() {
        // A sibling array is not on the demanded spine; neither it nor its
        // elements may be materialised.
        let m = module("a:\n  b: 1\n  big: [9, 9, 9, 9]\nother: 7\n");
        let big = m.resolve_value_path("a.big").unwrap();
        let element = match m.get(big) {
            Some(Node::Array { elements }) => elements[0],
            _ => panic!("a.big is not an array"),
        };
        let closure = cone_closure(&m, "a.b").expect("closure");
        assert!(!closure.contains(big), "undemanded array is a member");
        assert!(!closure.contains(element), "undemanded element is a member");
        assert!(closure.contains(m.resolve_value_path("a.b").unwrap()));
    }

    #[test]
    fn cone_carries_envelope_subtree() {
        let m = module("envelope:\n  version: 1\n  sender: \"peer\"\nx:\n  y: 1\n");
        let env = m.resolve_value_path("envelope").unwrap();
        let closure = cone_closure(&m, "x.y").expect("closure");
        assert!(closure.contains(env), "envelope subtree missing");
    }

    #[test]
    fn cone_carries_every_envelope_child_when_the_cone_is_inside_it() {
        // Review-C1: when the cone path lies under the envelope, the
        // envelope root is already a member (a demanded spine ancestor), so a
        // subtree walk that stopped at the first member marked only the root
        // and dropped `version`/`sender`. The post-walk gate then read a
        // partial envelope and fabricated E033/E035 the full run never emits.
        // Every envelope child must be materialised.
        let m = module(
            "envelope:\n  version: 1\n  sender: \"peer\"\n  payload:\n    ok: true\nx:\n  y: 1\n",
        );
        let got = member_paths(&m, "envelope.payload");
        for want in [
            "envelope",
            "envelope.version",
            "envelope.sender",
            "envelope.payload",
            "envelope.payload.ok",
        ] {
            assert!(got.contains(&want.to_string()), "missing {want}: {got:?}");
        }
    }

    #[test]
    fn cone_materialises_a_container_patch_value_that_is_a_spine_ancestor() {
        // Review-C1: the `@patch` Set value is the whole table `a`, which is
        // also the demanded path's spine ancestor. The value dependency must
        // bring `a`'s other fields into the closure, or the cone's `a.b`
        // copy is truncated and `cone_value != project(full_value)`.
        let m = module("a:\n  x: 1\n  y: 2\n  b: 0\n@patch(\"a.b\", a)\nkeep: 5\n");
        let got = member_paths(&m, "a.b");
        for want in ["a", "a.b", "a.x", "a.y"] {
            assert!(got.contains(&want.to_string()), "missing {want}: {got:?}");
        }
    }

    #[test]
    fn cone_carries_profile_body() {
        let m = module("active_profile = \"p\"\nprofile \"p\" {\n  a = 2\n}\nx:\n  y: 1\n");
        let closure = cone_closure(&m, "x.y").expect("closure");
        assert!(!m.profiles.is_empty(), "profiles parsed");
        for &body in m.profiles.values() {
            assert!(closure.contains(body), "profile body missing");
        }
    }

    /// Review-F8: a selector that names no profile (`active_profile =
    /// "missing"`) yields W002 in the full run. With no profile bodies the
    /// old cone skipped every root selector, so the cone reported no W002
    /// and the envelope gate saw no profile at all. The document-level
    /// selector keys are always demanded.
    #[test]
    fn cone_carries_root_selectors_without_profile_bodies() {
        let m = module("active_profile = \"missing\"\nx:\n  y: 1\n");
        assert!(m.profiles.is_empty(), "no profile bodies parsed");
        let closure = cone_closure(&m, "x.y").expect("closure");
        let selector = match m.get(m.root.expect("root")) {
            Some(Node::Table { fields }) => fields["active_profile"],
            _ => panic!("root is not a table"),
        };
        assert!(closure.contains(selector), "root selector missing");
    }

    /// The default node of a field a `§2.2.5` check demands is carried in
    /// the cone, so the resilient fill can apply it and the cone reports
    /// the same diagnostics the full run does.
    #[test]
    fn cone_carries_default_of_demanded_check_operand() {
        let m = module(
            "schema {\n  a = Int,\n  b = Int(default = 1),\n  check {\n    both = a == b\n  }\n}\na = 1\n",
        );
        let closure = cone_closure(&m, "a").expect("closure");
        let probe = schema_probe(&m, &closure);
        assert_eq!(probe.demanded, vec!["b".to_string()], "demanded operand");
        assert_eq!(probe.field_defaults.len(), 1, "one defaulted field");
        assert!(
            closure.contains(probe.field_defaults[0]),
            "the demanded field's default node must be a member"
        );
    }

    #[test]
    fn cone_of_a_table_demands_no_sibling_default() {
        // Cone `a.b` pulls in no check, so the sibling `c` default stays out.
        let m = module(
            "schema {\n  a = Table { b = Int, c = Int(default = 1) },\n  check {\n    c_is_one = a.c == 1\n  }\n}\na = { b = 1 }\n",
        );
        let closure = cone_closure(&m, "a.b").expect("closure");
        let probe = schema_probe(&m, &closure);
        assert!(probe.demanded.is_empty(), "no check reads a.b");
        for default_id in probe.field_defaults {
            assert!(
                !closure.contains(default_id),
                "an undemanded default must stay out"
            );
        }
    }

    struct SchemaProbe {
        field_defaults: Vec<NodeId>,
        demanded: Vec<String>,
    }

    /// Every schema field's default node, and the closure's demanded set.
    fn schema_probe(m: &Module, closure: &Closure) -> SchemaProbe {
        let mut field_defaults = Vec::new();
        for index in 0..m.arena.len() {
            let id = NodeId(index as u32);
            if let Some(Node::SchemaField { field }) = m.get(id) {
                if let Some(default_id) = field.default {
                    field_defaults.push(default_id);
                }
            }
        }
        let demanded = match closure {
            Closure::Cone { paths, .. } => paths.clone(),
            Closure::All => Vec::new(),
        };
        SchemaProbe {
            field_defaults,
            demanded,
        }
    }

    #[test]
    fn indexed_and_binding_paths_have_no_cone() {
        let m = module("arr: [1, 2]\nlet base = 5\n");
        assert!(cone_closure(&m, "arr[0]").is_none());
        assert!(cone_closure(&m, "base").is_none());
        assert!(cone_closure(&m, "missing").is_none());
        assert!(cone_closure(&m, "arr").is_some());
    }

    /// Review-M8: a rename writes its destination as well as its source.
    /// A cone that names the destination must carry the directive, so the
    /// cone emits the same patch result the full run does — an `E019`
    /// for a colliding target, or the moved value. The branch also pulls
    /// the source subtree in so the moved value materialises.
    #[test]
    fn cone_carries_rename_when_destination_is_demanded() {
        let m = module("a:\n  x: 1\n  y: 2\n@rename(\"a.x\", \"y\")\n");
        assert_eq!(m.patches.len(), 1, "one patch directive");
        let patch = m.patches[0];
        // The source `a.x` is in the cone, so the directive is a member.
        assert!(cone_closure(&m, "a.x").unwrap().contains(patch));
        // The destination `a.y` is written by the rename, so a demand
        // that names it must carry the directive.
        let dest = cone_closure(&m, "a.y").unwrap();
        assert!(dest.contains(patch), "destination cone drops the rename");
        // ... and the source value subtree comes along.
        let got = member_paths(&m, "a.y");
        assert!(
            got.contains(&"a.x".to_string()),
            "source not pulled: {got:?}"
        );
        assert!(got.contains(&"a.y".to_string()), "{got:?}");
    }

    /// A cone whose path is neither a rename's source nor a rename's
    /// destination leaves the directive out, so an unrelated demand is
    /// not charged for it.
    #[test]
    fn cone_of_unrelated_path_drops_rename() {
        let m = module("a:\n  x: 1\nz: 3\n@rename(\"a.x\", \"y\")\n");
        let patch = m.patches[0];
        let closure = cone_closure(&m, "z").unwrap();
        assert!(
            !closure.contains(patch),
            "an unrelated cone must drop the rename"
        );
    }

    /// Review-L10: the check-inclusion fixpoint is a worklist, so a long
    /// chain of checks that each pull in the next resolves in one linear
    /// sweep. Each check reads the next field's name, so including one
    /// demands the name that pulls in the next; listing the checks in
    /// reverse exercises what was O(passes x arena) as O(edges).
    #[test]
    fn cone_resolves_a_long_reverse_ordered_check_chain() {
        const N: usize = 24;
        let mut source = String::from("schema {\n");
        for i in 0..=N {
            source.push_str(&format!("  f{i} = Int(default = {i}),\n"));
        }
        source.push_str("  check {\n");
        for k in (0..N).rev() {
            source.push_str(&format!("    k{k} = f{k} == f{}\n", k + 1));
        }
        source.push_str("  }\n}\nf0 = 1\n");
        let m = module(&source);
        let closure = cone_closure(&m, "f0").expect("closure");
        // Every check is pulled in through the chain that starts at `f0`.
        let mut checks = 0;
        for rule in m.schema_checks.values().flatten() {
            assert!(closure.contains(rule.expr), "check {} missing", rule.name);
            checks += 1;
        }
        assert_eq!(checks, N, "every check is in scope");
        // ... and every defaulted field follows the demanded name.
        let mut defaults = 0;
        for index in 0..m.arena.len() {
            let id = NodeId(index as u32);
            if let Some(Node::SchemaField { field }) = m.get(id) {
                if let Some(default_id) = field.default {
                    assert!(closure.contains(default_id), "default node missing");
                    defaults += 1;
                }
            }
        }
        assert_eq!(defaults, N + 1, "each field carries a default");
    }
}
