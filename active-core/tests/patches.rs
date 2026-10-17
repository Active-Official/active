//! Structural patch tests (C-028/C-029).
//!
//! Drives the real shipped path (`parse_file` → evaluate → export) on
//! committed fixtures under `tests/patches/`. Covers every operation
//! (set, nested set with intermediate creation, remove by key and by
//! index, rename), source-order application, idempotent warnings
//! (W004), rename conflicts (E019), interaction with profiles
//! (patches apply post-merge) and includes, dependency-aware patch
//! values, determinism, and formatter round-trips.

use active_core::{parse_file, Engine, ParseOptions};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("patches")
        .join(name)
}

fn eval_file(name: &str) -> (serde_json::Value, Vec<active_core::Diagnostic>) {
    let module = parse_file(&fixture(name), ParseOptions::default())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    let engine = Engine::new();
    let (value, diags) = engine
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: evaluation failed: {e}"));
    let json = active_core::convert::value_to_pretty_json(&value);
    (serde_json::from_str(&json).expect("valid json"), diags)
}

#[test]
fn all_operations_apply_in_source_order() {
    let (v, diags) = eval_file("main_all_ops.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(v["server"]["port"], 9090);
    assert_eq!(v["server"]["tls"]["enabled"], true);
    assert!(v["server"].get("debug").is_none());
    assert_eq!(v["server"]["new_name"], "x");
    assert!(v["server"].get("old_name").is_none());
    assert_eq!(v["servers"]["list"][0]["port"], 10);
    assert_eq!(v["servers"]["list"].as_array().unwrap().len(), 1);
    // Dependency-aware value referencing lets.
    assert_eq!(v["computed"], 9091);
    // Patch directives never export.
    assert!(v.get("patch").is_none());
}

#[test]
fn last_patch_wins_and_missing_targets_warn() {
    let (v, diags) = eval_file("main_order_idempotent.active");
    assert_eq!(v["server"]["port"], 3);
    // remove-missing + rename-missing-source both warn W004.
    assert_eq!(
        diags.iter().filter(|d| d.code == "W004").count(),
        2,
        "{diags:?}"
    );
}

#[test]
fn rename_onto_existing_key_is_e019() {
    let module = parse_file(
        &fixture("main_rename_conflict.active"),
        ParseOptions::default(),
    )
    .expect("must parse");
    let err = Engine::new()
        .evaluate(&module)
        .expect_err("rename conflict must fail");
    assert_eq!(err.code(), "E019");
}

#[test]
fn patches_apply_after_profile_merge() {
    // Profile sets port=443; the patch adds debug=true on top.
    let (v, diags) = eval_file("main_profile_interact.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(v["server"]["port"], 443);
    assert_eq!(v["server"]["host"], "localhost");
    assert_eq!(v["server"]["debug"], true);
}

#[test]
fn patches_compose_with_includes() {
    // Included [server]{host} merges; the included patch (port=7777)
    // applies to the composed tree in encounter order; the local patch
    // (extra=added) applies after. Directives always win over base
    // values — they run post-evaluation by design.
    let (v, diags) = eval_file("main_include_interact.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(v["server"]["host"], "lib.local");
    assert_eq!(v["server"]["port"], 7777);
    assert_eq!(v["server"]["extra"], "added");
}

#[test]
fn patch_application_is_deterministic() {
    let (first, _) = eval_file("main_all_ops.active");
    let (second, _) = eval_file("main_all_ops.active");
    assert_eq!(first, second);
}

#[test]
fn include_patch_order_matches_source_order() {
    // include A, local patch, include B: textual order is from_a,
    // local, from_b — the last one wins regardless of splice mechanics.
    let (v, diags) = eval_file("main_order_marks.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(v["x"], "from_b");
}

#[test]
fn consecutive_includes_keep_source_order() {
    // Same recorded mark (no local patches between): the later include
    // still applies later.
    let (v, diags) = eval_file("main_consecutive.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(v["x"], "from_b");
}

#[test]
fn failed_set_rolls_back_partial_containers() {
    // `new` and `new.deep` materialize during navigation, then index 3
    // on the empty array fails: the rollback removes every trace.
    let (v, diags) = eval_file("main_rollback.active");
    assert!(v.get("new").is_none(), "partial containers leaked: {v}");
    assert_eq!(diags.iter().filter(|d| d.code == "W004").count(), 1);
    assert_eq!(v["t"]["v"], 1);
}

#[test]
fn failed_remove_and_rename_leave_no_trace() {
    // Read-only navigation: no tables materialize, and rename checks
    // the source before the target (missing source warns even when the
    // target exists).
    let (v, diags) = eval_file("main_nocreate.active");
    assert!(v.get("missing").is_none(), "phantom tables created: {v}");
    assert_eq!(diags.iter().filter(|d| d.code == "W004").count(), 2);
    assert_eq!(v["server"]["port"], 1);
}

#[test]
fn patches_round_trip_through_fmt() {
    for style in [active_core::Style::Brace, active_core::Style::Indent] {
        let module = parse_file(&fixture("main_all_ops.active"), ParseOptions::default())
            .expect("must parse");
        assert_eq!(module.patches.len(), 7);
        let out = active_core::fmt::format(&module, style);
        for op in ["@patch", "@remove", "@rename"] {
            assert!(out.contains(op), "{op} survives fmt in {style:?}: {out}");
        }
        let module2 =
            active_core::parse(&out, "fmt").unwrap_or_else(|d| panic!("fmt output parses: {d:?}"));
        assert_eq!(module2.patches.len(), 7, "patch count survives {style:?}");
        let (v2, _) = Engine::new().evaluate(&module2).expect("must evaluate");
        let json = active_core::convert::value_to_pretty_json(&v2);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["server"]["port"], 9090, "values survive fmt in {style:?}");
        assert_eq!(v["computed"], 9091, "values survive fmt in {style:?}");
    }
}

#[test]
fn fmt_preserves_include_patch_order() {
    // A local patch, then an include (whose patch sets x=from_a), then
    // another local patch: textual order is first, from_a, local-y, and
    // evaluation agrees (x=from_a, y=local). fmt must keep the
    // include/patch interleaving so a reparse resolves identically.
    let module =
        parse_file(&fixture("main_fmt_order.active"), ParseOptions::default()).expect("must parse");
    let (v, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&v);
    let vv: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(vv["x"], "from_a");
    assert_eq!(vv["y"], "local");
    for style in [active_core::Style::Brace, active_core::Style::Indent] {
        let out = active_core::fmt::format(&module, style);
        let first = out.find("\"first\"").expect("first patch survives");
        let inc = out.find("include").expect("include survives fmt");
        let y = out.find("\"y\"").expect("y patch survives fmt");
        assert!(
            first < inc && inc < y,
            "interleaving preserved in {style:?}: {out}"
        );
    }
}
