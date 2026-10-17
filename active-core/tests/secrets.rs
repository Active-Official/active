//! Secret taint tracking and redaction audit (C-035/C-036,
//! spec/secrets.md).
//!
//! Drives the real shipped path (`parse_file`/`parse` → evaluate →
//! JSON/TOML export) with secrets in every position: origins,
//! propagation through tables/arrays/profiles/patches/fills,
//! interpolation refusal (S004), arithmetic prohibition (T001),
//! equality semantics, export redaction, and a diagnostics audit
//! asserting no rendered output ever contains raw secret material.

use active_core::{parse, parse_file, Engine, EvalError, ParseOptions};
use std::path::{Path, PathBuf};

const RAW_A: &str = "db-s3cr3t-001";
const RAW_B: &str = "db-s3cr3t-002";
const RAW_C: &str = "db-s3cr3t-003";
const RAW_ENV: &str = "env-s3cr3t-004";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("secrets")
        .join(name)
}

fn eval_resilient_file(name: &str) -> (serde_json::Value, String, Vec<active_core::Diagnostic>) {
    let module = parse_file(&fixture(name), ParseOptions::default())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: resilient must succeed: {e:?}"));
    let json = active_core::convert::value_to_pretty_json(&value);
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    (v, json, diags)
}

fn eval_inline(src: &str) -> Result<(serde_json::Value, Vec<active_core::Diagnostic>), EvalError> {
    let module = parse(src, "secrets").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    Engine::new().evaluate(&module).map(|(value, diags)| {
        let json = active_core::convert::value_to_pretty_json(&value);
        (serde_json::from_str(&json).expect("valid json"), diags)
    })
}

#[test]
fn secrets_propagate_through_every_shape_and_redact() {
    let (v, json, diags) = eval_resilient_file("propagate.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(!json.contains(RAW_A), "raw secret in JSON: {json}");
    assert!(json.contains("***SECRET***"), "{json}");
    // Structure survives redaction: keys, nesting, and positions.
    assert_eq!(v["database"]["ports"][0], 5432);
    assert_eq!(v["database"]["ports"][1], "***SECRET***");
    assert_eq!(v["database"]["meta"]["token"], "***SECRET***");
    assert_eq!(v["database"]["password"], "***SECRET***");
    // TOML export redacts too.
    let module =
        parse_file(&fixture("propagate.active"), ParseOptions::default()).expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let toml = active_core::convert::value_to_simple_toml(&value, "");
    assert!(!toml.contains(RAW_A), "raw secret in TOML: {toml}");
}

#[test]
fn env_backed_secrets_redact() {
    // Unique variable name: no other test reads it, so parallel
    // execution cannot observe the mutation. The prior value (if any)
    // is restored afterwards.
    let prior = std::env::var("ACTIVE_TEST_E2E_SECRET").ok();
    std::env::set_var("ACTIVE_TEST_E2E_SECRET", RAW_ENV);
    // Prove the variable was actually read: the evaluated value
    // holds the raw secret pre-export.
    let module = parse(
        "token = secret.env(\"ACTIVE_TEST_E2E_SECRET\")\n",
        "secrets",
    )
    .expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    match &value {
        active_core::Value::Table(map) => match map.get("token") {
            Some(active_core::Value::Secret(s)) => assert_eq!(s, RAW_ENV, "env not read"),
            other => panic!("secret expected, got {other:?}"),
        },
        other => panic!("table expected, got {other:?}"),
    }
    let (v, json, _) = eval_resilient_file("propagate.active");
    match prior {
        Some(prev) => std::env::set_var("ACTIVE_TEST_E2E_SECRET", prev),
        None => std::env::remove_var("ACTIVE_TEST_E2E_SECRET"),
    }
    assert_eq!(v["service"]["api_key"], "***SECRET***");
    assert!(!json.contains(RAW_ENV), "env secret in JSON: {json}");
}

#[test]
fn profile_and_patch_secrets_redact() {
    let (v, json, diags) = eval_resilient_file("patch_secret.active");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(v["database"]["password"], "***SECRET***");
    assert!(!json.contains(RAW_C), "raw secret in JSON: {json}");
    // A string→secret profile override is a type change (W003, values
    // never quoted) and still redacts.
    let (v, json, diags) = eval_resilient_file("profile_secret.active");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "W003");
    assert!(!diags[0].message.contains(RAW_B), "{:?}", diags[0]);
    assert_eq!(v["database"]["password"], "***SECRET***");
    assert!(!json.contains(RAW_B), "raw secret in JSON: {json}");
}

#[test]
fn interpolation_refuses_secrets() {
    for src in [
        "let pw = secret(\"s3cr3t-i1\")\nurl = \"https://${pw}\"\n",
        "url = \"https://${secret('s3cr3t-i2')}\"\n",
    ] {
        let module = parse(src, "secrets").expect("must parse");
        let err = Engine::new().evaluate(&module).expect_err("must refuse");
        assert_eq!(err.code(), "S004");
    }
}

