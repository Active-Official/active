//! Incremental evaluation tests (C-055, spec/performance.md §4).
//!
//! Drives the real shipped path (`parse` → `evaluate_incremental`)
//! and asserts the testing contract: a 10k-node document evaluates
//! correctly with spot-checked values; a leaf edit recomputes
//! O(chain depth) nodes instead of N; env/secret/interpolation
//! edits invalidate exactly the affected cone; profile and budget
//! switches clear the memo; a steady-state re-evaluation serves
//! everything from the memo with zero recomputes.

use active_core::{parse, Engine, IncrementalMemo, Value};
use std::collections::HashMap;

/// 50 tables × 200 chained keys, mirroring `benches/eval.rs` so the
/// contract test and the tracked benchmark measure the same shape.
fn doc_10k() -> String {
    let mut out = String::new();
    for t in 0..50 {
        out.push_str(&format!("[t{t}]\n"));
        out.push_str(&format!("k0 = {t}\n"));
        for k in 1..200 {
            out.push_str(&format!("k{k} = k{} + 1\n", k - 1));
        }
    }
    out
}

fn nested<'v>(value: &'v Value, table: &str, key: &str) -> &'v Value {
    match value {
        Value::Table(map) => match map.get(table) {
            Some(Value::Table(inner)) => inner
                .get(key)
                .unwrap_or_else(|| panic!("{table}.{key} missing")),
            other => panic!("table {table} missing: {other:?}"),
        },
        other => panic!("root table expected: {other:?}"),
    }
}

fn int(value: &Value) -> i64 {
    match value {
        Value::Int(n) => *n,
        other => panic!("int expected: {other:?}"),
    }
}

fn json(value: &Value) -> String {
    active_core::convert::value_to_pretty_json(value)
}

fn engine_10k() -> Engine {
    // Debug-profile test builds evaluate ~10× slower than the
    // optimized profile criterion measures; the wall clock is a
    // production backstop, not what these tests assert (counts and
    // values), so the 10k tests lift it. Node/depth/size budgets
    // stay at their shipped defaults.
    let mut engine = Engine::new();
    engine.budget.max_wall_ms = 0;
    engine
}

#[test]
fn incremental_matches_full_evaluation_on_10k() {
    let src = doc_10k();
    let module = parse(&src, "incremental").expect("must parse");
    let engine = engine_10k();

    let (full, _) = engine.evaluate(&module).expect("must evaluate");
    let mut memo = IncrementalMemo::default();
    let (incr, _) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    // Same input, same value tree — byte for byte.
    assert_eq!(json(&incr), json(&full));
    // Spot-check first, middle, and last chains.
    assert_eq!(int(nested(&incr, "t0", "k199")), 199);
    assert_eq!(int(nested(&incr, "t25", "k199")), 25 + 199);
    assert_eq!(int(nested(&incr, "t49", "k199")), 49 + 199);
    // First call populates: every chain step evaluates. The only
    // hits are the structurally identical `1` literals shared by
    // all 50 chains (199 each) — same fingerprint, same value, so
    // serving them memoized is sound. The lower bound tolerates
    // walk-order luck (whichever occurrence stores first).
    assert!(memo.last_memo_hits >= 9_751, "{}", memo.last_memo_hits);
    assert!(memo.last_eval_count >= 10_000, "{}", memo.last_eval_count);
}

#[test]
fn leaf_edit_recomputes_only_the_affected_chain() {
    let src = doc_10k();
    let module = parse(&src, "incremental").expect("must parse");
    let engine = engine_10k();
    let mut memo = IncrementalMemo::default();
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    let full_count = memo.last_eval_count;
    let first_hits = memo.last_memo_hits;

    // Edit to a value no other chain holds: the whole edited
    // chain's fingerprints are novel, so invalidation must travel
    // its full depth (a small edit like 0→1 would alias t1's
    // identical chain and prove nothing about propagation).
    let edited = src.replacen("k0 = 0\n", "k0 = 100000\n", 1);
    assert_ne!(edited, src, "fixture must contain the leaf");
    let module2 = parse(&edited, "incremental").expect("must parse");
    let (value, _) = engine
        .evaluate_incremental(&module2, &mut memo)
        .expect("must evaluate");

    // The edited chain moved by exactly the delta; others did not.
    assert_eq!(int(nested(&value, "t0", "k199")), 100_199);
    assert_eq!(int(nested(&value, "t1", "k199")), 1 + 199);
    assert_eq!(int(nested(&value, "t49", "k199")), 49 + 199);
    // Sub-linear: one 200-deep chain re-evaluates out of 10k nodes.
    assert!(
        memo.last_eval_count * 15 < full_count,
        "recomputed {} of {full_count}",
        memo.last_eval_count
    );
    // Exact coverage: every walked node either hits or evaluates,
    // so the two calls account for the same walk.
    assert_eq!(
        memo.last_memo_hits + memo.last_eval_count,
        full_count + first_hits,
    );
}

