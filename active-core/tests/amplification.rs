//! Dynamic-value amplification is charged (LANG-C-001, ordering-gate item 1).
//!
//! A small document can build a huge value by doubling an environment
//! string and aliasing it. The DP pass can only bound static shapes, so the
//! evaluator now records each node's real size and charges the growth at the
//! producer plus the delta at every clone. These tests drive the shipped
//! `Engine` and assert the budget trips deterministically — never a
//! multi-megabyte result under a small ceiling.

use active_core::{parse, Budget, Engine};

fn engine_with_total(limit: u64) -> Engine {
    let mut engine = Engine::new();
    engine.budget = Budget {
        max_total_value_size: limit,
        ..Budget::default()
    };
    // Deterministic: no wall component. Zero means the shipped
    // default under Budget::resolve, so the explicit sentinel lifts it.
    engine.budget.max_wall_ms = active_core::eval::UNLIMITED;
    engine
}

fn amplification_source(depth: usize) -> String {
    let mut lines = vec!["seed = env.AMP_BIG".to_string(), "a0 = seed".to_string()];
    for i in 1..=depth {
        lines.push(format!("a{i} = \"${{a{}}} ${{a{}}}\"", i - 1, i - 1));
    }
    lines.join("\n") + "\n"
}

#[test]
fn doubling_an_env_string_trips_the_total_size_budget() {
    let src = amplification_source(12);
    let module = parse(&src, "amp").expect("parse");
    let mut engine = engine_with_total(100_000);
    engine.env.insert("AMP_BIG".to_string(), "E".repeat(1024));
    let err = engine.evaluate(&module).expect_err("must trip the budget");
    assert_eq!(err.code(), "X006", "{err}");
}

#[test]
fn cloning_a_large_dynamic_string_trips_the_budget() {
    // The clone path: one big dynamic value aliased many times.
    let mut lines = vec![
        "seed = env.AMP_BIG".to_string(),
        "base = \"${seed}${seed}${seed}${seed}\"".to_string(),
    ];
    for i in 0..64 {
        lines.push(format!("a{i} = base"));
    }
    lines.push(format!(
        "arr = [{}]",
        (0..64)
            .map(|i| format!("a{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let module = parse(&(lines.join("\n") + "\n"), "amp").expect("parse");
    let mut engine = engine_with_total(100_000);
    engine.env.insert("AMP_BIG".to_string(), "E".repeat(1024));
    let err = engine.evaluate(&module).expect_err("must trip the budget");
    assert_eq!(err.code(), "X006", "{err}");
}

#[test]
fn ordinary_documents_stay_within_a_generous_budget() {
    let module = parse("a = \"hello\"\nb = \"${a} world\"\n", "amp").expect("parse");
    let engine = engine_with_total(1_000_000);
    let (value, _d) = engine.evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("hello world"), "{json}");
}
