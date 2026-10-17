//! Execution budget tests (C-037, spec/budgets.md).
//!
//! Drives the real shipped path (`parse` → `Engine::evaluate` with a
//! tuned `Budget`) for every accountant limit: node count, value
//! depth, array length, total keys (all deterministic with spans),
//! and the wall-clock backstop through a decade-margin evil regex.
//! Also proves budgets are always fatal (both modes), disablable
//! with zeroes, and invisible to well-formed documents.

use active_core::{parse, Engine, EvalError};

fn engine_with(budget: active_core::Budget, resilient: bool) -> Engine {
    let mut engine = Engine::new();
    engine.budget = budget;
    engine.resilient = resilient;
    engine
}

fn unlimited() -> active_core::Budget {
    active_core::Budget {
        max_nodes: 0,
        max_depth: 0,
        max_array_len: 0,
        max_keys: 0,
        max_wall_ms: 0,
        max_value_size: 0,
        max_total_value_size: 0,
    }
}

/// Evaluate under a budget in both modes: budgets are always fatal,
/// so strict and resilient must Err with the same code. Returns the
/// strict error for assertions.
fn eval_limited(src: &str, budget: active_core::Budget) -> EvalError {
    let module = parse(src, "budgets").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    let strict = engine_with(budget, false)
        .evaluate(&module)
        .expect_err("budget must trip strict");
    let resilient = engine_with(budget, true)
        .evaluate(&module)
        .expect_err("budget must trip resilient");
    assert_eq!(strict.code(), resilient.code(), "both modes agree");
    strict
}

fn xcode(err: &EvalError) -> &str {
    err.code()
}

#[test]
fn node_budget_trips_on_large_documents() {
    let mut src = String::new();
    for i in 0..20 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let budget = active_core::Budget {
        max_nodes: 5,
        ..unlimited()
    };
    let err = eval_limited(&src, budget);
    assert_eq!(xcode(&err), "X001");
}

#[test]
fn depth_budget_trips_with_span() {
    // Six nested tables against a limit of three.
    let src = "a = { b = { c = { d = { e = { f = 1 } } } } }\n";
    let budget = active_core::Budget {
        max_depth: 3,
        ..unlimited()
    };
    let err = eval_limited(src, budget);
    assert_eq!(xcode(&err), "X003");
    assert!(err.span().is_some(), "depth trips carry a span");
}

#[test]
fn array_length_budget_trips_with_span() {
    let src = "xs = [1, 2, 3, 4, 5]\n";
    let budget = active_core::Budget {
        max_array_len: 3,
        ..unlimited()
    };
    let err = eval_limited(src, budget);
    assert_eq!(xcode(&err), "X004");
    assert!(err.span().is_some(), "array trips carry a span");
}

#[test]
fn key_count_budget_trips() {
    let mut src = String::new();
    for i in 0..20 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let budget = active_core::Budget {
        max_keys: 10,
        ..unlimited()
    };
    let err = eval_limited(&src, budget);
    assert_eq!(xcode(&err), "X005");
}

#[test]
fn wall_clock_backstop_trips_on_slow_evaluations() {
    // 50k keys stay far under every count budget but take ~1s to
    // evaluate in debug builds — a three-order margin over the 1ms
    // limit on any machine (spec/budgets.md §3), never a tight
    // timing assertion. (Deliberately NOT an evil regex: the regex
    // crate's automata guarantee makes backtracking linear-time, so
    // patterns cannot burn wall-clock by construction.)
    let mut src = String::with_capacity(1 << 20);
    for i in 0..50_000 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let budget = active_core::Budget {
        max_wall_ms: 1,
        ..unlimited()
    };
    let module = parse(&src, "budgets").expect("must parse");
    let err = engine_with(budget, true)
        .evaluate(&module)
        .expect_err("must trip");
    assert_eq!(xcode(&err), "X002");
}