#[test]
fn edit_aliasing_an_existing_value_serves_shared_memo() {
    // Companion to the propagation test above: editing t0's leaf
    // from 0 to 1 makes its chain structurally identical to t1's
    // (1..200), so nearly every node is served from the memo and
    // the values are still exactly right. Same fingerprint really
    // means same value — across tables, not just across calls.
    let src = doc_10k();
    let module = parse(&src, "incremental").expect("must parse");
    let engine = engine_10k();
    let mut memo = IncrementalMemo::default();
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");

    let edited = src.replacen("k0 = 0\n", "k0 = 1\n", 1);
    let module2 = parse(&edited, "incremental").expect("must parse");
    let (value, _) = engine
        .evaluate_incremental(&module2, &mut memo)
        .expect("must evaluate");
    assert_eq!(int(nested(&value, "t0", "k199")), 200);
    assert_eq!(int(nested(&value, "t1", "k199")), 200);
    assert!(
        memo.last_eval_count <= 2,
        "aliased chain must hit: {}",
        memo.last_eval_count
    );
}

#[test]
fn steady_state_reevaluation_serves_everything_from_memo() {
    let src = "a = 1\nb = a + 1\n[c]\nd = \"${b}!\"\n";
    let module = parse(src, "incremental").expect("must parse");
    let engine = Engine::new();
    let mut memo = IncrementalMemo::default();
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert!(memo.last_eval_count > 0);
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert_eq!(
        memo.last_eval_count, 0,
        "nothing changed, nothing recomputes"
    );
    assert!(memo.last_memo_hits > 0);
}

#[test]
fn env_change_invalidates_only_readers() {
    let src = "a = env.ACTIVE_INC_A ?: \"x\"\nb = a\nc = 1\nd = c\n";
    let module = parse(src, "incremental").expect("must parse");
    let mut memo = IncrementalMemo::default();
    let empty: HashMap<String, String> = HashMap::new();
    let (first, _) = Engine::new()
        .with_env(empty)
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    let full_count = memo.last_eval_count;
    assert!(json(&first).contains("\"x\""), "{}", json(&first));

    let changed = HashMap::from([("ACTIVE_INC_A".to_string(), "y".to_string())]);
    let (second, _) = Engine::new()
        .with_env(changed)
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    let rendered = json(&second);
    assert!(rendered.contains("\"y\""), "{rendered}");
    assert!(!rendered.contains("\"x\""), "{rendered}");
    // The env node and its dependent re-evaluate; the `c`/`d`
    // cone keeps its memoized values.
    assert!(
        memo.last_eval_count < full_count,
        "recomputed {} of {full_count}",
        memo.last_eval_count
    );
}

#[test]
fn interpolation_edit_propagates_through_dependents_only() {
    let src = "name = \"a\"\ngreeting = \"hi ${name}\"\nother = 1\n";
    let module = parse(src, "incremental").expect("must parse");
    let engine = Engine::new();
    let mut memo = IncrementalMemo::default();
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    let full_count = memo.last_eval_count;

    let edited = src.replace("name = \"a\"", "name = \"b\"");
    let module2 = parse(&edited, "incremental").expect("must parse");
    let (value, _) = engine
        .evaluate_incremental(&module2, &mut memo)
        .expect("must evaluate");
    let rendered = json(&value);
    assert!(rendered.contains("hi b"), "{rendered}");
    assert!(memo.last_eval_count < full_count);
    assert!(memo.last_memo_hits > 0, "untouched nodes must hit");
}

