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
        // Value-free by construction: operands render as kind names.
        assert!(
            message.contains("secret"),
            "kind name missing in error: {message}"
        );
    }
}

#[test]
fn redaction_markers_are_opaque_and_stable() {
    // A secret whose text is a substring of the marker ("SECRET") or
    // the marker's padding ("*") must not rewrite the marker itself.
    let src = "x = \"SECRET\"\nk = secret(x)\nstar = \"*\"\nj = secret(star)\n";
    let module = parse(src, "marker").expect("must parse");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert_eq!(
        profile.redact_text("***SECRET***"),
        None,
        "an existing marker must be opaque"
    );
    assert_eq!(
        profile.redact_text("prefix ***SECRET*** suffix"),
        None,
        "markers inside text stay intact"
    );
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    for path in ["x", "star"] {
        let report = active_core::convert::inspect_path(&module, &value, path).expect("inspect");
        assert_eq!(report["value"], "***SECRET***", "path {path}: {report}");
    }
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module)
        .expect("must evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("******"),
        "marker was corrupted by redaction: {rendered}"
    );
}

#[test]
fn blob_media_diagnostics_and_inspection_redact_material() {
    use std::collections::HashMap;
    let src = "schema = { f = Blob(media_type = \"image/png\") }\nm = env.BLOB_MEDIA\nk = secret(m)\nf = blob(m, \"data\")\n";
    let module = parse(src, "blob-media-redact").expect("must parse");
    let mut env = HashMap::new();
    env.insert("BLOB_MEDIA".to_string(), "ENVSECRET-media".to_string());
    let (value, diagnostics) = Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let b004 = diagnostics
        .iter()
        .find(|d| d.code == "B004")
        .expect("B004 media mismatch");
    assert!(
        !b004.message.contains("ENVSECRET-media"),
        "B004 leaked the evaluated media type: {}",
        b004.message
    );
    let report =
        active_core::convert::inspect_path(&module, &value, "f").expect("inspect the blob");
    assert_eq!(report["tainted"], true, "{report}");
    assert_eq!(
        report["content_addressed"]["media_type"], "***SECRET***",
        "{report}"
    );
    assert!(!report.to_string().contains("ENVSECRET"), "{report}");
}

#[test]
fn constraint_diagnostics_redact_non_string_material() {
    let src = "let x = 4242\nk = secret(x)\n# @constraint: value == 7\nt = x\n";
    let module = parse(src, "t006-number").expect("must parse");
    let mut engine = Engine::new();
    engine.resilient = false;
    let failure = engine.evaluate(&module).expect_err("constraint fails");
    let active_core::EvalError::ValidationFailed { diagnostics } = failure else {
        panic!("expected validation failure, got {failure}");
    };
    let t006 = diagnostics
        .iter()
        .find(|d| d.code == "T006")
        .expect("T006 diagnostic");
    let actual = t006
        .message
        .split("actual: ")
        .nth(1)
        .unwrap_or_else(|| panic!("no actual value in {}", t006.message))
        .trim_end_matches(')');
    assert_eq!(
        actual, "***SECRET***",
        "non-string material leaked into T006: {}",
        t006.message
    );
}

#[test]
fn tainted_blob_media_is_redacted_in_every_export() {
    use std::collections::HashMap;
    let src = "m = env.R13BLOB\nk = secret(m)\nf = blob(m, \"data\")\n";
    let module = parse(src, "blob-media-exports").expect("must parse");
    let mut env = HashMap::new();
    env.insert("R13BLOB".to_string(), "ENVSECRET-media".to_string());
    let (value, _) = Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let json = active_core::convert::value_to_json(&value);
    // The root also carries the authored `m` value (source-visible);
    // the blob identity is the publishing surface under test.
    assert!(!json["f"].to_string().contains("ENVSECRET"), "{json}");
    let active_core::Value::Table(map) = &value else {
        panic!("expected a table");
    };
    let blob = map.get("f").expect("blob value");
    let toml = active_core::convert::value_to_toml_string(blob)
        .expect("toml")
        .0;
    assert!(!toml.contains("ENVSECRET"), "{toml}");
    assert!(toml.contains("***SECRET***"), "{toml}");
    let debug = format!("{blob:?}");
    assert!(!debug.contains("ENVSECRET"), "Debug leaked: {debug}");
}

#[test]
fn constraint_diagnostics_redact_interpolated_material() {
    // T006 renders the actual value; an interpolation over a secret
    // source is material even though the string has no marker.
    let src = "x = \"hunter2-t006\"\nk = secret(x)\n# @constraint: value != \"prefix hunter2-t006\"\nmsg = \"prefix ${x}\"\n";
    let module = parse(src, "t006").expect("must parse");
    let mut engine = Engine::new();
    engine.resilient = false;
    let failure = engine.evaluate(&module).expect_err("constraint fails");
    let active_core::EvalError::ValidationFailed { diagnostics } = failure else {
        panic!("expected validation failure, got {failure}");
    };
    let t006 = diagnostics
        .iter()
        .find(|d| d.code == "T006")
        .expect("T006 diagnostic");
    // The predicate source is authored text (the author can read it in
    // the file); the evaluated actual value is the publishing surface.
    let actual = t006
        .message
        .split("actual: ")
        .nth(1)
        .unwrap_or_else(|| panic!("no actual value in {}", t006.message));
    assert!(
        !actual.contains("hunter2"),
        "constraint message leaked material: {}",
        t006.message
    );
    assert!(
        actual.contains("***SECRET***"),
        "constraint message must carry the marker: {}",
        t006.message
    );
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