#[test]
fn arithmetic_and_ordering_on_secrets_fail_redacted() {
    for (src, raw) in [
        ("let pw = secret(\"s3cr3t-a1\")\nx = pw + 1\n", "s3cr3t-a1"),
        ("let pw = secret(\"s3cr3t-a2\")\nx = pw * 2\n", "s3cr3t-a2"),
        ("let pw = secret(\"s3cr3t-a3\")\nx = pw > 1\n", "s3cr3t-a3"),
        ("let pw = secret(\"s3cr3t-a4\")\nx = -pw\n", "s3cr3t-a4"),
    ] {
        let module = parse(src, "secrets").expect("must parse");
        let err = Engine::new().evaluate(&module).expect_err("must fail");
        assert_eq!(err.code(), "T001");
        let message = err.to_string();
        assert!(!message.contains(raw), "raw secret in error: {message}");
        assert!(message.contains("Secret(***)"), "{message}");
    }
}

#[test]
fn secret_equality_never_confirms() {
    // Anti-oracle rule (spec/secrets.md §5): no guess is ever confirmed.
    let (v, _) = eval_inline(
        "let a = secret(\"s3cr3t-e1\")\nlet b = secret(\"s3cr3t-e1\")\nsame = a == b\ndiff = a != b\nvs = a == \"nope\"\n",
    )
    .expect("equality must evaluate");
    assert_eq!(v["same"], false);
    assert_eq!(v["diff"], true);
    assert_eq!(v["vs"], false);
}

#[test]
fn schema_secret_type_and_secret_bounds() {
    // A secret satisfies a Secret field; anything else is T002.
    let (v, diags) =
        eval_inline("schema = { t = Secret }\nt = secret(\"s3cr3t-s1\")\n").expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(v["t"], "***SECRET***");
    let err_src = "schema = { t = Secret }\nt = \"plain\"\n";
    let module = parse(err_src, "secrets").expect("must parse");
    let mut strict = Engine::new();
    strict.resilient = false;
    match strict.evaluate(&module) {
        Err(EvalError::ValidationFailed { diagnostics }) => {
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].code, "T002");
        }
        other => panic!("must fail strict: {other:?}"),
    }
    // A secret bound never echoes: incomparable, structural message.
    let module = parse(
        "let pw = secret(\"s3cr3t-b1\")\nschema = { n = Int(min = pw) }\nn = 5\n",
        "secrets",
    )
    .expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T003");
    assert!(!diags[0].message.contains("s3cr3t-b1"), "{:?}", diags[0]);
}

#[test]
fn secret_defaults_fill_redacted() {
    let src = "schema = { t = Secret(default = secret(\"s3cr3t-d1\")) }\n";
    let (v, diags) = eval_inline(src).expect("resilient must succeed");
    assert_eq!(v["t"], "***SECRET***");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "W005");
    assert!(!diags[0].message.contains("s3cr3t-d1"), "{:?}", diags[0]);
}

#[test]
fn diagnostics_audit_never_shows_raw_secrets() {
    // Every diagnostic path exercised with live secrets: collect all
    // rendered text (messages, codes, spans omitted) and assert the
    // raw material appears nowhere.
    let mut rendered = String::new();
    let mut check = |src: &str, raw: &str| {
        let outcome = parse(src, "secrets").map(|module| Engine::new().evaluate(&module));
        match outcome {
            Ok(Ok((_, diags))) => {
                for d in &diags {
                    rendered.push_str(&d.message);
                }
            }
            Ok(Err(e)) => {
                rendered.push_str(&e.to_string());
            }
            Err(diags) => {
                for d in &diags {
                    rendered.push_str(&d.message);
                }
            }
        }
        assert!(!rendered.contains(raw), "leak for {src:?}: {rendered}");
        rendered.clear();
    };
    check("let pw = secret(\"s3cr3t-x1\")\nx = pw + 1\n", "s3cr3t-x1");
    check(
        "let pw = secret(\"s3cr3t-x2\")\nurl = \"${pw}\"\n",
        "s3cr3t-x2",
    );
    check(
        "let pw = secret(\"s3cr3t-x3\")\nschema = { n = Int }\nn = pw\n",
        "s3cr3t-x3",
    );
    check(
        "let pw = secret(\"s3cr3t-x4\")\n# @constraint: value == 1\nn = pw\n",
        "s3cr3t-x4",
    );
    check(
        "let pw = secret(\"s3cr3t-x5\")\nschema = { s = String(pattern = pw) }\ns = \"a\"\n",
        "s3cr3t-x5",
    );
}

#[test]
fn redaction_is_deterministic() {
    let (_, json_a, _) = eval_resilient_file("propagate.active");
    let (_, json_b, _) = eval_resilient_file("propagate.active");
    assert_eq!(json_a, json_b);
}

#[test]
fn generic_debug_and_serialize_never_emit_secrets() {
    // The public footguns from the review: `{:?}` and direct serde
    // serialization of a Value must redact at every depth.
    let module =
        parse_file(&fixture("propagate.active"), ParseOptions::default()).expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let debug = format!("{value:?}");
    assert!(!debug.contains(RAW_A), "raw secret in Debug: {debug}");
    assert!(debug.contains("Secret(***)"), "{debug}");
    let direct = serde_json::to_string(&value).expect("must serialize");
    assert!(!direct.contains(RAW_A), "raw secret in Serialize: {direct}");
    assert!(direct.contains("***SECRET***"), "{direct}");
    // Scalars keep their derived shapes through the custom impl.
    assert_eq!(format!("{:?}", active_core::Value::Int(7)), "7");
    assert_eq!(format!("{:?}", active_core::Value::Null), "Null");
}
