//! Progressive warnings survive a budget trip (LANG-C-009).
//!
//! Data Mode documents get W013 "approaching a ceiling" warnings at 80% of
//! any static limit. A wall-clock trip used to replace the whole diagnostic
//! list with the error, so an agent watching W013 would only ever see it on a
//! fast host. The warnings now travel on the error itself.

use active_core::{Budget, Engine, EvalError, Mode};

/// A Data Mode document whose static shape sits at ~80-90% of `max_nodes`
/// and `max_keys`: 8,000 keys is 80% of a 10,000-key limit.
fn near_the_ceiling(keys: usize) -> String {
    let mut src = String::from("active_mode = \"data\"\n");
    for i in 0..keys {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    src
}

#[test]
fn a_tripped_walk_still_carries_its_progressive_warnings() {
    let src = near_the_ceiling(8_000);
    let module = active_core::parse(&src, "w013").expect("parse");
    assert_eq!(module.mode, Mode::Data);

    // Small limits put the shape at 80% while the walk is forced to trip on
    // the wall clock: a 1 ms budget cannot finish a 16k-node walk in debug.
    let budget = Budget {
        max_nodes: 10_000,
        max_keys: 10_000,
        max_wall_ms: 1,
        ..Budget::default()
    };
    let mut engine = Engine::hermetic();
    engine.budget = budget;
    let err = engine.evaluate(&module).expect_err("the walk must trip");
    assert_eq!(err.code(), "X002", "{err:?}");
    let carried: Vec<&str> = err
        .attached_warnings()
        .iter()
        .map(|d| d.code.as_str())
        .collect();
    assert!(
        carried.contains(&"W013"),
        "the trip must carry the progressive warnings it replaced: {err:?} {carried:?}"
    );
    assert!(
        err.attached_warnings().iter().all(|d| d.code == "W013"),
        "only the progressive warnings travel: {carried:?}"
    );

    // Control: the same document under a generous clock completes and
    // reports the same W013s in its ordinary diagnostic list, so the test
    // proves the warnings existed rather than inventing them.
    let mut quiet = Engine::hermetic();
    quiet.budget = Budget {
        max_wall_ms: active_core::eval::UNLIMITED,
        ..budget
    };
    let (_, diags) = quiet
        .evaluate(&module)
        .expect("under the ceiling: completes");
    let ordinary: Vec<&str> = diags.iter().map(|d| d.code.as_str()).collect();
    assert!(ordinary.contains(&"W013"), "{ordinary:?}");
    assert_eq!(
        err.attached_warnings().len(),
        diags.iter().filter(|d| d.code == "W013").count(),
        "the carried list is the same list the walk had collected"
    );
}

#[test]
fn a_clean_evaluation_has_no_attached_warnings() {
    let module = active_core::parse("port = 1\n", "clean").expect("parse");
    let (_, diags) = Engine::hermetic()
        .evaluate(&module)
        .expect("clean evaluates");
    assert!(diags.is_empty());
    let err = EvalError::CycleDetected;
    assert!(err.attached_warnings().is_empty());
}
