//! L-P40 §3 — the three-class cost model.
//!
//! One class per node, computed statically from the same topological
//! pass the budget bounds already use: `literal` (0), `alias` (1), and
//! `computed` (2). A node's *effective* class is the maximum of its own
//! class and its dependency children's, so a container is classed by its
//! most expensive part. There are no source annotations and no weights:
//! exactly three classes plus the DP units `spec/budgets.md` §2 already
//! defines. The report exists to explain a run's charge — which class
//! spends the units, and which nodes spend the most — and to name the
//! demand it describes (`all` or a cone path).

use crate::ir::{Module, Node, NodeId, PatchOp, Value};
use crate::secret::node_children;
use crate::Budget;

use super::demand::{cone_closure, Closure, Demand};

/// The three fixed classes of §3.1, in cost order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CostClass {
    /// `Literal`, `Quantity`, unresolved `Ident`: 1 DP unit.
    Literal = 0,
    /// `Ref`, `Table`, `Array`, `Schema*`: 1 + max(child) units; a `Ref`
    /// also counts one per hop in its chain.
    Alias = 1,
    /// `Env`, `Secret`, `ContentAddressed`, `Interpolate`, `Binary`,
    /// `Unary`, `ApplyConstraint`, `Semantic`, patch writes: measured
    /// bytes plus child units, charged through the accountant.
    Computed = 2,
}

impl CostClass {
    /// The three classes in cost order, for deterministic iteration.
    pub const ALL: [CostClass; 3] = [CostClass::Literal, CostClass::Alias, CostClass::Computed];

    /// Class index (0, 1, 2) for the report arrays.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Fixed class name for the dashboard.
    pub fn name(self) -> &'static str {
        match self {
            CostClass::Literal => "literal",
            CostClass::Alias => "alias",
            CostClass::Computed => "computed",
        }
    }
}

/// One ranked node of the report (§3.2.1). `class` is the effective
/// class (own or a child's), `reason` names the node's OWN kind, and
/// `charged_units` is the node's DP bound in the units the budget
/// charges.
#[derive(Debug, Clone)]
pub struct CostNode {
    pub node: u32,
    pub path: Option<String>,
    pub class: CostClass,
    pub depth: u64,
    pub charged_units: u64,
    pub reason: String,
}

/// The cost report for one demand: class counts and charged units, and
/// the bounded, deterministic node ranking.
#[derive(Debug, Clone)]
pub struct CostModel {
    /// `None` is the whole-document demand; `Some(path)` is a cone.
    pub demand: Option<String>,
    /// Arena nodes this demand materialises: the whole arena for `All`,
    /// the closure's member count for a cone. The demand-scoped sibling
    /// of the dashboard's `usage.nodes`, which is the whole-document
    /// arena length (the X001 observation).
    pub scope_nodes: u64,
    /// Nodes per class, indexed by class (0, 1, 2).
    pub class_counts: [u64; 3],
    /// Sum of charged units per class, same indexing.
    pub class_units: [u64; 3],
    /// True when more than [`TOP_NODES`] nodes were ranked.
    pub truncated: bool,
    /// The top nodes by `(charged_units desc, path asc, node asc)`.
    pub top: Vec<CostNode>,
}

/// Hard cap on the ranked node list (§3.2.1), so the report is bounded.
pub const TOP_NODES: usize = 256;

/// Build the cost report for a demand. `Demand::All` ranks every arena
/// node; a resolved `Cone(P)` ranks only its closure, and a cone path
/// that is not a document subtree falls back to the whole document, the
/// same total-function rule as [`Engine::evaluate_demanded`].
///
/// `profile` redacts the report: a node whose own content is material
/// (a secret, a whole-value copy of one, or a value under a secret
/// locator) is sized at the redacted marker and its parents aggregate
/// that clamped child, so `charged_units` never reveals material length
/// (LANG-C-010). The report carries no material of its own.
pub fn cost_model(
    module: &Module,
    budget: &Budget,
    demand: &Demand,
    profile: &crate::secret::SecretProfile,
) -> CostModel {
    let closure = match demand {
        Demand::All => Closure::all(),
        Demand::Cone(path) => cone_closure(module, path).unwrap_or(Closure::all()),
    };
    cost_model_for(module, budget, &closure, profile)
}

