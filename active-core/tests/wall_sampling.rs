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
        max_total_value_size: active_core::eval::UNLIMITED,
        max_value_size: active_core::eval::UNLIMITED,
        max_nodes: active_core::eval::UNLIMITED,
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
            max_wall_ms: active_core::eval::UNLIMITED,
            // Same lifting as the timed run: this control proves the
            // DOCUMENT is legal, with time as the only bound in play.
            max_total_value_size: active_core::eval::UNLIMITED,
            max_value_size: active_core::eval::UNLIMITED,
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

#[test]
fn canonical_passes_use_one_finite_wall_budget() {
    // LANG-C-008: audit, lock, replay, minimizer, and language-server step
    // passes must not inherit a caller's clock (a document that just passed
    // could fail the second pass on load) and must not run without one
    // (the amp fixtures returned multi-megabyte outputs with max_wall_ms =
    // 0). Every one of those surfaces calls this function.
    let base = Budget {
        max_nodes: 7,
        max_depth: 9,
        max_array_len: 11,
        max_keys: 13,
        max_wall_ms: 0,
        max_value_size: 17,
        max_total_value_size: 19,
    };
    let canonical = active_core::eval::canonical_budget(&base);
    assert_eq!(canonical.max_wall_ms, active_core::eval::CANONICAL_WALL_MS);
    assert_eq!(canonical.max_wall_ms, 30_000);
    assert_eq!(canonical.max_wall_ms, Budget::trusted().max_wall_ms);
    // Only the clock moves; every other limit is the caller's.
    assert_eq!(canonical.max_nodes, base.max_nodes);
    assert_eq!(canonical.max_depth, base.max_depth);
    assert_eq!(canonical.max_array_len, base.max_array_len);
    assert_eq!(canonical.max_keys, base.max_keys);
    assert_eq!(canonical.max_value_size, base.max_value_size);
    assert_eq!(canonical.max_total_value_size, base.max_total_value_size);
    // A caller's finite clock is replaced too: one value everywhere.
    let timed = Budget {
        max_wall_ms: 100,
        ..base
    };
    assert_eq!(
        active_core::eval::canonical_budget(&timed).max_wall_ms,
        active_core::eval::CANONICAL_WALL_MS
    );
}

#[test]
fn no_shipped_surface_clears_the_wall_budget() {
    // Structural guard for LANG-C-008: the fix replaced every
    // `max_wall_ms = 0` assignment on the audit/lock/replay/minimizer/LSP
    // surfaces with `canonical_budget`. A new assignment would reintroduce
    // the unbounded pass this finding closed.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut offenders = Vec::new();
    for crate_dir in ["active-core/src", "active-cli/src", "active-lsp/src"] {
        let base = root.join(crate_dir);
        let mut stack = vec![base];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for (index, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    if code.contains("max_wall_ms = 0") {
                        offenders.push(format!("{}:{}", path.display(), index + 1));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "surfaces must use eval::canonical_budget instead of clearing the clock: {offenders:?}"
    );
}

#[test]
fn canonical_budget_bounds_an_unlimited_base_in_every_dimension() {
    // Review finding on LANG-C-008: setting only the wall clock left the
    // counts/sizes at UNLIMITED, so a huge arena could exhaust memory
    // before the clock fired. The canonical pass now bounds every
    // dimension: unlimited fields become the shipped default, tighter
    // caller limits are kept.
    use active_core::eval::{canonical_budget, Budget, CANONICAL_WALL_MS};
    let shipped = Budget::default();
    let from_unlimited = canonical_budget(&Budget::unlimited());
    assert_eq!(from_unlimited.max_wall_ms, CANONICAL_WALL_MS);
    assert_eq!(from_unlimited.max_nodes, shipped.max_nodes);
    assert_eq!(from_unlimited.max_depth, shipped.max_depth);
    assert_eq!(from_unlimited.max_value_size, shipped.max_value_size);
    assert_eq!(
        from_unlimited.max_total_value_size,
        shipped.max_total_value_size
    );

    let tighter = Budget {
        max_nodes: 10,
        max_wall_ms: 5,
        ..Budget::unlimited()
    };
    let canonical = canonical_budget(&tighter);
    assert_eq!(canonical.max_nodes, 10, "a tighter limit is kept");
    assert_eq!(canonical.max_wall_ms, CANONICAL_WALL_MS);
}

#[test]
fn the_unlimited_sentinel_is_the_only_value_without_a_deadline() {
    use active_core::eval::{Budget, UNLIMITED};
    assert!(Budget::default().wall_deadline().is_some());
    assert!(Budget::unlimited().wall_deadline().is_none());
    // A raw zero (never produced by Budget::resolve at an evaluation
    // boundary) also means "no deadline" rather than a zero-length one.
    let raw = Budget {
        max_wall_ms: 0,
        ..Budget::default()
    };
    assert!(raw.wall_deadline().is_none());
    assert!(UNLIMITED > Budget::default().max_wall_ms);
}