#[test]
fn zero_disables_every_limit() {
    let mut src = String::new();
    for i in 0..200 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    src.push_str("xs = [");
    for i in 0..200 {
        src.push_str(&format!("{i},"));
    }
    src.push_str("]\n");
    let module = parse(&src, "budgets").expect("must parse");
    engine_with(unlimited(), false)
        .evaluate(&module)
        .expect("unlimited must pass");
}

#[test]
fn shipped_defaults_admit_ordinary_documents() {
    let module = parse("port = 8080\nhost = \"h\"\n", "budgets").expect("must parse");
    Engine::new().evaluate(&module).expect("defaults must pass");
    let mut strict = Engine::new();
    strict.resilient = false;
    strict.evaluate(&module).expect("defaults must pass strict");
}

#[test]
fn reference_expansion_dies_before_materializing() {
    // Billion-laughs shape: thirty doublings demand 2^30 values from
    // a 31-line document. The DP pass must abort (X006) in
    // milliseconds without allocating the monster.
    let mut src = String::from("let a0 = \"x\"\n");
    for i in 1..30 {
        src.push_str(&format!("let a{i} = [a{}, a{}]\n", i - 1, i - 1));
    }
    src.push_str("top = a29\n");
    let budget = active_core::Budget {
        max_value_size: 1_000_000,
        ..unlimited()
    };
    let err = eval_limited(&src, budget);
    assert_eq!(xcode(&err), "X006");
    assert!(err.span().is_some(), "size trips carry a span");
    assert!(err.to_string().contains("1000000"), "{err:?}");
}

#[test]
fn saturation_comparison_holds_at_max_limit() {
    // At max_value_size = u64::MAX the saturating cap still trips on
    // truly astronomical bounds (2^70 doublings) instead of going
    // blind — without materializing anything.
    let mut src = String::from("let a0 = \"x\"\n");
    for i in 1..70 {
        src.push_str(&format!("let a{i} = [a{}, a{}]\n", i - 1, i - 1));
    }
    src.push_str("top = a69\n");
    let budget = active_core::Budget {
        max_value_size: u64::MAX,
        ..unlimited()
    };
    let err = eval_limited(&src, budget);
    assert_eq!(xcode(&err), "X006");
}

#[test]
fn reference_chains_count_toward_depth() {
    // Seventy single-element nestings through lets: the arena is
    // shallow (each statement is flat) but materialized depth is 70.
    let mut src = String::from("let a0 = 1\n");
    for i in 1..70 {
        src.push_str(&format!("let a{i} = [a{}]\n", i - 1));
    }
    src.push_str("top = a69\n");
    let budget = active_core::Budget {
        max_depth: 64,
        ..unlimited()
    };
    let err = eval_limited(&src, budget);
    assert_eq!(xcode(&err), "X003");
}

#[test]
fn dynamic_values_charge_actuals_per_clone() {
    // A 10KB env value referenced twenty times: the DP sees ones,
    // but each clone materializes 10KB — the delta accounting must
    // trip a 50KB cumulative cap. Unique variable: no other test
    // reads it; the prior value is restored afterwards.
    let prior = std::env::var("ACTIVE_TEST_BUDGET_BIG").ok();
    std::env::set_var("ACTIVE_TEST_BUDGET_BIG", "z".repeat(10_000));
    let mut src = String::from("let big = env.ACTIVE_TEST_BUDGET_BIG\n");
    for i in 0..20 {
        src.push_str(&format!("a{i} = big\n"));
    }
    let budget = active_core::Budget {
        max_total_value_size: 50_000,
        ..unlimited()
    };
    let err = eval_limited(&src, budget);
    match prior {
        Some(prev) => std::env::set_var("ACTIVE_TEST_BUDGET_BIG", prev),
        None => std::env::remove_var("ACTIVE_TEST_BUDGET_BIG"),
    }
    assert_eq!(xcode(&err), "X006");
}