fn cost_model_for(
    module: &Module,
    budget: &Budget,
    closure: &Closure,
    profile: &crate::secret::SecretProfile,
) -> CostModel {
    let demand = match closure {
        Closure::All => None,
        Closure::Cone { path, .. } => Some(path.clone()),
    };
    let n = module.arena.len();
    // `member_count` is `None` for the whole document, so the whole
    // arena is the scope; for a cone it is the closure size.
    let scope_nodes = closure.member_count().unwrap_or(n) as u64;
    let empty = |demand: Option<String>| CostModel {
        demand,
        scope_nodes,
        class_counts: [0; 3],
        class_units: [0; 3],
        truncated: false,
        top: Vec::new(),
    };

    // Recompute the shared DP bounds without enforcement: this report
    // explains a run that already cleared the whole static phase, so
    // this pass is total. (A toposort failure is the only structural
    // error, and the enforcing pass would have failed the run first.)
    let order = match petgraph::algo::toposort(&module.graph, None) {
        Ok(order) => order,
        Err(_) => return empty(demand),
    };
    // The report's units are the demand's own DP bounds: a cone counts
    // only its closure members, the same scope the accountant seeds from
    // (`DpScope::Closure`), so the class totals explain the charge the
    // run paid and never inherit an undemanded sibling's weight (§4.2).
    // `All` counts the whole arena.
    let dp_scope = match closure {
        Closure::All => super::DpScope::WholeReport,
        Closure::Cone { members, .. } => super::DpScope::Closure(members.as_slice()),
    };
    // Material nodes are sized at the redacted marker, and each parent
    // aggregates the clamped child, so no reported unit derives from
    // material length.
    let material = material_flags(module, profile);
    let (size, depth) = match super::materialization_bounds(
        module,
        &order,
        budget,
        dp_scope,
        Some(material.as_slice()),
    ) {
        Ok(bounds) => (bounds.size, bounds.depth),
        Err(_) => return empty(demand),
    };

    // Effective class and, for a `Ref`, its hop count. Children precede
    // parents in the topological order, so a child's entry is final
    // before its dependent reads it.
    let mut own = vec![CostClass::Literal; n];
    let mut hops = vec![0u64; n];
    let mut effective = vec![CostClass::Literal; n];
    for node_idx in &order {
        let id = module.graph[*node_idx];
        let i = id.0 as usize;
        if i >= n {
            continue;
        }
        let Some(node) = module.get(id) else {
            continue;
        };
        let this_own = own_class(node);
        own[i] = this_own;
        if let Node::Ref(target) = node {
            hops[i] = hops.get(target.0 as usize).copied().unwrap_or(0) + 1;
        }
        let mut eff = this_own;
        for child in node_children(node) {
            eff = eff.max(
                effective
                    .get(child.0 as usize)
                    .copied()
                    .unwrap_or(CostClass::Literal),
            );
        }
        if let Node::PatchDirective {
            op: PatchOp::Set { value, .. },
        } = node
        {
            eff = eff.max(
                effective
                    .get(value.0 as usize)
                    .copied()
                    .unwrap_or(CostClass::Literal),
            );
        }
        effective[i] = eff;
    }

    let paths = module.value_paths();
    let mut class_counts = [0u64; 3];
    let mut class_units = [0u64; 3];
    let mut top: Vec<CostNode> = Vec::new();
    for i in 0..n {
        let id = NodeId(i as u32);
        if !closure.contains(id) {
            continue;
        }
        let class = effective[i];
        class_counts[class.index()] += 1;
        class_units[class.index()] = class_units[class.index()].saturating_add(size[i]);
        top.push(CostNode {
            node: i as u32,
            path: paths.get(&id).cloned(),
            class,
            depth: depth.get(i).copied().unwrap_or(0),
            charged_units: size.get(i).copied().unwrap_or(0),
            reason: reason_for(own[i], module.get(id), hops[i]),
        });
    }
    let truncated = top.len() > TOP_NODES;
    top.sort_by(|a, b| {
        b.charged_units
            .cmp(&a.charged_units)
            .then_with(|| {
                a.path
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.path.as_deref().unwrap_or(""))
            })
            .then_with(|| a.node.cmp(&b.node))
    });
    top.truncate(TOP_NODES);

    CostModel {
        demand,
        scope_nodes,
        class_counts,
        class_units,
        truncated,
        top,
    }
}

/// Arena-indexed material flags for the cost report and the report-side
/// charge bounds. A node is material when its own value is secret or a
/// whole-value copy of one, or when it lives at a path the profile marks
/// material (a secret locator subtree, a `secret(...)` selection, a patch
/// target). The report sizes such a node at the redacted marker and lets
/// every parent aggregate it, so no reported unit derives from material
/// length.
pub(crate) fn material_flags(module: &Module, profile: &crate::secret::SecretProfile) -> Vec<bool> {
    let paths = module.value_paths();
    let mut flags = vec![false; module.arena.len()];
    for (i, flag) in flags.iter_mut().enumerate() {
        let id = NodeId(i as u32);
        let by_path = paths
            .get(&id)
            .is_some_and(|path| profile.path_is_secret(path));
        *flag = by_path || node_is_material(module.get(id), profile);
    }
    flags
}

