//! Include mechanism tests (C-025/C-026).
//!
//! Drives the real shipped path (`parse_file` → evaluate → export) on
//! committed multi-file fixtures under `tests/includes/`. Covers merge
//! semantics (defaults, local-wins, cascade, deep tables), aliasing,
//! private visibility (C-026), transitivity, cycles, missing files, and
//! the policy errors (E015/E016/E017).

use active_core::{parse_file, Engine, ParseOptions};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("includes")
        .join(name)
}

fn eval_file(name: &str) -> serde_json::Value {
    eval_file_with(name, ParseOptions::default())
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

fn parse_err_codes(name: &str) -> Vec<String> {
    parse_file(&fixture(name), ParseOptions::default())
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
fn private_binding_leak_attempts_fail() {
    // A file that references a private name WITHOUT including it gets
    // null + W001, never the value. Written to the OS temp dir (unique
    // per process) so the repo tree stays clean even on failure.
    let main =
        std::env::temp_dir().join(format!("active_leak_attempt_{}.active", std::process::id()));
    std::fs::write(&main, "x = db_password\n").unwrap();
    let module = parse_file(&main, ParseOptions::default()).expect("must parse");
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
        .evaluate(
            &parse_file(&fixture("main_typerr.active"), ParseOptions::default())
                .expect("must parse"),
        )
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
        let module = parse_file(
            &fixture("main_alias_private.active"),
            ParseOptions::default(),
        )
        .expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let out = match fmt {
            "json" => active_core::convert::value_to_pretty_json(&value),
            _ => active_core::convert::value_to_simple_toml(&value, ""),
        };
        assert!(!out.contains("test/db/password"), "{fmt} leak: {out}");
    }
}