#[test]
fn patch_composed_depth_is_checked() {
    // Base and value are each shallow; the patch stacks five fresh
    // levels under one key, past a limit of four.
    let src = "base = { a = 1 }\n@patch(\"base.b.c.d.e.f\", 1)\n";
    let budget = active_core::Budget {
        max_depth: 4,
        ..unlimited()
    };
    let err = eval_limited(src, budget);
    assert_eq!(xcode(&err), "X003");
}

#[test]
fn wall_clock_deadline_never_panics_on_extremes() {
    // Absurd configured durations must not panic on Instant overflow
    // (checked_add degrades to no deadline where the platform would
    // overflow; elsewhere the deadline is simply far away). Either
    // way, ordinary evaluation passes.
    let budget = active_core::Budget {
        max_wall_ms: u64::MAX,
        ..unlimited()
    };
    let _ = budget.wall_deadline();
    let module = parse("x = 1\n", "budgets").expect("must parse");
    engine_with(budget, true)
        .evaluate(&module)
        .expect("must pass");
}

#[test]
fn over_depth_spans_are_stable_across_runs() {
    // Two over-depth bindings: deterministic order must report the
    // same span every run (topological order, never HashMap order).
    let src = "let a = { x = { y = { z = 1 } } }\nlet b = { x = { y = { z = 2 } } }\n";
    let budget = active_core::Budget {
        max_depth: 2,
        ..unlimited()
    };
    let run = || {
        let module = parse(src, "budgets").expect("must parse");
        engine_with(budget, false)
            .evaluate(&module)
            .expect_err("must trip")
            .span()
            .map(|s| (s.line, s.column))
    };
    assert_eq!(run(), run());
}

#[test]
fn cumulative_total_trips_on_many_aliases() {
    // Two hundred aliases of a 1KB string: every node is far below
    // any per-node limit, but the materialized total (~200KB) passes
    // a 100KB cumulative cap. Per-node accounting alone would miss it.
    let big = "x".repeat(1000);
    let mut src = format!("let big = \"{big}\"\n");
    for i in 0..200 {
        src.push_str(&format!("a{i} = big\n"));
    }
    let budget = active_core::Budget {
        max_total_value_size: 100_000,
        ..unlimited()
    };
    let err = eval_limited(&src, budget);
    assert_eq!(xcode(&err), "X006");
    assert!(err.to_string().contains("100000"), "{err:?}");
}

#[test]
fn patch_clones_charge_the_cumulative_total() {
    // Three patches cloning one 10KB value: the DP total stays under
    // 50KB (one definition plus three references), but the three
    // patch-time clones push the cumulative sum past it. Per-node
    // accounting alone would miss the second copy of every clone.
    let big = "y".repeat(10_000);
    let mut src = format!("let big = \"{big}\"\nbase = {{}}\n");
    for i in 0..3 {
        src.push_str(&format!("@patch(\"base.k{i}\", big)\n"));
    }
    let budget = active_core::Budget {
        max_total_value_size: 50_000,
        ..unlimited()
    };
    let err = eval_limited(&src, budget);
    assert_eq!(xcode(&err), "X006");
}

#[test]
fn expired_deadline_takes_precedence_in_strict_validation() {
    // Twenty-five thousand violating array elements with a 1ms limit:
    // validation cannot finish in time, so the budget error must win
    // over the collected violations (never masked by
    // ValidationFailed). Debug-scale margins make this deterministic
    // in practice (tens of ms of validation against a 1ms limit).
    let mut src = String::from("schema = { xs = Array<Int(min = 0)> }\nxs = [");
    for _ in 0..25_000 {
        src.push_str("-1,");
    }
    src.push_str("]\n");
    let budget = active_core::Budget {
        max_wall_ms: 1,
        ..unlimited()
    };
    let module = parse(&src, "budgets").expect("must parse");
    let err = engine_with(budget, false)
        .evaluate(&module)
        .expect_err("must fail");
    assert_eq!(xcode(&err), "X002", "{err:?}");
}