/// Whether a node's own value is material by kind: a `Secret`, or a
/// literal the profile redacts (an exact copy of a secret, at any
/// length).
fn node_is_material(node: Option<&Node>, profile: &crate::secret::SecretProfile) -> bool {
    match node {
        Some(Node::Secret { .. }) => true,
        Some(Node::Literal(Value::Secret(_))) => true,
        Some(Node::Literal(Value::String(text))) => profile.redact_text(text).is_some(),
        _ => false,
    }
}

/// The node's OWN class, before the child maximum (§3.1).
fn own_class(node: &Node) -> CostClass {
    match node {
        Node::Literal(_) | Node::Quantity { .. } | Node::Ident(_) => CostClass::Literal,
        Node::Ref(_)
        | Node::Table { .. }
        | Node::Array { .. }
        | Node::SchemaTable { .. }
        | Node::SchemaArray { .. }
        | Node::SchemaField { .. } => CostClass::Alias,
        Node::Env { .. }
        | Node::Secret { .. }
        | Node::ContentAddressed { .. }
        | Node::Interpolate { .. }
        | Node::Binary { .. }
        | Node::Unary { .. }
        | Node::ApplyConstraint { .. }
        | Node::Semantic { .. }
        | Node::PatchDirective { .. } => CostClass::Computed,
    }
}

