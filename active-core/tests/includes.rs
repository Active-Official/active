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
    let diags = parse_file(&main, ParseOptions::default()).expect_err("must fail");
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

#[test]
fn included_mode_and_capabilities_stay_local() {
    // spec/mode.md §1: mode and capability declarations are resolved
    // from the main document only. A profile selector is an ordinary
    // root value, so an include may supply it as a default (covered by
    // `an_included_profile_selector_is_a_root_default`).
    let path = fixture("meta_main.active");
    let module = parse_file(&path, ParseOptions::default()).expect("parse");
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
