//! Include mechanism tests (C-025/C-026).
//!
//! Drives the real shipped path (`parse_file` → evaluate → export) on
//! committed multi-file fixtures under `tests/includes/`. Covers merge
//! semantics (defaults, local-wins, cascade, deep tables), aliasing,
//! private visibility (C-026), transitivity, cycles, missing files, and
//! the policy errors (E015/E016/E017).

use active_core::ir::IncludeResolutionKind;
use active_core::provenance::{self, IncludeLock, LockManifest, LockMode};
use active_core::{parse_file, Engine, ParseOptions};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("includes")
        .join(name)
}

fn parse_options() -> ParseOptions {
    // Include semantics are not a parse-timing test: a loaded machine must
    // not turn a fixture into an E029 flake. The shipped bounds themselves
    // are pinned by `parse_budget.rs`.
    ParseOptions {
        max_wall_ms: 30_000,
        ..ParseOptions::default()
    }
}

fn eval_file(name: &str) -> serde_json::Value {
    eval_file_with(name, parse_options())
}

fn eval_file_with(name: &str, options: ParseOptions) -> serde_json::Value {
    let module = parse_file(&fixture(name), options)
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    let engine = Engine::new();
    let (value, diags) = engine
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: evaluation failed: {e}"));
    assert!(diags.is_empty(), "{name}: diagnostics: {diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    serde_json::from_str(&json).expect("valid json")
}

fn eval_file_with_diags(name: &str) -> (serde_json::Value, Vec<active_core::Diagnostic>) {
    let module = parse_file(&fixture(name), parse_options())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: evaluation failed: {e}"));
    let json = active_core::convert::value_to_pretty_json(&value);
    (serde_json::from_str(&json).expect("valid json"), diags)
}

fn parse_err_codes(name: &str) -> Vec<String> {
    parse_file(&fixture(name), parse_options())
        .expect_err(&format!("{name} must fail"))
        .iter()
        .map(|d| d.code.clone())
        .collect()
}

#[test]
fn basic_include_deep_merges_with_local_win() {
    let v = eval_file("main_basic.active");
    assert_eq!(v["server"]["host"], "base.local");
    assert_eq!(v["server"]["port"], 9000);
    assert_eq!(v["server"]["debug"], false);
}

#[test]
fn later_include_cascades_but_local_still_wins() {
    // override_b: port=200 debug=true; override_a: host+port=100.
    // Local port=1 beats the whole cascade.
    let v = eval_file("main_cascade.active");
    assert_eq!(v["server"]["host"], "a.local");
    assert_eq!(v["server"]["port"], 1);
    assert_eq!(v["server"]["debug"], true);
}

#[test]
fn conflicting_includes_are_e046_and_keep_the_first_source() {
    let (value, diags) = eval_file_with_diags("main_include_conflict.active");
    let conflict = diags
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {diags:?}"));
    assert_eq!(conflict.severity, active_core::Severity::Error);
    assert_eq!(conflict.path.as_deref(), Some("server.port"));
    assert!(
        conflict.message.contains("conflict_a.active"),
        "{conflict:?}"
    );
    assert!(
        conflict.message.contains("conflict_b.active"),
        "{conflict:?}"
    );
    // Values are never echoed in the conflict report.
    assert!(!conflict.message.contains("100"), "{conflict:?}");
    assert!(!conflict.message.contains("200"), "{conflict:?}");
    // Resilient tie-break: the first include in source order remains.
    assert_eq!(value["server"]["port"], 100);
}

#[test]
fn equal_values_across_includes_agree() {
    let (value, diags) = eval_file_with_diags("main_include_equal.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(value["server"]["port"], 100);
    assert_eq!(value["server"]["host"], "equal.local");
}

#[test]
fn a_three_way_include_merge_reports_one_conflict() {
    let (value, diags) = eval_file_with_diags("main_include_three_way.active");
    let conflicts: Vec<_> = diags.iter().filter(|d| d.code == "E046").collect();
    assert_eq!(conflicts.len(), 1, "{diags:?}");
    assert_eq!(value["server"]["port"], 100);
}

#[test]
fn opaque_table_values_union_across_includes() {
    let (value, diags) = eval_file_with_diags("main_include_opaque_union.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(value["shared"]["x"], 1);
    assert_eq!(value["shared"]["y"], 2);
}

#[test]
fn a_bare_transitive_include_keeps_the_eval_time_union() {
    let (value, diags) = eval_file_with_diags("main_transitive_opaque_union.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(value["x"]["y"], 1);
    assert_eq!(value["x"]["z"], 2);
}

#[test]
fn a_shallow_union_keeps_a_deeper_union_applied_first() {
    let (value, diags) = eval_file_with_diags("main_shallow_nested_union.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(value["x"]["p"]["y"], 11);
    assert_eq!(value["x"]["p"]["z"], 22);
    assert_eq!(value["x"]["q"], 33);
}

#[test]
fn a_transitive_leaf_union_survives_an_outer_include() {
    let (value, diags) = eval_file_with_diags("main_transitive_leaf_union.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(value["x"]["p"]["r"], 0);
    assert_eq!(value["x"]["p"]["y"], 11);
    assert_eq!(value["x"]["p"]["z"], 22);
}

#[test]
fn a_local_header_value_masks_an_ancestor_include_conflict() {
    let (value, diags) = eval_file_with_diags("main_discarded_header_conflict.active");
    assert!(
        diags.iter().all(|d| d.code != "E046"),
        "local value must mask the include conflict: {diags:?}"
    );
    assert_eq!(value["a"]["b"]["p"], 9);
}

#[test]
fn a_local_value_equal_to_an_include_still_masks_the_conflict() {
    let (value, diags) = eval_file_with_diags("main_local_masks_equal.active");
    assert!(
        diags.iter().all(|d| d.code != "E046"),
        "equal-valued local replacement must mask the include conflict: {diags:?}"
    );
    assert_eq!(value["a"]["b"]["p"], 1);
}

#[test]
fn extracted_metadata_is_never_a_merge_target() {
    let (value, diags) = eval_file_with_diags("main_depends_merge.active");
    assert!(
        diags.iter().all(|d| d.code != "E046"),
        "metadata produced a merge conflict: {diags:?}"
    );
    assert!(value.get("depends_on").is_none(), "{value}");
}

#[test]
fn a_profile_survives_a_same_named_table_replacement() {
    let (_, diags) = eval_file_with_diags("main_profile_name_collision.active");
    assert!(
        diags.iter().any(|d| d.code == "E046"),
        "profile conflict was purged: {diags:?}"
    );
}

#[test]
fn a_nested_local_header_value_masks_an_include_conflict() {
    let (value, diags) = eval_file_with_diags("main_nested_header_discard.active");
    assert!(
        diags.iter().all(|d| d.code != "E046"),
        "nested local value must mask the include conflict: {diags:?}"
    );
    assert_eq!(value["outer"]["a"]["b"]["p"], 9);
}

#[test]
fn a_shadowed_alias_include_does_not_report_its_internal_conflict() {
    let (value, diags) = eval_file_with_diags("main_shadowed_alias_conflict.active");
    assert!(
        diags.iter().all(|d| d.code != "E046"),
        "shadowed include's conflict leaked: {diags:?}"
    );
    assert_eq!(value["base"], 5);
}

#[test]
fn an_included_file_repeated_header_conflict_survives_the_include() {
    let dir = std::env::temp_dir().join(format!("active_inc_header_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("c.active"), "[t]\na = 1\n[t]\na = 2\n").unwrap();
    std::fs::write(dir.join("a.active"), "include \"c.active\"\n").unwrap();
    std::fs::write(dir.join("b.active"), "[t]\na = 1\n").unwrap();
    let main = dir.join("main.active");
    std::fs::write(&main, "include \"a.active\"\ninclude \"b.active\"\n").unwrap();
    let module = parse_file(&main, parse_options()).expect("parse");
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("evaluate: {e}"));
    let conflict = diags
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {diags:?}"));
    assert_eq!(conflict.path.as_deref(), Some("t.a"));
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("\"a\": 1"), "{json}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_include_never_overrides_an_earlier_local_repeated_body() {
    let dir = std::env::temp_dir().join(format!("active_inc_priority_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("def_host.active"), "host = \"inc\"\n").unwrap();
    for (name, src) in [
        (
            "local_first.active",
            "t { host = \"local\" }\nt { include \"def_host.active\" }\n",
        ),
        (
            "include_first.active",
            "t { include \"def_host.active\" }\nt { host = \"local\" }\n",
        ),
    ] {
        let main = dir.join(name);
        std::fs::write(&main, src).unwrap();
        let module = parse_file(&main, parse_options()).expect("parse");
        let (value, diags) = Engine::new()
            .evaluate(&module)
            .unwrap_or_else(|e| panic!("evaluate: {e}"));
        assert!(diags.is_empty(), "{name}: {diags:?}");
        let json = active_core::convert::value_to_pretty_json(&value);
        assert!(
            json.contains("\"host\": \"local\""),
            "{name}: include overrode local: {json}"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_material_union_never_exports_plaintext() {
    let module =
        parse_file(&fixture("main_material_union.active"), parse_options()).expect("parse");
    let engine = Engine::new();
    let (value, diags, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let profile = active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds);
    let redacted = active_core::convert::redact_value(&profile, &value);
    let json = active_core::convert::value_to_pretty_json(&redacted);
    assert!(!json.contains("1234"), "material Int leaked: {json}");
    assert!(json.contains("SECRET"), "marker missing: {json}");
}

#[test]
fn material_merge_never_becomes_an_equality_oracle() {
    fn outcome(guess: &str) -> Vec<String> {
        let dir = std::env::temp_dir().join(format!(
            "active_oracle_{}_{}",
            std::process::id(),
            guess.len()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("env.active"), "x = env.AUDIT_TOKEN\n").unwrap();
        std::fs::write(dir.join("guess.active"), format!("x = \"{guess}\"\n")).unwrap();
        std::fs::write(dir.join("secret.active"), "k = secret(env.AUDIT_TOKEN)\n").unwrap();
        let main = dir.join("main.active");
        std::fs::write(
            &main,
            "include \"secret.active\"\ninclude \"env.active\"\ninclude \"guess.active\"\n",
        )
        .unwrap();
        let module = parse_file(&main, parse_options()).expect("parse");
        let mut env = std::collections::HashMap::new();
        env.insert("AUDIT_TOKEN".to_string(), "s3cr3t".to_string());
        let engine = Engine::hermetic().with_env(env);
        let (_, diags, _) = engine.evaluate_with_seeds(&module).expect("evaluate");
        let result = diags
            .iter()
            .map(|d| format!("{}|{}|{:?}", d.code, d.message, d.path))
            .collect();
        std::fs::remove_dir_all(&dir).ok();
        result
    }
    let correct = outcome("s3cr3t");
    let wrong = outcome("wrong");
    assert_eq!(correct, wrong, "diagnostics revealed the guess");
    assert!(correct.iter().any(|line| line.starts_with("E046")));
}

#[test]
fn merge_budgets_do_not_reveal_secret_length() {
    fn outcome(length: usize) -> bool {
        let dir =
            std::env::temp_dir().join(format!("active_len_{}_{}", std::process::id(), length));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.active"), "[x]\na = 1\n").unwrap();
        std::fs::write(
            dir.join("b.active"),
            "token = secret(env.TOKEN)\nlet t = { token = token }\nx = t\n",
        )
        .unwrap();
        let main = dir.join("main.active");
        std::fs::write(&main, "include \"a.active\"\ninclude \"b.active\"\n").unwrap();
        let module = parse_file(&main, parse_options()).expect("parse");
        let mut env = std::collections::HashMap::new();
        env.insert("TOKEN".to_string(), "x".repeat(length));
        let mut engine = Engine::hermetic().with_env(env);
        engine.budget = active_core::Budget {
            max_value_size: 20,
            ..active_core::Budget::default()
        };
        let ok = engine.evaluate(&module).is_ok();
        std::fs::remove_dir_all(&dir).ok();
        ok
    }
    assert_eq!(outcome(5), outcome(50), "secret length changed the budget");
    assert!(outcome(5), "a short secret must fit");
}

#[test]
fn merged_growth_is_charged_to_the_budgets() {
    let dir = std::env::temp_dir().join(format!("active_growth_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let large = "x".repeat(300_000);
    std::fs::write(dir.join("a.active"), "[x]\na = 1\n").unwrap();
    std::fs::write(
        dir.join("b.active"),
        format!("let t = {{ b = \"{large}\" }}\nx = t\n"),
    )
    .unwrap();
    let mut main = String::from("include \"a.active\"\ninclude \"b.active\"\n");
    for index in 0..50 {
        main.push_str(&format!("y{index} = x\n"));
    }
    let main_path = dir.join("main.active");
    std::fs::write(&main_path, main).unwrap();
    let module = parse_file(&main_path, parse_options()).expect("parse");
    let mut engine = Engine::new();
    engine.budget = active_core::Budget {
        max_value_size: 200_000,
        max_total_value_size: 2_000_000,
        ..active_core::Budget::default()
    };
    let error = engine.evaluate(&module).expect_err("growth must trip X006");
    assert_eq!(error.code(), "X006");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_oversized_dotted_header_is_refused_not_aborted() {
    let header = format!("[{}]\nv = 1\n", vec!["a"; 1_000].join("."));
    let codes: Vec<String> = match active_core::parse(&header, "deep-header") {
        Ok(module) => module.diagnostics.iter().map(|d| d.code.clone()).collect(),
        Err(diags) => diags.iter().map(|d| d.code.clone()).collect(),
    };
    assert!(codes.iter().any(|code| code == "E011"), "{codes:?}");
}

#[test]
fn equal_large_includes_fit_the_value_budget_once() {
    let dir = std::env::temp_dir().join(format!("active_merge_budget_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let big = "v".repeat(600);
    let body = format!("t = {{ v = \"{big}\" }}\n");
    std::fs::write(dir.join("a.active"), &body).unwrap();
    std::fs::write(dir.join("b.active"), &body).unwrap();
    let main = dir.join("main.active");
    std::fs::write(&main, "include \"a.active\"\ninclude \"b.active\"\n").unwrap();
    let module = parse_file(&main, parse_options()).expect("parse");
    let mut engine = Engine::new();
    engine.budget = active_core::Budget {
        max_value_size: 1_000,
        ..active_core::Budget::default()
    };
    let (value, diags) = engine
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("equal values must fit: {e}"));
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains(&big), "union value lost");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_included_leaf_under_a_local_container_still_conflicts() {
    let (value, diags) = eval_file_with_diags("main_nested_local_container.active");
    let conflict = diags
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {diags:?}"));
    assert_eq!(conflict.path.as_deref(), Some("server.db.port"));
    assert_eq!(value["server"]["db"]["port"], 1);
    assert_eq!(value["server"]["db"]["local"], 1);
}

#[test]
fn a_repeated_body_merges_and_keeps_its_internal_conflict() {
    let (value, diags) = eval_file_with_diags("main_discarded_duplicate.active");
    // The first outer body's two includes conflict internally; repeated
    // tables now merge, so that conflict is real (E046), and the first
    // body's value is kept while the second body's keys union.
    assert!(
        diags.iter().any(|d| d.code == "E046"),
        "internal include conflict must report: {diags:?}"
    );
    assert!(
        diags.iter().all(|d| d.code != "W011"),
        "repeated tables are no longer discarded: {diags:?}"
    );
    assert_eq!(value["outer"]["outer"]["port"], 100);
    assert_eq!(value["outer"]["y"], 1);
}

#[test]
fn nested_include_conflicts_report_their_full_path() {
    let (_, diags) = eval_file_with_diags("main_include_nested_conflict.active");
    let conflict = diags
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {diags:?}"));
    assert_eq!(conflict.path.as_deref(), Some("outer.inner.value"));
}

#[test]
fn aliased_includes_conflict_under_the_alias_path() {
    let (value, diags) = eval_file_with_diags("main_include_alias_conflict.active");
    let conflict = diags
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {diags:?}"));
    assert_eq!(conflict.path.as_deref(), Some("base.server.port"));
    assert_eq!(value["base"]["server"]["port"], 400);
}

#[test]
fn include_conflicts_inside_a_profile_body_are_reported() {
    let (_, diags) = eval_file_with_diags("main_profile_include_conflict.active");
    let conflict = diags
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {diags:?}"));
    assert!(
        conflict
            .path
            .as_deref()
            .is_some_and(|p| p.ends_with("feature.on")),
        "{conflict:?}"
    );
}

#[test]
fn secret_conflicts_never_echo_material() {
    let (value, diags) = eval_file_with_diags("main_include_secret_conflict.active");
    let conflict = diags
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {diags:?}"));
    assert!(!conflict.message.contains("first-material"), "{conflict:?}");
    assert!(
        !conflict.message.contains("second-material"),
        "{conflict:?}"
    );
    // Exported JSON is redacted, so material never appears.
    let json = serde_json::to_string(&value).expect("json");
    assert!(!json.contains("first-material"), "{json}");
    assert!(!json.contains("second-material"), "{json}");
}

#[test]
fn incremental_merge_conflicts_keep_their_own_position() {
    let dir = std::env::temp_dir().join(format!("active_merge_memo_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.active"), "[server]\nport = 100\n").unwrap();
    std::fs::write(dir.join("b.active"), "[server]\nport = 200\n").unwrap();
    let main = dir.join("main.active");
    std::fs::write(&main, "include \"a.active\"\ninclude \"b.active\"\n").unwrap();
    let module = parse_file(&main, parse_options()).expect("parse");
    let engine = Engine::new();
    let mut memo = active_core::IncrementalMemo::default();
    let (_, first) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("evaluate");
    let first = first
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {first:?}"))
        .clone();
    assert!(first.span.is_some(), "candidate position lost");

    // Whole-memo hit: the cached diagnostic keeps its own position
    // instead of being re-anchored to the scope (which has none).
    let (_, warm) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("evaluate");
    let warm = warm
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {warm:?}"));
    assert_eq!(
        warm.span
            .as_ref()
            .map(|s| (s.line, s.column, s.end_line, s.end_column)),
        first
            .span
            .as_ref()
            .map(|s| (s.line, s.column, s.end_line, s.end_column)),
        "warm span drifted"
    );
    assert_eq!(warm.file, first.file, "warm file drifted");
    assert_eq!(warm.message, first.message, "warm message drifted");

    // A shifted include line invalidates the fingerprint; the fresh
    // diagnostic must match a cold evaluation exactly.
    std::fs::write(dir.join("b.active"), "# shifted\n[server]\nport = 200\n").unwrap();
    let shifted = parse_file(&main, parse_options()).expect("parse");
    let (_, warm) = engine
        .evaluate_incremental(&shifted, &mut memo)
        .expect("evaluate");
    let warm = warm
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {warm:?}"));
    let (_, cold) = Engine::new().evaluate(&shifted).expect("evaluate");
    let cold = cold
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {cold:?}"));
    assert_eq!(
        warm.span
            .as_ref()
            .map(|s| (s.line, s.column, s.end_line, s.end_column)),
        cold.span
            .as_ref()
            .map(|s| (s.line, s.column, s.end_line, s.end_column)),
        "shifted span drifted"
    );
    assert_eq!(warm.file, cold.file, "shifted file drifted");
    assert_eq!(warm.message, cold.message, "shifted message drifted");

    // A column-only shift must also invalidate the fingerprint.
    std::fs::write(dir.join("b.active"), "[server]\nport    = 200\n").unwrap();
    let shifted = parse_file(&main, parse_options()).expect("parse");
    let (_, warm) = engine
        .evaluate_incremental(&shifted, &mut memo)
        .expect("evaluate");
    let warm = warm
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {warm:?}"));
    let (_, cold) = Engine::new().evaluate(&shifted).expect("evaluate");
    let cold = cold
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {cold:?}"));
    assert_eq!(
        warm.span
            .as_ref()
            .map(|s| (s.line, s.column, s.end_line, s.end_column)),
        cold.span
            .as_ref()
            .map(|s| (s.line, s.column, s.end_line, s.end_column)),
        "column shift served a stale position"
    );

    // An identical document under another directory must not serve the
    // first document's absolute file path from the shared memo.
    let other = dir.join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("a.active"), "[server]\nport = 100\n").unwrap();
    std::fs::write(other.join("b.active"), "[server]\nport = 200\n").unwrap();
    let other_main = other.join("main.active");
    std::fs::write(&other_main, "include \"a.active\"\ninclude \"b.active\"\n").unwrap();
    let moved = parse_file(&other_main, parse_options()).expect("parse");
    let (_, warm) = engine
        .evaluate_incremental(&moved, &mut memo)
        .expect("evaluate");
    let warm = warm
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {warm:?}"));
    let (_, cold) = Engine::new().evaluate(&moved).expect("evaluate");
    let cold = cold
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {cold:?}"));
    assert_eq!(warm.file, cold.file, "foreign file path served");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn alias_nests_and_private_stays_hidden() {
    let v = eval_file("main_alias_private.active");
    // Aliased content nests under `base`.
    assert_eq!(v["base"]["server"]["host"], "base.local");
    // Included lets are visible for reference through shared bindings.
    assert_eq!(v["app"]["port"], 8000);
    // The private secret resolves but redacts.
    assert_eq!(v["app"]["pw"], "***SECRET***");
    // Private root keys never reach the exported tree...
    assert!(v.get("server").is_none(), "private keys leaked: {v}");
    // ...and neither does the secret's raw material.
    let raw = serde_json::to_string(&v).unwrap();
    assert!(!raw.contains("test/db/password"), "secret leaked: {raw}");
}

#[test]
fn include_merge_totals_stay_bounded() {
    // Four included files at 400 unclosed brackets each would merge
    // ~1344 diagnostics without a merge bound. Includes sub-parse on
    // the shared parser (capped pushes) and merge through extend_diags,
    // so the merged total stays within the per-parse bound. Written to
    // the OS temp dir (unique per process) so the repo stays clean.
    // Contract guard: the bound lives in push_diag/extend_diags; this
    // pins the include-merge path inherits it end to end.
    let dir = std::env::temp_dir().join(format!("active_include_flood_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut mains = vec!["v = 1\n".to_string()];
    for i in 0..4 {
        let name = format!("flood{i}.active");
        std::fs::write(dir.join(&name), format!("x = {}", "[".repeat(400))).unwrap();
        mains.push(format!("include \"{name}\"\n"));
    }
    let main = dir.join("main_flood.active");
    std::fs::write(&main, mains.concat()).unwrap();
    let diags = parse_file(&main, parse_options()).expect_err("must fail");
    assert!(
        diags.iter().any(|d| d.code == "E004"),
        "depth refusal present"
    );
    assert!(
        diags.len() <= 1024,
        "merged diagnostics bounded, got {}",
        diags.len()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn private_binding_leak_attempts_fail() {
    // A file that references a private name WITHOUT including it gets
    // null + W001, never the value. Written to the OS temp dir (unique
    // per process) so the repo tree stays clean even on failure.
    let main =
        std::env::temp_dir().join(format!("active_leak_attempt_{}.active", std::process::id()));
    std::fs::write(&main, "x = db_password\n").unwrap();
    let module = parse_file(&main, parse_options()).expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.iter().any(|d| d.code == "W001"), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("null"), "{json}");
    std::fs::remove_file(&main).unwrap();
}

#[test]
fn transitive_includes_flatten() {
    let v = eval_file("main_transitive.active");
    assert_eq!(v["app"]["name"], "top");
    assert_eq!(v["mid"]["level"], 2);
    assert_eq!(v["server"]["host"], "base.local");
}

#[test]
fn cascade_scalar_never_discards_local_table() {
    // Review finding: local parent.sub.x = 1, an include conflicting only
    // there, then `parent = 5`. The table (holding local content) must
    // survive; the scalar is ignored.
    let v = eval_file("main_cascade_guard.active");
    assert_eq!(v["parent"]["sub"]["x"], 1);
}

#[test]
fn eval_error_carries_the_included_file() {
    // A type error inside an included file attributes that file, not the
    // root, on both the EvalError and the CLI-equivalent path.
    let err = Engine::new()
        .evaluate(&parse_file(&fixture("main_typerr.active"), parse_options()).expect("must parse"))
        .expect_err("type error must fail");
    assert_eq!(err.code(), "T001");
    let file = err.file().expect("T001 must carry its file");
    assert!(
        file.ends_with("lib_typerr.active"),
        "wrong file attribution: {file}"
    );
}

#[test]
fn circular_includes_are_e016() {
    let codes = parse_err_codes("loop_a.active");
    assert!(codes.iter().any(|c| c == "E016"), "{codes:?}");
}

#[test]
fn evaluation_is_deterministic_across_runs() {
    let first = eval_file("main_cascade.active");
    let second = eval_file("main_cascade.active");
    assert_eq!(first, second);
}

#[test]
fn secret_through_include_redacts_everywhere() {
    for fmt in ["json", "toml"] {
        let module =
            parse_file(&fixture("main_alias_private.active"), parse_options()).expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let out = match fmt {
            "json" => active_core::convert::value_to_pretty_json(&value),
            _ => active_core::convert::value_to_simple_toml(&value, ""),
        };
        assert!(!out.contains("test/db/password"), "{fmt} leak: {out}");
    }
}

#[test]
fn included_mode_and_capabilities_stay_local() {
    // spec/mode.md §1: mode and capability declarations are resolved
    // from the main document only. A profile selector is an ordinary
    // root value, so an include may supply it as a default (covered by
    // `an_included_profile_selector_is_a_root_default`).
    let path = fixture("meta_main.active");
    let module = parse_file(&path, parse_options()).expect("parse");
    assert_eq!(module.mode, active_core::Mode::Config);
    assert!(module.capabilities.is_empty(), "{:?}", module.capabilities);
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let active_core::Value::Table(map) = value else {
        panic!("expected a root table");
    };
    for key in ["active_mode", "active_capabilities", "schema"] {
        assert!(!map.contains_key(key), "included {key} leaked: {map:?}");
    }
    assert!(matches!(map.get("x"), Some(active_core::Value::Int(1))));
}

#[test]
fn an_included_profile_selector_is_a_root_default() {
    // spec/grammar.md §2.7: included files provide defaults, and
    // spec/specification.md §3.5 reads the selector from the root
    // value. An include may therefore supply `active_profile`; mode and
    // capabilities stay local to the main document.
    let value = eval_file("profile_selector_main.active");
    assert_eq!(value["active_profile"], "prod", "{value}");
    assert_eq!(value["x"], "from-prod", "{value}");
}

#[test]
fn include_fan_out_fails_fast_with_e028() {
    // LANG-C-002: a fan-out graph resolves 2^depth includes at a small
    // depth (depth 16 = 65k files scanned, a 196k-node arena). The
    // resolution budget must stop it with a dedicated code.
    let dir = std::env::temp_dir().join(format!("active_fanout_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..16 {
        let next = format!("d{}.active", i + 1);
        std::fs::write(
            dir.join(format!("d{i}.active")),
            format!("include \"{next}\"\ninclude \"{next}\"\n"),
        )
        .unwrap();
    }
    std::fs::write(dir.join("d16.active"), "leaf = 1\n").unwrap();
    let diags =
        parse_file(&dir.join("d0.active"), parse_options()).expect_err("fan-out must fail fast");
    assert!(
        diags.iter().any(|d| d.code == "E028"),
        "fan-out must be E028: {:?}",
        diags.iter().map(|d| d.code.clone()).collect::<Vec<_>>()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_normal_include_chain_stays_within_budget() {
    // Control: a linear chain of includes (the real-world shape) is far
    // below the budget and still parses.
    let dir = std::env::temp_dir().join(format!("active_chain_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..8 {
        std::fs::write(
            dir.join(format!("c{i}.active")),
            format!("include \"c{}.active\"\nk{i} = {i}\n", i + 1),
        )
        .unwrap();
    }
    std::fs::write(dir.join("c8.active"), "leaf = 1\n").unwrap();
    let module =
        parse_file(&dir.join("c0.active"), parse_options()).expect("linear chain must parse");
    assert!(module.root.is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn nonportable_include_paths_are_refused_on_every_host() {
    // LANG-H-004: a backslash separator or a Windows drive-letter
    // absolute cannot mean the same file on every platform, so both are
    // E026 here. LANG-H-902: an empty path is E015 (no path declared),
    // not a host-dependent E017 from reading the including directory.
    let dir = std::env::temp_dir().join(format!("active-inc-port-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    for (name, body, code) in [
        (
            "back.active",
            "include \"sub\\inner.active\"\nport = 1\n",
            "E026",
        ),
        (
            "drive.active",
            "include \"C:/temp/x.active\"\nport = 1\n",
            "E026",
        ),
        ("empty.active", "include \"\"\nport = 1\n", "E015"),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write");
        let diags = parse_file(&path, parse_options()).expect_err("must fail");
        assert!(
            diags.iter().any(|d| d.code == code),
            "{name}: expected {code}, got {diags:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// L-P03/L-P06: hash-pinned includes. `pinned_lib.active` is `pinned_value
// = 7`; its SHA-256 is the pin every test below either carries or
// deliberately violates.
const PINNED_HASH: &str = "2b31d5d570b6cc25ed82a4f538b603f5534da6e601f9ba5b1762bf721a19320b";
const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn lock_of(pins: &[(&str, &str)], mode: LockMode) -> IncludeLock {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut hashes = HashSet::new();
    for (path, hash) in pins {
        map.entry((*path).to_string())
            .or_default()
            .push((*hash).to_string());
        hashes.insert((*hash).to_string());
    }
    IncludeLock {
        resolved: BTreeMap::new(),
        pins: map,
        hashes,
        mode,
        identity_pins: true,
    }
}

fn parse_with_lock(name: &str, lock: IncludeLock) -> active_core::Module {
    let options = ParseOptions {
        include_lock: Some(lock),
        ..parse_options()
    };
    parse_file(&fixture(name), options).unwrap_or_else(|d| panic!("{name}: parse failed: {d:?}"))
}

fn resolution_kinds(module: &active_core::Module) -> Vec<IncludeResolutionKind> {
    module
        .include_resolutions
        .iter()
        .map(|(_, kind)| kind.clone())
        .collect()
}

fn eval_seen(module: &active_core::Module) -> serde_json::Value {
    let (value, diags) = Engine::new().evaluate(module).expect("evaluation");
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == active_core::Severity::Error),
        "errors: {diags:?}"
    );
    let json = active_core::convert::value_to_pretty_json(&value);
    serde_json::from_str(&json).expect("valid json")
}

#[test]
fn inline_pin_matches_and_records_the_resolution() {
    let module = parse_file(&fixture("main_inline_pin.active"), parse_options())
        .expect("inline pin must parse");
    assert_eq!(module.include_pins.len(), 1);
    let pin = &module.include_pins[0];
    assert_eq!(pin.as_written, "pinned_lib.active");
    assert_eq!(pin.sha256, PINNED_HASH);
    assert!(
        pin.resolved.ends_with("pinned_lib.active"),
        "resolved path recorded: {}",
        pin.resolved
    );
    assert_eq!(
        resolution_kinds(&module),
        vec![IncludeResolutionKind::Inline]
    );
    assert_eq!(eval_seen(&module)["pinned_value"], 7);
}

#[test]
fn inline_pin_mismatch_is_e041() {
    let diags = parse_file(&fixture("main_inline_pin_bad.active"), parse_options())
        .expect_err("a wrong inline pin must fail");
    assert!(
        diags.iter().any(|d| d.code == "E041"
            && d.message.contains(PINNED_HASH)
            && d.message.contains(ZERO_HASH)),
        "E041 with both digests expected, got {diags:?}"
    );
}

#[test]
fn inline_pin_survives_formatting() {
    let module = parse_file(&fixture("main_inline_pin.active"), parse_options())
        .expect("inline pin must parse");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    assert!(
        out.contains(&format!(
            "include \"pinned_lib.active\" sha256:{PINNED_HASH}"
        )),
        "formatter dropped the pin: {out}"
    );
}

#[test]
fn lockfile_path_pin_admits_matching_content() {
    let module = parse_with_lock(
        "main_lock_pin.active",
        lock_of(&[("pinned_lib.active", PINNED_HASH)], LockMode::Enforce),
    );
    assert_eq!(
        resolution_kinds(&module),
        vec![IncludeResolutionKind::LockPath]
    );
    assert_eq!(eval_seen(&module)["pinned_value"], 7);
}

#[test]
fn lockfile_pin_mismatch_is_e041() {
    let options = ParseOptions {
        include_lock: Some(lock_of(
            &[("pinned_lib.active", ZERO_HASH)],
            LockMode::Enforce,
        )),
        ..parse_options()
    };
    let diags = parse_file(&fixture("main_lock_pin.active"), options)
        .expect_err("a mismatched lock pin must fail");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "E041" && d.message.contains(ZERO_HASH)),
        "E041 expected, got {diags:?}"
    );
}

#[test]
fn lockfile_missing_pin_is_e041_under_enforce() {
    let options = ParseOptions {
        include_lock: Some(lock_of(&[], LockMode::Enforce)),
        ..parse_options()
    };
    let diags = parse_file(&fixture("main_lock_pin.active"), options)
        .expect_err("an unpinned include must fail under Enforce");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "E041" && d.message.contains("no lockfile pin")),
        "E041 expected, got {diags:?}"
    );
}

#[test]
fn lockfile_missing_pin_is_a_loud_w017_under_warn() {
    let module = parse_with_lock("main_lock_pin.active", lock_of(&[], LockMode::Warn));
    assert_eq!(
        resolution_kinds(&module),
        vec![IncludeResolutionKind::Unpinned]
    );
    assert!(
        module
            .healing
            .iter()
            .any(|d| d.code == "W017" && d.message.contains("not pinned")),
        "W017 expected, got {:?}",
        module.healing
    );
    assert_eq!(eval_seen(&module)["pinned_value"], 7);
}

#[test]
fn legacy_warn_lock_admits_a_renamed_include_by_content() {
    // A lockfile without spelling pins (legacy v1) admits identical bytes
    // under another spelling in warn mode. Enforce refuses the new edge;
    // see `enforce_refuses_a_new_edge_with_pinned_content`.
    let module = parse_with_lock(
        "main_lock_content.active",
        lock_of(&[("other.active", PINNED_HASH)], LockMode::Warn),
    );
    assert_eq!(
        resolution_kinds(&module),
        vec![IncludeResolutionKind::LockContent]
    );
    assert_eq!(eval_seen(&module)["pinned_value"], 7);
}

#[test]
fn enforce_refuses_a_new_edge_with_pinned_content() {
    // Same bytes, different spelling, enforce mode: a lockfile pins the
    // include graph as well as its bytes, so a new edge is E041 even
    // though its content hash appears in the lock.
    let options = ParseOptions {
        include_lock: Some(lock_of(&[("other.active", PINNED_HASH)], LockMode::Enforce)),
        ..parse_options()
    };
    let diags = parse_file(&fixture("main_lock_content.active"), options)
        .expect_err("a new include edge must fail under Enforce");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "E041" && d.message.contains("no lockfile pin")),
        "E041 expected, got {diags:?}"
    );
}

#[test]
fn one_spelling_may_pin_several_digests() {
    // Nested includes can spell the same relative path from different
    // directories. Both pinned digests are admitted rather than only the
    // last one parsed, so a legitimate second resolution is not refused.
    let mut lock = lock_of(&[("pinned_lib.active", ZERO_HASH)], LockMode::Enforce);
    lock.pins
        .get_mut("pinned_lib.active")
        .expect("pin")
        .push(PINNED_HASH.to_string());
    lock.hashes.insert(PINNED_HASH.to_string());
    let module = parse_with_lock("main_lock_pin.active", lock);
    assert_eq!(
        resolution_kinds(&module),
        vec![IncludeResolutionKind::LockPath]
    );
}

#[test]
fn inline_and_lock_pins_are_both_enforced() {
    // Inline pin matches but the lockfile pin does not: the lock still
    // refuses, proving neither check is skipped when both are present.
    let options = ParseOptions {
        include_lock: Some(lock_of(
            &[("pinned_lib.active", ZERO_HASH)],
            LockMode::Enforce,
        )),
        ..parse_options()
    };
    let diags = parse_file(&fixture("main_inline_pin.active"), options)
        .expect_err("the lock mismatch must fail even with a matching inline pin");
    assert!(
        diags.iter().any(|d| d.code == "E041"),
        "E041 expected, got {diags:?}"
    );
}

#[test]
fn lockfile_records_include_pins_and_reparses_them() {
    let path = fixture("main_inline_pin.active");
    let module = parse_file(&path, parse_options()).expect("inline pin must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("evaluation");
    assert!(diags.is_empty(), "diagnostics: {diags:?}");
    let provenance = provenance::collect_provenance(&module, &HashMap::new(), Some(&path), None)
        .expect("provenance");
    let rendered = LockManifest::new("active-test".into(), &value, provenance).render();
    assert!(
        rendered.contains("include_pins") && rendered.contains(PINNED_HASH),
        "lockfile lacks the include pin: {rendered}"
    );
    assert!(
        rendered.contains("as = \"pinned_lib.active\""),
        "lockfile lacks the as-written spelling: {rendered}"
    );
    assert!(
        rendered.contains("resolved = "),
        "lockfile lacks the resolved path: {rendered}"
    );
    let lock = IncludeLock::parse(&rendered).expect("rendered lockfile must parse");
    assert_eq!(
        lock.resolved.get("pinned_lib.active"),
        Some(&vec![PINNED_HASH.to_string()])
    );
    assert!(lock.identity_pins);
    assert_eq!(lock.mode, LockMode::Enforce);
}

#[test]
fn include_lock_parses_the_legacy_include_list() {
    // v1 lockfiles written before `include_pins` existed still pin by the
    // stamped include path; the content-hash set is what admits them.
    let text = format!(
        "version = 1\n[provenance]\n[[provenance.includes]]\npath = \"/tmp/lib.active\"\nsha256 = \"{PINNED_HASH}\"\n"
    );
    let lock = IncludeLock::parse(&text).expect("legacy lockfile must parse");
    assert!(lock.hashes.contains(PINNED_HASH));
    assert!(!lock.identity_pins);
    assert_eq!(lock.mode, LockMode::Warn);
    assert!(
        lock.resolved.is_empty(),
        "legacy include paths are lockframe paths, not identities: {:?}",
        lock.resolved
    );
}

#[test]
fn resolved_pins_disambiguate_one_spelling_across_directories() {
    // Both nested documents spell their include `lib.active`; the two
    // files differ. A spelling-keyed lock would admit either digest for
    // either edge, so swapping the bytes would pass. The resolved path
    // is the identity: the swapped file must fail E041.
    let dir = std::env::temp_dir().join(format!("active_pin_paths_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    std::fs::write(
        dir.join("main.active"),
        "include \"a/inner.active\" as x\ninclude \"b/inner.active\" as y\n",
    )
    .unwrap();
    for (sub, value) in [("a", 1), ("b", 2)] {
        std::fs::write(
            dir.join(sub).join("inner.active"),
            "include \"lib.active\"\n",
        )
        .unwrap();
        std::fs::write(dir.join(sub).join("lib.active"), format!("v = {value}\n")).unwrap();
    }
    let main = dir.join("main.active");
    let module = parse_file(&main, parse_options()).expect("unlocked parse");
    let mut resolved: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut hashes = HashSet::new();
    for pin in &module.include_pins {
        resolved
            .entry(pin.resolved.clone())
            .or_default()
            .push(pin.sha256.clone());
        hashes.insert(pin.sha256.clone());
    }
    let lock = IncludeLock {
        resolved,
        pins: BTreeMap::new(),
        hashes,
        mode: LockMode::Enforce,
        identity_pins: true,
    };
    let options = ParseOptions {
        include_lock: Some(lock.clone()),
        ..parse_options()
    };
    parse_file(&main, options).expect("the pinned graph must parse");
    // Substitute b/lib.active with a/lib.active's bytes.
    std::fs::write(dir.join("b").join("lib.active"), "v = 1\n").unwrap();
    let options = ParseOptions {
        include_lock: Some(lock),
        ..parse_options()
    };
    let diags = parse_file(&main, options)
        .expect_err("a swapped same-spelling include must fail under Enforce");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "E041" && d.message.contains("lockfile pin")),
        "E041 for the swapped same-spelling include expected, got {diags:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_malformed_or_repeated_inline_pin_is_e015() {
    let dir = std::env::temp_dir().join(format!("active_pin_shape_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.active"), "v = 1\n").unwrap();
    let short = &PINNED_HASH[..63];
    for (name, line, probe) in [
        (
            "short.active",
            format!("include \"lib.active\" sha256:{short}\n"),
            "malformed include statement",
        ),
        (
            "repeated.active",
            format!("include \"lib.active\" sha256:{PINNED_HASH} sha256:{PINNED_HASH}\n"),
            "repeated sha256 pin",
        ),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, line).unwrap();
        let diags = parse_file(&path, parse_options()).expect_err("must be refused");
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E015" && d.message.contains(probe)),
            "{name}: E015 ({probe}) expected, got {diags:?}"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_uppercase_inline_pin_matches() {
    let dir = std::env::temp_dir().join(format!("active_pin_case_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.active"), "v = 1\n").unwrap();
    let hash = active_core::provenance::hash_file(dir.join("lib.active").to_str().expect("utf8"))
        .expect("hash");
    std::fs::write(
        dir.join("main.active"),
        format!("include \"lib.active\" sha256:{}\n", hash.to_uppercase()),
    )
    .unwrap();
    let module = parse_file(&dir.join("main.active"), parse_options())
        .expect("hex comparison is case-insensitive");
    assert_eq!(
        resolution_kinds(&module),
        vec![IncludeResolutionKind::Inline]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn inline_pins_verify_loader_supplied_bytes() {
    // The digest is over the bytes the loader returned, not a re-read
    // file: a host loader (browser blob, archive, test fixture) is
    // covered by the same promise.
    let loader: active_core::parser::IncludeLoader =
        std::sync::Arc::new(|_path: &str| Ok("pinned_value = 7\n".to_string()));
    let options = ParseOptions {
        enable_includes: true,
        include_loader: Some(loader.clone()),
        ..parse_options()
    };
    let source = format!("include \"lib.active\" sha256:{PINNED_HASH}\n");
    let module = active_core::parser::parse_with(&source, "loader-main", options)
        .expect("matching loader bytes must pass");
    assert_eq!(
        resolution_kinds(&module),
        vec![IncludeResolutionKind::Inline]
    );

    let options = ParseOptions {
        enable_includes: true,
        include_loader: Some(loader),
        ..parse_options()
    };
    let source = format!("include \"lib.active\" sha256:{ZERO_HASH}\n");
    let diags = active_core::parser::parse_with(&source, "loader-main", options)
        .expect_err("a wrong pin must refuse loader bytes");
    assert!(diags.iter().any(|d| d.code == "E041"), "{diags:?}");
}

#[test]
fn include_lock_rejects_future_or_malformed_files() {
    // Fail closed: a lockfile this tool cannot read must not silently
    // resolve fewer pins than it names.
    assert!(IncludeLock::parse("version = 3\n").is_err());
    // Version 2 always carries include_pins; a v2 file that lost the
    // section was edited or truncated and must not become a legacy lock.
    assert!(IncludeLock::parse("version = 2\n").is_err());
    // Identity pins are a v2 feature; a v1 file carrying them is a
    // mislabeled or hand-edited lock, not a legacy file.
    assert!(IncludeLock::parse("version = 1\n[provenance]\ninclude_pins = []\n").is_err());
    assert!(IncludeLock::parse(
        "version = 2\n[provenance]\n[[provenance.include_pins]]\nas = \"x\"\nsha256 = \"abc\"\n"
    )
    .is_err());
    assert!(IncludeLock::parse(
        "version = 2\n[provenance]\n[[provenance.include_pins]]\nsha256 = \"2b31d5d570b6cc25ed82a4f538b603f5534da6e601f9ba5b1762bf721a19320b\"\n"
    )
    .is_err());
    // A malformed shape must not silently downgrade enforcement to warn.
    assert!(IncludeLock::parse("version = 2\n[provenance]\ninclude_pins = \"oops\"\n").is_err());
    assert!(IncludeLock::parse(
        "version = 2\n[provenance]\n[[provenance.include_pins]]\nas = \"x\"\nresolved = 7\nsha256 = \"2b31d5d570b6cc25ed82a4f538b603f5534da6e601f9ba5b1762bf721a19320b\"\n"
    )
    .is_err());
}

#[test]
fn an_inline_pin_is_not_warned_about_under_a_legacy_lock() {
    // The document already proved the bytes; a legacy lock's lack of a
    // spelling pin is not a gap in that promise.
    let module = parse_with_lock("main_inline_pin.active", lock_of(&[], LockMode::Warn));
    assert_eq!(
        resolution_kinds(&module),
        vec![IncludeResolutionKind::Inline]
    );
    assert!(
        !module.healing.iter().any(|d| d.code == "W017"),
        "no W017 expected for an inline-pinned include: {:?}",
        module.healing
    );
}

#[test]
fn a_pin_on_its_own_line_is_e002() {
    let dir = std::env::temp_dir().join(format!("active_pin_line_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.active"), "v = 1\n").unwrap();
    std::fs::write(
        dir.join("main.active"),
        format!("include \"lib.active\"\nsha256:{PINNED_HASH}\n"),
    )
    .unwrap();
    let diags = parse_file(&dir.join("main.active"), parse_options())
        .expect_err("a detached pin must not be silently dropped");
    assert!(
        diags.iter().any(|d| d.code == "E002"
            && d.message
                .contains("only valid on the include statement line")),
        "E002 expected, got {diags:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn root_relative_pins_survive_a_moved_tree() {
    let base = std::env::temp_dir().join(format!("active_pin_move_{}", std::process::id()));
    let one = base.join("one");
    let two = base.join("two");
    for dir in [&one, &two] {
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
    }
    for root in [&one, &two] {
        std::fs::write(
            root.join("main.active"),
            "include \"a/inner.active\" as x\ninclude \"b/inner.active\" as y\n",
        )
        .unwrap();
        for (sub, value) in [("a", 1), ("b", 2)] {
            std::fs::write(
                root.join(sub).join("inner.active"),
                "include \"lib.active\"\n",
            )
            .unwrap();
            std::fs::write(root.join(sub).join("lib.active"), format!("v = {value}\n")).unwrap();
        }
    }
    let module = parse_file(&one.join("main.active"), parse_options()).expect("parse");
    // Every pin is root-relative, so the lock travels with the tree.
    for pin in &module.include_pins {
        assert!(
            !pin.resolved.contains(&one.to_string_lossy().to_string()),
            "resolved must be root-relative: {}",
            pin.resolved
        );
    }
    let mut resolved: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut hashes = HashSet::new();
    for pin in &module.include_pins {
        resolved
            .entry(pin.resolved.clone())
            .or_default()
            .push(pin.sha256.clone());
        hashes.insert(pin.sha256.clone());
    }
    let lock = IncludeLock {
        resolved,
        pins: BTreeMap::new(),
        hashes,
        mode: LockMode::Enforce,
        identity_pins: true,
    };
    let options = ParseOptions {
        include_lock: Some(lock),
        ..parse_options()
    };
    // Same tree in another directory: the lock still verifies.
    parse_file(&two.join("main.active"), options.clone()).expect("moved tree must verify");
    // Substitute b's include in the moved tree: the resolved pin refuses.
    std::fs::write(two.join("b").join("lib.active"), "v = 1\n").unwrap();
    let diags = parse_file(&two.join("main.active"), options)
        .expect_err("a substituted same-spelling include in the moved tree must fail");
    assert!(
        diags.iter().any(|d| d.code == "E041"),
        "E041 expected, got {diags:?}"
    );
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn enforce_refuses_a_new_same_spelling_edge_even_with_identical_bytes() {
    let dir = std::env::temp_dir().join(format!("active_pin_edge_{}", std::process::id()));
    for sub in ["a", "b"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
        std::fs::write(
            dir.join(sub).join("inner.active"),
            "include \"lib.active\"\n",
        )
        .unwrap();
        // Identical bytes on purpose: content alone must not admit a new
        // edge under an enforced lock.
        std::fs::write(dir.join(sub).join("lib.active"), "v = 1\n").unwrap();
    }
    let main = dir.join("main.active");
    std::fs::write(
        &main,
        "include \"a/inner.active\" as x\ninclude \"b/inner.active\" as y\n",
    )
    .unwrap();
    let module = parse_file(&main, parse_options()).expect("parse");
    let mut resolved: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut hashes = HashSet::new();
    for pin in &module.include_pins {
        if pin.resolved.ends_with("b/lib.active") {
            // The lock recorded every edge except this one.
            continue;
        }
        resolved
            .entry(pin.resolved.clone())
            .or_default()
            .push(pin.sha256.clone());
        hashes.insert(pin.sha256.clone());
    }
    let lock = IncludeLock {
        resolved,
        pins: BTreeMap::new(),
        hashes,
        mode: LockMode::Enforce,
        identity_pins: true,
    };
    let options = ParseOptions {
        include_lock: Some(lock),
        ..parse_options()
    };
    let diags = parse_file(&main, options)
        .expect_err("a new same-spelling edge must be refused when its bytes match");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "E041" && d.message.contains("no lockfile pin")),
        "E041 for the new edge expected, got {diags:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn loader_pins_use_loader_bytes_and_stay_root_relative() {
    let dir = std::env::temp_dir().join(format!("active_pin_loader_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("main.active");
    // On-disk bytes differ on purpose: the loader's bytes are the truth.
    std::fs::write(dir.join("lib.active"), "v = 1\n").unwrap();
    std::fs::write(&main, "include \"lib.active\"\n").unwrap();
    let loader: active_core::parser::IncludeLoader = std::sync::Arc::new(|path: &str| {
        assert!(path.ends_with("lib.active"), "loader path: {path}");
        Ok("v = 2\n".to_string())
    });
    let options = ParseOptions {
        include_loader: Some(loader),
        ..parse_options()
    };
    let module = active_core::parser::parse_text_with_file(
        "include \"lib.active\"\n",
        "main.active",
        options,
        &main,
    )
    .expect("loader parse");
    let pin = &module.include_pins[0];
    assert_eq!(pin.resolved, "lib.active");
    let loader_hash = active_core::provenance::hash_file(
        // Hash the loader bytes without writing them to disk.
        {
            let probe = dir.join("loader-bytes.active");
            std::fs::write(&probe, "v = 2\n").unwrap();
            probe
        }
        .to_str()
        .expect("utf8"),
    )
    .expect("hash");
    assert_eq!(pin.sha256, loader_hash, "pin is over loader bytes");
    let provenance =
        active_core::provenance::collect_provenance(&module, &HashMap::new(), Some(&main), None)
            .expect("provenance");
    assert_eq!(provenance.includes.len(), 1);
    assert_eq!(
        provenance.includes[0].sha256, loader_hash,
        "provenance must not re-read the disk for loader bytes"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn nested_loader_includes_keep_the_host_lexical_frame() {
    // F-016: a host loader knows its own keys; a nested include must be
    // asked for in the same relative frame, not an absolute cwd path.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let loader: active_core::parser::IncludeLoader = std::sync::Arc::new(move |path: &str| {
        recorder.lock().expect("lock").push(path.to_string());
        let key = path.replace('\\', "/");
        let key = key.trim_start_matches("./");
        match key {
            "a.active" => Ok("include \"b.active\"\n".to_string()),
            "b.active" => Ok("v = 1\n".to_string()),
            other => Err(format!("unknown virtual path {other}")),
        }
    });
    let options = ParseOptions {
        enable_includes: true,
        include_loader: Some(loader),
        ..parse_options()
    };
    let module = active_core::parser::parse_with("include \"a.active\"\n", "ffi", options)
        .expect("nested loader parse");
    let seen = seen.lock().expect("lock").clone();
    assert_eq!(seen.len(), 2);
    assert!(
        seen.iter().all(|p| !std::path::Path::new(p).is_absolute()),
        "a nested loader callback must stay lexical: {seen:?}"
    );
    assert_eq!(module.include_pins.len(), 2);
    assert_eq!(module.include_pins[1].as_written, "b.active");
}

#[test]
fn include_store_records_each_parsed_include_once() {
    let root = std::env::temp_dir().join(format!("active_inc_store_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let options = ParseOptions {
        include_store: Some(root.clone()),
        ..parse_options()
    };
    parse_file(&fixture("main_inline_pin.active"), options).expect("parse");
    let store = active_core::store::Store::new(root.clone());
    let hashes = store.hashes().expect("hashes");
    assert_eq!(hashes, vec![PINNED_HASH.to_string()]);
    assert_eq!(
        store.get(PINNED_HASH).expect("get").as_deref(),
        Some(b"pinned_value = 7\n".as_slice())
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_store_failure_is_w018_and_never_changes_the_tree() {
    let dir = std::env::temp_dir().join(format!("active_inc_store_bad_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A file where the store directory should be: every write fails.
    let not_a_dir = dir.join("not-a-dir");
    std::fs::write(&not_a_dir, "occupied").unwrap();
    let options = ParseOptions {
        include_store: Some(not_a_dir),
        ..parse_options()
    };
    let module = parse_file(&fixture("main_inline_pin.active"), options).expect("parse");
    assert!(
        module
            .healing
            .iter()
            .any(|d| d.code == "W018" && d.message.contains("store write failed")),
        "W018 expected, got {:?}",
        module.healing
    );
    assert_eq!(eval_seen(&module)["pinned_value"], 7);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_repeated_inline_pinned_include_records_each_resolution() {
    let dir = std::env::temp_dir().join(format!("active_pin_repeat_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.active"), "pinned_value = 7\n").unwrap();
    std::fs::write(
        dir.join("main.active"),
        format!(
            "include \"lib.active\" sha256:{PINNED_HASH}\ninclude \"lib.active\" sha256:{PINNED_HASH}\n"
        ),
    )
    .unwrap();
    let module = parse_file(&dir.join("main.active"), parse_options()).expect("parse");
    assert_eq!(module.include_pins.len(), 2, "each resolution is recorded");
    assert_eq!(
        resolution_kinds(&module),
        vec![IncludeResolutionKind::Inline; 2]
    );
    assert_eq!(eval_seen(&module)["pinned_value"], 7);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn repeated_identical_includes_still_trip_the_resolution_cap() {
    // Interning deduplicates work by content, never the C-002 fan-out
    // budget: 300 resolutions of one file must still fail.
    let dir = std::env::temp_dir().join(format!("active_pin_repeat_cap_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.active"), "v = 1\n").unwrap();
    let mut main = String::new();
    for _ in 0..300 {
        main.push_str("include \"lib.active\"\n");
    }
    std::fs::write(dir.join("main.active"), main).unwrap();
    let diags = parse_file(&dir.join("main.active"), parse_options())
        .expect_err("the resolution cap still applies");
    assert!(
        diags.iter().any(|d| d.code == "E028"),
        "E028 expected, got {:?}",
        diags.iter().map(|d| d.code.clone()).collect::<Vec<_>>()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn identical_content_under_two_paths_keeps_per_file_diagnostics() {
    let dir = std::env::temp_dir().join(format!("active_intern_paths_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Identical bytes, identical lexical error: the cached stream must
    // still be attributed to each including file.
    for name in ["a.active", "b.active"] {
        std::fs::write(dir.join(name), "x = $\n").unwrap();
    }
    std::fs::write(
        dir.join("main.active"),
        "include \"a.active\"\ninclude \"b.active\"\n",
    )
    .unwrap();
    let diags = parse_file(&dir.join("main.active"), parse_options())
        .expect_err("the lexical error is refused");
    let files: Vec<String> = diags
        .iter()
        .filter(|d| d.code == "E005")
        .filter_map(|d| d.file.clone())
        .collect();
    assert_eq!(files.len(), 2, "one E005 per file: {diags:?}");
    assert_ne!(files[0], files[1], "file stamps stay per include");
    std::fs::remove_dir_all(&dir).ok();
}
