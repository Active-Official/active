//! Property-based invariant tests (C-011).
//!
//! Drives the real evaluator and export functions on randomly generated
//! IR graphs and value trees, asserting the three evaluation-model
//! invariants from the production proposal:
//! 1. evaluation is deterministic (same graph → same result, always),
//! 2. a cycle is always detected before any value is produced,
//! 3. secret values never appear in JSON or TOML export.

use indexmap::IndexMap;
use proptest::prelude::*;
use solid_core::convert::{value_to_pretty_json, value_to_simple_toml};
use solid_core::diag::Diagnostic;
use solid_core::eval::EvalError;
use solid_core::ir::{BinOp, Module, Node, NodeId, Value};
use solid_core::Engine;

/// Tiny deterministic xorshift PRNG — test scaffolding only.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// Build a random acyclic IR graph of 2–9 nodes. Every node depends only
/// on earlier nodes, and the last node always references node 0 — so a
/// back-edge `last → 0` is guaranteed to create a cycle.
fn build_random_dag(seed: u64) -> (Module, Vec<NodeId>) {
    let mut rng = Rng(seed | 1);
    let n = 2 + rng.below(8) as usize;
    let mut module = Module::new("proptest");
    let mut ids: Vec<NodeId> = Vec::with_capacity(n);

    for i in 0..n {
        let pick =
            |rng: &mut Rng, ids: &[NodeId]| -> NodeId { ids[rng.below(ids.len() as u64) as usize] };
        let mut deps: Vec<NodeId> = Vec::new();
        let node = if i == 0 {
            Node::Literal(Value::Int(rng.below(1000) as i64))
        } else if i == n - 1 {
            // Last node: an array that includes node 0, guaranteeing the
            // edge 0 → last exists for the cycle test.
            let mut els = vec![NodeId(0)];
            for _ in 0..1 + rng.below(2) {
                let d = pick(&mut rng, &ids);
                els.push(d);
                deps.push(d);
            }
            deps.push(NodeId(0));
            Node::Array { elements: els }
        } else {
            match rng.below(5) {
                0 => Node::Literal(Value::Int(rng.below(1000) as i64)),
                1 => Node::Literal(Value::String(format!("s{i}"))),
                2 => Node::Literal(Value::Bool(rng.below(2) == 0)),
                3 => {
                    let op = match rng.below(4) {
                        0 => BinOp::Add,
                        1 => BinOp::Sub,
                        2 => BinOp::Mul,
                        _ => BinOp::Div,
                    };
                    let lhs = pick(&mut rng, &ids);
                    let rhs = pick(&mut rng, &ids);
                    deps.push(lhs);
                    deps.push(rhs);
                    Node::Binary { op, lhs, rhs }
                }
                _ => {
                    let mut fields = IndexMap::new();
                    for j in 0..1 + rng.below(3) {
                        let d = pick(&mut rng, &ids);
                        deps.push(d);
                        fields.insert(format!("f{j}"), d);
                    }
                    Node::Table { fields }
                }
            }
        };
        let id = module.add_node(node);
        for d in &deps {
            module.add_edge(*d, id);
        }
        ids.push(id);
    }
    module.root = Some(*ids.last().unwrap());
    (module, ids)
}

/// Render an evaluation result deterministically for comparison.
fn render(result: Result<(Value, Vec<Diagnostic>), EvalError>) -> String {
    match result {
        Ok((v, diags)) => format!("ok|{}|{diags:?}", value_to_pretty_json(&v)),
        Err(e) => format!("err|{e}"),
    }
}

/// Build a random value tree that always contains at least one secret.
fn build_value_with_secret(seed: u64, secret: &str, depth: u32) -> Value {
    fn gen(rng: &mut Rng, secret: &str, depth: u32) -> Value {
        if depth == 0 {
            return match rng.below(4) {
                0 => Value::Secret(secret.to_string()),
                1 => Value::Int(rng.below(100) as i64),
                2 => Value::String(format!("plain{}", rng.below(100))),
                _ => Value::Bool(rng.below(2) == 0),
            };
        }
        match rng.below(5) {
            0 => Value::Secret(secret.to_string()),
            1 => Value::Int(rng.below(100) as i64),
            2 => Value::String(format!("plain{}", rng.below(100))),
            3 => Value::Array(
                (0..1 + rng.below(3))
                    .map(|_| gen(rng, secret, depth - 1))
                    .collect(),
            ),
            _ => {
                let mut table = IndexMap::new();
                for j in 0..1 + rng.below(3) {
                    table.insert(format!("k{j}"), gen(rng, secret, depth - 1));
                }
                Value::Table(table)
            }
        }
    }
    let mut rng = Rng(seed | 1);
    let mut root = IndexMap::new();
    root.insert("payload".to_string(), gen(&mut rng, secret, depth));
    root.insert("credential".to_string(), Value::Secret(secret.to_string()));
    Value::Table(root)
}

proptest! {
    /// Invariant 1: evaluating the same graph twice always yields the
    /// same value tree and the same diagnostics.
    #[test]
    fn evaluation_is_deterministic(seed in any::<u64>()) {
        let (module, _) = build_random_dag(seed);
        let engine = Engine::new();
        let first = render(engine.evaluate(&module));
        let second = render(engine.evaluate(&module));
        prop_assert_eq!(first, second, "evaluation of the same graph differed between runs");
    }

    /// Invariant 2a: a cyclic graph always fails with CycleDetected —
    /// before any value is produced (evaluate returns Err, no partial
    /// values escape).
    #[test]
    fn cycle_is_detected_before_any_value(seed in any::<u64>()) {
        let (mut module, ids) = build_random_dag(seed);
        let last = *ids.last().unwrap();
        module.add_edge(last, NodeId(0)); // node 0 now depends on last
        let engine = Engine::new();
        let result = engine.evaluate(&module);
        prop_assert!(
            matches!(result, Err(EvalError::CycleDetected)),
            "cyclic graph must fail with CycleDetected, got {result:?}"
        );
    }

    /// Invariant 2b: an acyclic graph never reports a cycle.
    #[test]
    fn acyclic_graph_never_reports_cycle(seed in any::<u64>()) {
        let (module, _) = build_random_dag(seed);
        let engine = Engine::new();
        if let Err(e) = engine.evaluate(&module) {
            prop_assert!(
                !matches!(e, EvalError::CycleDetected),
                "acyclic graph must never report a cycle, got {e:?}"
            );
        }
    }

    /// Invariant 3: secret values never appear in JSON or TOML export,
    /// and the redaction marker is always present.
    #[test]
    fn secrets_never_leak_into_exports(seed in any::<u64>(), secret in "[a-z]{6,16}") {
        let value = build_value_with_secret(seed, &secret, 3);
        let json = value_to_pretty_json(&value);
        let toml = value_to_simple_toml(&value, "");
        prop_assert!(!json.contains(&secret), "secret leaked into JSON: {json}");
        prop_assert!(!toml.contains(&secret), "secret leaked into TOML: {toml}");
        prop_assert!(json.contains("***SECRET***"), "redaction marker missing from JSON: {json}");
    }
}