#[test]
fn secret_edit_propagates_and_stays_redacted() {
    let src = "pw = \"s3cr3t-1\"\ntoken = secret(pw)\nplain = 1\n";
    let module = parse(src, "incremental").expect("must parse");
    let engine = Engine::new();
    let mut memo = IncrementalMemo::default();
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");

    let edited = src.replace("s3cr3t-1", "s3cr3t-2");
    let module2 = parse(&edited, "incremental").expect("must parse");
    let (value, _) = engine
        .evaluate_incremental(&module2, &mut memo)
        .expect("must evaluate");
    let rendered = json(&value);
    // The tainted copy stays redacted on every export path; the
    // operator's own plain-text locator still exports under its own
    // key by design, so the test pins redaction to `token`, not to
    // the whole document.
    assert!(
        rendered.contains("\"token\": \"***SECRET***\""),
        "{rendered}"
    );
    match &value {
        Value::Table(map) => match map.get("token") {
            Some(Value::Secret(s)) => assert_eq!(s, "s3cr3t-2"),
            other => panic!("secret expected: {other:?}"),
        },
        other => panic!("table expected: {other:?}"),
    }
}

#[test]
fn profile_switch_clears_the_memo() {
    let src = "active_profile = \"dev\"\nbase = 1\nprofile \"dev\" {\nbase = 2\n}\n";
    let module = parse(src, "incremental").expect("must parse");
    let mut memo = IncrementalMemo::default();
    let mut dev = Engine::new();
    dev.profile_override = Some("dev".to_string());
    let (first, _) = dev
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    let full_count = memo.last_eval_count;
    assert!(full_count > 0);

    // Same profile: steady state, zero recomputes.
    dev.evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert_eq!(memo.last_eval_count, 0);

    // Different profile: config token changes, the store clears,
    // and the walk evaluates everything again.
    let mut prod = Engine::new();
    prod.profile_override = Some("other".to_string());
    let (second, _) = prod
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert_eq!(memo.last_eval_count, full_count);
    assert_ne!(json(&first), json(&second));
}

#[test]
fn budget_and_resilient_changes_clear_the_memo() {
    // The config token is (resilient, profile, budget) compared by
    // value: touching any of them re-evaluates everything, so a
    // tightened budget or a strictness flip can never serve values
    // vetted under looser rules.
    let src = "a = 1\nb = a\n";
    let module = parse(src, "incremental").expect("must parse");
    let mut memo = IncrementalMemo::default();
    let mut engine = Engine::new();
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    let full_count = memo.last_eval_count;
    assert!(full_count > 0);
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert_eq!(memo.last_eval_count, 0);

    engine.budget.max_wall_ms = 30_000;
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert_eq!(memo.last_eval_count, full_count);

    engine.resilient = false;
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert_eq!(memo.last_eval_count, full_count);
}

#[test]
fn moved_ident_serves_the_current_span() {
    // Regression test for the W001 stale-span bug: an unresolved
    // identifier moved across lines must report its CURRENT line
    // under incremental evaluation, exactly as a full evaluation
    // does. Memoized diagnostics carry positions from an earlier
    // parse; the hit path re-anchors them (eval/mod.rs).
    let first = "x = nosuch\n";
    let module = parse(first, "incremental").expect("must parse");
    let engine = Engine::new();
    let mut memo = IncrementalMemo::default();
    let (_, diags1) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert_eq!(diags1.iter().filter(|d| d.code == "W001").count(), 1);
    assert_eq!(diags1[0].span.as_ref().map(|s| s.line), Some(1));

    let moved = "\n\nx = nosuch\n";
    let module2 = parse(moved, "incremental").expect("must parse");
    let (_, diags2) = engine
        .evaluate_incremental(&module2, &mut memo)
        .expect("must evaluate");
    let (_, fresh) = engine.evaluate(&module2).expect("must evaluate");
    assert_eq!(diags2.len(), fresh.len(), "{diags2:?} vs {fresh:?}");
    for (served, expected) in diags2.iter().zip(fresh.iter()) {
        assert_eq!(served.code, expected.code);
        assert_eq!(
            served.span.as_ref().map(|s| (s.line, s.column)),
            expected.span.as_ref().map(|s| (s.line, s.column)),
            "stale span served: {served:?}"
        );
    }
    assert_eq!(
        diags2[0].span.as_ref().map(|s| s.line),
        Some(3),
        "{diags2:?}"
    );
}

