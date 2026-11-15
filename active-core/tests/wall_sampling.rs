//! Wall-clock sampling on materialised bytes (LANG-C-006).
//!
//! The walk sampled the deadline every 64 nodes, which few-node/huge-work
//! documents never reach: an 8-level environment doubling has 27 nodes and
//! spent 579-1,756 ms against a 100 ms limit before X002 fired (and only at
//! the closing sample). The accountant now also compares against the
//! deadline after every `WALL_SAMPLE_BYTES` (1 MiB) it charges, and the Ref
//! arm charges BEFORE cloning, so the trip lands mid-walk and before the
//! allocation it would have paid for.

use active_core::{Budget, Engine, EvalError, Value};

/// A document whose arena stays far below the 64-node walk sample while
/// each alias materialises megabytes: `s` reads a large host value and a
/// handful of aliases clone it.
fn aliased_host_value(
    aliases: usize,
    value_bytes: usize,
) -> (String, std::collections::HashMap<String, String>) {
    let mut src = String::from("s = env.HUGE\n");
    for i in 0..aliases {
        src.push_str(&format!("a{i} = s\n"));
    }
    let mut env = std::collections::HashMap::new();
    env.insert("HUGE".to_string(), "x".repeat(value_bytes));
    (src, env)
}

fn engine_with_budget(budget: Budget, env: std::collections::HashMap<String, String>) -> Engine {
    let mut engine = Engine::hermetic().with_env(env);
    engine.budget = budget;
    engine
}

#[test]
fn a_few_node_document_trips_x002_mid_walk_on_bytes_not_nodes() {
    let (src, env) = aliased_host_value(8, 8 * 1024 * 1024);
    let module = active_core::parse(&src, "amp").expect("parse");
    assert!(
        module.arena.len() < 64,
        "the document must stay below the node-sampling interval, arena is {}",
        module.arena.len()
    );

    let budget = Budget {
        max_wall_ms: 1,
        // Isolate the clock: sizes are unlimited, so only X002 can fire.
        max_total_value_size: 0,
        max_value_size: 0,
        max_nodes: 0,
        ..Budget::default()
    };
    let engine = engine_with_budget(budget, env.clone());
    let started = std::time::Instant::now();
    let err = engine
        .evaluate(&module)
        .expect_err("the byte-driven wall sample must trip");
    let elapsed = started.elapsed();
    match &err {
        EvalError::BudgetExceeded { kind, span, .. } => {
            assert_eq!(err.code(), "X002", "{err:?}");
            assert_eq!(format!("{kind}"), "time budget", "{err:?}");
            assert!(
                span.is_some(),
                "the trip must be attributed to a node mid-walk, not the closing sample: {err:?}"
            );
        }
        other => panic!("expected a budget trip, got {other:?}"),
    }
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the trip must land promptly, took {elapsed:?}"
    );

    // Negative control: the same document and host value with the clock
    // disabled evaluates (the document is legal; only time was the issue).
    let quiet = engine_with_budget(
        Budget {
            max_wall_ms: 0,
            // Same lifting as the timed run: this control proves the
            // DOCUMENT is legal, with time as the only bound in play.
            max_total_value_size: 0,
            max_value_size: 0,
            ..Budget::default()
        },
        env,
    );
    let (value, _) = quiet.evaluate(&module).expect("clock off: evaluates");
    let Value::Table(root) = value else {
        panic!("root must be a table")
    };
    assert_eq!(root.len(), 9, "every alias materialises");
}

#[test]
fn byte_sampling_does_not_trip_a_document_that_finishes_in_time() {
    // Control for the sample interval itself: a small value cloned a few
    // times stays far below one 1 MiB sample and under a generous clock.
    let (src, env) = aliased_host_value(5, 1024);
    let module = active_core::parse(&src, "small").expect("parse");
    let engine = engine_with_budget(
        Budget {
            max_wall_ms: 2_000,
            ..Budget::default()
        },
        env,
    );
    let (value, _) = engine.evaluate(&module).expect("small document evaluates");
    assert!(matches!(value, Value::Table(_)));
}

#[test]
fn the_sample_interval_is_one_mebibyte_of_charged_bytes() {
    // The constant is part of the contract the tests above rely on: a trip
    // lands within one interval of charged work past the deadline.
    assert_eq!(
        active_core::eval::WALL_SAMPLE_BYTES,
        1024 * 1024,
        "byte-driven wall sampling interval"
    );
}