/// Fixed explanation string for a node's own kind (§3.2.1).
fn reason_for(own: CostClass, node: Option<&Node>, hops: u64) -> String {
    match (own, node) {
        (CostClass::Literal, _) => "literal".to_string(),
        (CostClass::Alias, Some(Node::Ref(_))) => format!("alias chain depth {hops}"),
        (
            CostClass::Alias,
            Some(Node::SchemaField { .. } | Node::SchemaTable { .. } | Node::SchemaArray { .. }),
        ) => "schema shape".to_string(),
        (CostClass::Alias, _) => "container shape".to_string(),
        (CostClass::Computed, Some(Node::Env { .. })) => "computed: env".to_string(),
        (CostClass::Computed, Some(Node::Secret { .. })) => "computed: secret".to_string(),
        (CostClass::Computed, Some(Node::ContentAddressed { .. })) => "computed: blob".to_string(),
        (CostClass::Computed, Some(Node::Interpolate { .. })) => {
            "computed: interpolate".to_string()
        }
        (CostClass::Computed, Some(Node::Binary { .. })) => "computed: binary".to_string(),
        (CostClass::Computed, Some(Node::Unary { .. })) => "computed: unary".to_string(),
        (CostClass::Computed, Some(Node::ApplyConstraint { .. })) => {
            "computed: constraint".to_string()
        }
        (CostClass::Computed, Some(Node::Semantic { .. })) => "computed: semantic".to_string(),
        (CostClass::Computed, Some(Node::PatchDirective { .. })) => "computed: patch".to_string(),
        (CostClass::Computed, _) => "computed".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(source: &str) -> Module {
        crate::parse(source, "cost").expect("parse")
    }

    fn profile(module: &Module) -> crate::secret::SecretProfile {
        crate::secret::SecretProfile::analyze_with_profile(module, None)
    }

    #[test]
    fn classes_are_the_max_of_own_and_children() {
        // `keep` is a table (alias) whose interpolation child is
        // computed, so `keep` is classed computed; its sibling `n` is a
        // literal.
        let m = module("name: \"x\"\nkeep:\n  n: 1\n  s: \"${name}\"\n");
        let model = cost_model(&m, &Budget::default(), &Demand::All, &profile(&m));
        let by_path = |p: &str| -> &CostNode {
            model
                .top
                .iter()
                .find(|n| n.path.as_deref() == Some(p))
                .unwrap_or_else(|| panic!("no node at {p}"))
        };
        assert_eq!(by_path("keep.n").class, CostClass::Literal);
        assert_eq!(by_path("keep.s").class, CostClass::Computed);
        assert_eq!(by_path("keep").class, CostClass::Computed);
        assert_eq!(by_path("keep.n").reason, "literal");
        assert_eq!(by_path("keep.s").reason, "computed: interpolate");
    }

    #[test]
    fn ref_reason_names_the_chain_depth() {
        // base -> b -> c is a three-hop alias chain ending at `c`.
        let m = module("let base = 1\nb: base\nc: b\n");
        let model = cost_model(&m, &Budget::default(), &Demand::All, &profile(&m));
        let c = model
            .top
            .iter()
            .find(|n| n.path.as_deref() == Some("c"))
            .expect("c is ranked");
        assert_eq!(c.class, CostClass::Alias, "a `Ref` is class alias");
        assert_eq!(c.reason, "alias chain depth 2", "b is 1 hop from base");
    }

    #[test]
    fn a_cone_ranks_only_its_closure() {
        let m = module("keep:\n  n: 1\nother:\n  m: 2\n");
        let model = cost_model(
            &m,
            &Budget::default(),
            &Demand::Cone("keep".to_string()),
            &profile(&m),
        );
        assert_eq!(model.demand.as_deref(), Some("keep"));
        assert!(
            model.top.iter().all(|n| n
                .path
                .as_deref()
                .is_none_or(|p| p == "keep" || p.starts_with("keep."))),
            "the cone must not rank an undemanded sibling: {:?}",
            model.top.iter().map(|n| &n.path).collect::<Vec<_>>()
        );
        let all = cost_model(&m, &Budget::default(), &Demand::All, &profile(&m));
        assert!(all.demand.is_none());
        assert!(all.top.len() > model.top.len());
    }

    #[test]
    fn a_cone_counts_only_its_closure_units() {
        // A cone's units are the demand's own DP bounds (L-P40 §4.2), so
        // a heavy undemanded sibling must not inflate them. The document
        // root is a closure member whose undemanded child is a
        // non-member, so the root must not absorb the sibling's weight
        // (M7): otherwise the report explains a cost the run never paid.
        let m = module("keep:\n  n: 1\nside:\n  big: \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n");
        let all = cost_model(&m, &Budget::default(), &Demand::All, &profile(&m));
        let cone = cost_model(
            &m,
            &Budget::default(),
            &Demand::Cone("keep".to_string()),
            &profile(&m),
        );
        assert!(
            cone.scope_nodes < all.scope_nodes,
            "the closure is a subset"
        );
        assert!(
            all.top.iter().any(|n| n.charged_units >= 33),
            "the whole-document report carries the 32-byte sibling string"
        );
        let max_units = cone.top.iter().map(|n| n.charged_units).max().unwrap();
        assert!(
            max_units < 33,
            "no cone node may absorb the undemanded sibling: {max_units}"
        );
    }

    #[test]
    fn ranking_is_bounded_and_deterministic() {
        let mut source = String::new();
        for i in 0..(TOP_NODES + 40) {
            source.push_str(&format!("k{i}: {i}\n"));
        }
        let m = module(&source);
        let first = cost_model(&m, &Budget::default(), &Demand::All, &profile(&m));
        let second = cost_model(&m, &Budget::default(), &Demand::All, &profile(&m));
        assert!(first.truncated, "more than the cap ranks as truncated");
        assert_eq!(first.top.len(), TOP_NODES);
        let ids = |model: &CostModel| model.top.iter().map(|n| n.node).collect::<Vec<_>>();
        assert_eq!(ids(&first), ids(&second), "ranking must be deterministic");
        // Every literal is 1 unit, so the tie-break is path then node.
        let paths = first
            .top
            .iter()
            .map(|n| n.path.clone().unwrap_or_default())
            .collect::<Vec<_>>();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted, "equal units break ties by path ascending");
    }

    #[test]
    fn a_material_node_is_sized_at_the_marker_regardless_of_length() {
        // Two documents that differ only in the secret's length. The
        // report must not reveal that length: a material node is sized
        // at the redacted marker, so no charged unit derives from it
        // (LANG-C-010).
        let short = module(
            "let tok = secret(\"secretshort1\")\ncopy = \"secretshort1\"\ntbl = { secretshort1 = 1 }\n",
        );
        let long = module(
            "let tok = secret(\"secretmuchlongervalue1\")\ncopy = \"secretmuchlongervalue1\"\ntbl = { secretmuchlongervalue1 = 1 }\n",
        );
        let a = cost_model(&short, &Budget::default(), &Demand::All, &profile(&short));
        let b = cost_model(&long, &Budget::default(), &Demand::All, &profile(&long));
        assert_eq!(
            a.class_units, b.class_units,
            "the secret's length must not change any charged unit"
        );
        assert_eq!(a.class_counts, b.class_counts);
        let tok = a
            .top
            .iter()
            .find(|n| n.path.as_deref() == Some("tok"))
            .expect("tok is ranked");
        assert_eq!(
            tok.charged_units,
            crate::secret::MARKER_SIZE,
            "a secret node is sized at the marker"
        );
        // Path redaction is a serialization concern, asserted end to end
        // in `active-core/tests/viz.rs` on the dashboard's `cost_model`.
    }
}