#[test]
fn errors_match_between_full_and_incremental() {
    // Hard failures abort before any memo write, but pin it: a
    // cyclic document fails identically on both paths, and the
    // memo stays empty so the next good document populates cleanly.
    let src = "a = b\nb = a\n";
    let module = parse(src, "incremental").expect("must parse");
    let engine = Engine::new();
    let full = engine.evaluate(&module).expect_err("cycle must fail");
    let mut memo = IncrementalMemo::default();
    let incr = engine
        .evaluate_incremental(&module, &mut memo)
        .expect_err("cycle must fail");
    assert_eq!(full.to_string(), incr.to_string());
    assert_eq!(memo.last_eval_count, 0);
    assert_eq!(memo.last_memo_hits, 0);
}

#[test]
fn schema_doc_evaluates_identically_under_incremental() {
    // Schema nodes have no stable fingerprint and re-evaluate every
    // call — steady state never reaches zero here — but the VALUES
    // must still match a full evaluation exactly, diagnostics
    // included. (Validation itself re-runs post-walk either way.)
    let src = "schema = { port = Int }\nport = 8080\n";
    let module = parse(src, "incremental").expect("must parse");
    let engine = Engine::new();
    let (full, full_diags) = engine.evaluate(&module).expect("must evaluate");
    let mut memo = IncrementalMemo::default();
    let (incr, incr_diags) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert_eq!(json(&incr), json(&full));
    assert_eq!(incr_diags.len(), full_diags.len());
    let (again, _) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert_eq!(json(&again), json(&full));
}

#[test]
fn memo_debug_reports_shape_not_content() {
    // The memo holds evaluated values (including plaintext
    // intermediates — unavoidable and same-process, like the
    // evaluator's own value map), so its `Debug` impl must never
    // dump document content into logs: shape only.
    let src = "pw = \"s3cr3t-debug\"\ntoken = secret(pw)\n";
    let module = parse(src, "incremental").expect("must parse");
    let engine = Engine::new();
    let mut memo = IncrementalMemo::default();
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    let rendered = format!("{memo:?}");
    assert!(!rendered.contains("s3cr3t-debug"), "{rendered}");
    assert!(!rendered.contains("Secret"), "{rendered}");
    assert!(rendered.contains("entries"), "{rendered}");
}

#[test]
fn store_clear_fires_and_recovers_under_version_churn() {
    // The memo is capped at `max(1024, 4 × arena nodes)` entries
    // and cleared past the cap, so a long edit history cannot grow
    // it without bound. Clearing is behaviorally observable: a
    // version evaluated twice in a row normally hits everything
    // the second time — unless the first call tripped the cap and
    // wiped the store, in which case the repeat recomputes it all.
    // Churn guarantees the trip (30 fully-novel versions dwarf any
    // cap a 600-key document yields); values stay exact throughout
    // and the store works normally afterwards.
    fn version(v: i64) -> String {
        (0..600)
            .map(|k| format!("k{k} = {}\n", v * 1000 + k as i64))
            .collect()
    }
    let engine = Engine::new();
    let mut memo = IncrementalMemo::default();
    let mut tripped = None;
    for v in 0..30 {
        let src = version(v);
        let module = parse(&src, "incremental").expect("must parse");
        let (first, _) = engine
            .evaluate_incremental(&module, &mut memo)
            .expect("must evaluate");
        let full_count = memo.last_eval_count;
        assert!(full_count > 500, "{full_count}");
        match &first {
            Value::Table(map) => assert_eq!(
                int(map.get("k599").expect("k599 present")),
                v * 1000 + 599,
                "wrong values at v{v}"
            ),
            other => panic!("table expected: {other:?}"),
        }
        let (second, _) = engine
            .evaluate_incremental(&module, &mut memo)
            .expect("must evaluate");
        assert_eq!(json(&second), json(&first), "repeat diverged at v{v}");
        if memo.last_eval_count * 2 > full_count {
            tripped = Some(v);
            break;
        }
        assert_eq!(memo.last_eval_count, 0, "no clear yet at v{v}");
    }
    let v = tripped.expect("cap must trip within 30 novel versions");
    // Recovery: the repeat above repopulated the store, so one more
    // identical call serves everything from the memo again.
    let module = parse(&version(v), "incremental").expect("must parse");
    engine
        .evaluate_incremental(&module, &mut memo)
        .expect("must evaluate");
    assert_eq!(memo.last_eval_count, 0, "store broken after clear");
}
