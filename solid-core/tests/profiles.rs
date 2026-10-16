//! Profile selection and merge tests (C-027).
//!
//! Drives the real shipped path (`parse_file` → evaluate → export) on
//! committed fixtures under `tests/profiles/`. Covers selection,
//! deep-merge with local-wins, references from profiles to base keys
//! and lets, W002/W003 diagnostics, E018 diagnostics, strictness toward
//! unselected profiles, determinism, and the `--profile` override path
//! (via profile_override, since env mutation is unsafe in tests).

use solid_core::{parse_file, Engine, ParseOptions};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("profiles")
        .join(name)
}

fn eval_with(
    name: &str,
    profile_override: Option<String>,
) -> (serde_json::Value, Vec<solid_core::Diagnostic>) {
    let module = parse_file(&fixture(name), ParseOptions::default())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    let mut engine = Engine::new();
    engine.profile_override = profile_override;
    let (value, diags) = engine
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: evaluation failed: {e}"));
    let json = solid_core::convert::value_to_pretty_json(&value);
    (serde_json::from_str(&json).expect("valid json"), diags)
}

fn eval(name: &str) -> serde_json::Value {
    let (v, diags) = eval_with(name, None);
    assert!(
        diags.is_empty(),
        "{name}: unexpected diagnostics: {diags:?}"
    );
    v
}

#[test]
fn development_profile_merges_by_default() {
    let v = eval("main_select.solid");
    assert_eq!(v["server"]["host"], "localhost");
    assert_eq!(v["server"]["port"], 8080);
    assert_eq!(v["server"]["debug"], true);
    assert_eq!(v["limits"]["max"], 100);
    // Profile blocks never export; the selector stays visible.
    assert!(v.get("development").is_none() && v.get("production").is_none());
    assert_eq!(v["active_profile"], "development");
}

#[test]
fn profile_override_selects_production() {
    let (v, diags) = eval_with("main_select.solid", Some("production".into()));
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(v["server"]["host"], "prod.example.com");
    assert_eq!(v["server"]["port"], 443);
    assert_eq!(v["server"]["debug"], false);
    assert_eq!(v["limits"]["max"], 1000);
}

#[test]
fn profiles_reference_base_keys_and_lets() {
    // Forward (base_port defined after) and backward references.
    let v = eval("main_refs.solid");
    assert_eq!(v["server"]["host"], "prod.example.com");
    assert_eq!(v["server"]["port"], 443);
}

#[test]
fn unknown_profile_warns_w002_without_merge() {
    let module =
        parse_file(&fixture("main_unknown.solid"), ParseOptions::default()).expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.iter().any(|d| d.code == "W002"), "{diags:?}");
    let json = solid_core::convert::value_to_pretty_json(&value);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["server"]["port"], 8080);
}

#[test]
fn type_mismatch_override_warns_w003_and_wins() {
    let module =
        parse_file(&fixture("main_mismatch.solid"), ParseOptions::default()).expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.iter().any(|d| d.code == "W003"), "{diags:?}");
    let json = solid_core::convert::value_to_pretty_json(&value);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["server"]["port"], "fast");
}

#[test]
fn broken_inactive_profile_still_fails() {
    // Strictness: an unselected profile's errors fail the run.
    let module = parse_file(
        &fixture("main_broken_inactive.solid"),
        ParseOptions::default(),
    )
    .expect("must parse");
    assert!(
        Engine::new().evaluate(&module).is_err(),
        "unselected broken profile must fail evaluation"
    );
}

#[test]
fn duplicate_profile_is_e018() {
    let codes: Vec<String> = parse_file(&fixture("main_duplicate.solid"), ParseOptions::default())
        .expect_err("duplicate profile must fail")
        .iter()
        .map(|d| d.code.clone())
        .collect();
    assert!(codes.iter().any(|c| c == "E018"), "{codes:?}");
}

#[test]
fn nested_profile_is_e018() {
    let codes: Vec<String> = parse_file(&fixture("main_nested.solid"), ParseOptions::default())
        .expect_err("nested profile must fail")
        .iter()
        .map(|d| d.code.clone())
        .collect();
    assert!(codes.iter().any(|c| c == "E018"), "{codes:?}");
}

#[test]
fn profile_selection_is_deterministic() {
    let (first, _) = eval_with("main_select.solid", Some("production".into()));
    let (second, _) = eval_with("main_select.solid", Some("production".into()));
    assert_eq!(first, second);
}

#[test]
fn override_with_no_profiles_warns_about_override() {
    // No profile blocks at all: an explicit --profile names an unknown
    // profile (the override takes precedence over the absent key).
    let module = parse_file(
        &fixture("main_unknown_noprofiles.solid"),
        ParseOptions::default(),
    )
    .expect("must parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("prod".into());
    let (_, diags) = engine.evaluate(&module).expect("must evaluate");
    let d = diags.iter().find(|d| d.code == "W002").expect("{diags:?}");
    assert!(
        d.message.contains("'prod'"),
        "warning names the override: {d:?}"
    );
}

#[test]
fn sentinel_normalized_duplicate_is_e018() {
    // `"$"` and `"\$"` normalize to the same name: the second is a
    // duplicate, not a silent replacement.
    let src = "profile \"a$b\" {\n  x = 1\n}\nprofile \"a\\$b\" {\n  y = 2\n}\n";
    let diags = solid_core::parse(src, "dup").expect_err("normalized duplicate must fail");
    assert!(diags.iter().any(|d| d.code == "E018"), "{diags:?}");
}

#[test]
fn indent_style_profile_blocks_parse_and_merge() {
    // `profile "name":` + indented body must parse exactly like braces.
    let v = eval("main_indent_profile.solid");
    assert_eq!(v["server"]["port"], 200);
}

#[test]
fn profile_key_shadowing_base_key_resolves_to_base() {
    // `port = port` inside the profile: the self-reference exclusion is
    // lifted for the root fallback, so both keys read the base value.
    let v = eval("main_shadow.solid");
    assert_eq!(v["port"], 1);
    assert_eq!(v["other"], 1);
}

#[test]
fn unknown_selector_warns_even_without_profiles() {
    let module = parse_file(
        &fixture("main_unknown_noprofiles.solid"),
        ParseOptions::default(),
    )
    .expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.iter().any(|d| d.code == "W002"), "{diags:?}");
}

#[test]
fn no_profile_override_takes_precedence_over_invalid_root_selector() {
    let module = solid_core::parse("active_profile = 42\n", "override").expect("must parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("prod".into());
    let (_, diags) = engine.evaluate(&module).expect("must evaluate");
    let d = diags
        .iter()
        .find(|d| d.code == "W002")
        .expect("W002 expected");
    assert!(d.message.contains("'prod'"), "override is effective: {d:?}");
    assert!(
        !d.message.contains("must be a string"),
        "base selector is overridden: {d:?}"
    );
    assert!(d.span.is_none(), "CLI override has no source span: {d:?}");
}

#[test]
fn null_to_value_override_warns_w003() {
    let module = parse_file(
        &fixture("main_null_override.solid"),
        ParseOptions::default(),
    )
    .expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.iter().any(|d| d.code == "W003"), "{diags:?}");
    let json = solid_core::convert::value_to_pretty_json(&value);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["server"]["port"], 443);
}

#[test]
fn formatter_escapes_profile_names_that_look_like_interpolation() {
    let module = solid_core::parse("profile \"a\\${b}\" { x = 1 }\n", "fmt")
        .expect("escaped-dollar profile name must parse");
    let output = solid_core::fmt::format(&module, solid_core::Style::Brace);
    assert!(
        output.contains(r#"profile "a\${b}""#),
        "name is escaped: {output}"
    );
    let reparsed = solid_core::parse(&output, "fmt").expect("formatted output must parse");
    assert!(reparsed.profiles.contains_key("a${b}"));
}

#[test]
fn profile_blocks_round_trip_through_fmt() {
    for style in [solid_core::Style::Brace, solid_core::Style::Indent] {
        let module =
            parse_file(&fixture("main_select.solid"), ParseOptions::default()).expect("must parse");
        let out = solid_core::fmt::format(&module, style);
        assert!(out.contains("profile"), "profile blocks survive fmt: {out}");
        let module2 =
            solid_core::parse(&out, "fmt").unwrap_or_else(|d| panic!("fmt output parses: {d:?}"));
        assert_eq!(
            module2.profiles.len(),
            2,
            "both profiles survive fmt in {style:?}"
        );
        // Values survive the round trip under selection.
        let mut engine = Engine::new();
        engine.profile_override = Some("production".into());
        let (v2, _) = engine.evaluate(&module2).expect("must evaluate");
        let json = solid_core::convert::value_to_pretty_json(&v2);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["server"]["port"], 443, "values survive fmt in {style:?}");
    }
}
