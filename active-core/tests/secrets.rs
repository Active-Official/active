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
fn computed_schema_expressions_are_not_echoed() {
    use std::collections::HashMap;
    // A schema `media_type` can be an expression; B004 must not echo
    // its evaluated (possibly material) value.
    let src = "media = env.R15MEDIA\nk = secret(media)\nschema = { f = Blob(media_type = media) }\nf = blob(\"text/plain\", \"hi\")\n";
    let module = parse(src, "b004-computed").expect("must parse");
    let mut env = HashMap::new();
    env.insert(
        "R15MEDIA".to_string(),
        "ENVSECRET-expected-media".to_string(),
    );
    let (_, diagnostics) = Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let b004 = diagnostics
        .iter()
        .find(|d| d.code == "B004")
        .expect("B004 media mismatch");
    assert!(
        !b004.message.contains("ENVSECRET"),
        "B004 echoed a computed expectation: {}",
        b004.message
    );

    // The same rule for a computed `format` string (T003).
    let src =
        "fmt = env.R15FMT\nk = secret(fmt)\nschema = { s = String(format = fmt) }\ns = \"x\"\n";
    let module = parse(src, "t003-computed").expect("must parse");
    let mut env = HashMap::new();
    env.insert("R15FMT".to_string(), "ENVSECRET-format".to_string());
    let (_, diagnostics) = Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let t003 = diagnostics
        .iter()
        .find(|d| d.code == "T003")
        .expect("T003 unknown format");
    assert!(
        !t003.message.contains("ENVSECRET"),
        "T003 echoed a computed format: {}",
        t003.message
    );
}

#[test]
fn constraint_redaction_honors_the_profile_override() {
    // The engine override selects a profile the document does not name;
    // taint analysis must follow the override, not the literal selector.
    let src = "n = 4242\nk = secret(n)\nactive_profile = \"a\"\nprofile \"a\" { x = 1 }\nprofile \"b\" { x = n }\n# @constraint: value == 7\nx = 0\n";
    let module = parse(src, "t006-override").expect("must parse");
    let mut engine = Engine::hermetic();
    engine.profile_override = Some("b".to_string());
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
        "override material leaked into T006: {}",
        t006.message
    );
}

#[test]
fn computed_requires_target_is_not_echoed() {
    use std::collections::HashMap;
    let src = "req = env.R15REQ\nk = secret(req)\nschema = { s = String(requires = req), tls = Bool }\ntls = true\n";
    let module = parse(src, "t004-computed").expect("must parse");
    let mut env = HashMap::new();
    env.insert("R15REQ".to_string(), "tls".to_string());
    let (_, diagnostics) = Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let t004 = diagnostics
        .iter()
        .find(|d| d.code == "T004")
        .expect("T004 missing required key");
    assert!(
        !t004.message.contains("'tls'"),
        "T004 echoed a computed requires target: {}",
        t004.message
    );
    assert!(
        t004.message.contains("computed requires target"),
        "T004 did not take the computed branch: {}",
        t004.message
    );
}

#[test]
fn simple_toml_blob_media_is_escaped() {
    let module = parse("f = blob(\"text/pl\\\"ain\", \"hi\")\n", "toml-escape").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let out = active_core::convert::value_to_simple_toml(&value, "");
    assert!(
        out.contains("media_type = \"text/pl\\\"ain\""),
        "blob media not escaped as a TOML string: {out}"
    );
}

#[test]
fn unknown_computed_profile_selector_is_not_echoed() {
    use std::collections::HashMap;
    let src =
        "p = env.R14W\nk = secret(p)\nactive_profile = env.R14W\nprofile \"prod\" { x = 1 }\n";
    let module = parse(src, "w002-computed").expect("must parse");
    let mut env = HashMap::new();
    env.insert("R14W".to_string(), "ENVSECRET-profname".to_string());
    let (_, diagnostics) = Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let w002 = diagnostics
        .iter()
        .find(|d| d.code == "W002")
        .expect("W002 unknown profile");
    assert!(
        !w002.message.contains("ENVSECRET"),
        "W002 echoed a computed selector: {}",
        w002.message
    );
    assert!(w002.message.contains("computed"), "{}", w002.message);
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

#[test]
fn profile_override_marks_the_selected_locator_source() {
    // A profile override replaces the value at a material path. The
    // selected node is the locator source under that selector, so it is
    // material even though the base-tree node set only knows the
    // unprofiled literal. Exports must redact the selected value.
    let raw = "profile-s3cr3t-override";
    std::env::set_var("ACTIVE_PROFILE_SOURCE_TEST", raw);
    let src = "active_profile = \"a\"\nprofile \"a\" { n = \"benign\" }\nprofile \"b\" { n = env.ACTIVE_PROFILE_SOURCE_TEST }\nn = \"default\"\nk = secret(n)\n";
    let module = parse(src, "profile-source").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("b".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("b"));
    assert!(
        profile.path_is_secret("n"),
        "selected source must be material"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "selected material leaked: {json}");
    assert!(json.contains("***SECRET***"), "{json}");
}

#[test]
fn computed_selector_unions_override_material_paths() {
    // A computed selector cannot be resolved, so every profile
    // contributes material paths; an override-only material path must
    // stay material (fail closed) instead of reading as benign.
    let src = "active_profile = env.ACTIVE_PROFILE_SELECTOR_TEST\nprofile \"benign\" { n = \"public\" }\nprofile \"material\" { n = secret(\"computed-s3cr3t\") }\nn = \"default\"\nk = n\n";
    let module = parse(src, "profile-computed").expect("parse");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(
        profile.path_is_secret("n"),
        "computed selector must union override material"
    );
}

#[test]
fn deep_profile_alias_chain_keeps_material_paths() {
    // F1 (round 22): a profile value eight references deep is still a
    // table, so the merge must keep the base material path. The old
    // 8-hop cap classified it as a scalar, removed the path, and the
    // evaluator's merge then exported the raw value.
    let raw = "chain-s3cr3t-round22";
    std::env::set_var("ACTIVE_CHAIN_SECRET", raw);
    let mut src =
        String::from("let p = env.ACTIVE_CHAIN_SECRET\nk = secret(p)\nlet r1 = { b = 2 }\n");
    for i in 2..=8 {
        src.push_str(&format!("let r{i} = r{}\n", i - 1));
    }
    src.push_str("x = { a = p, b = 1 }\nactive_profile = \"a\"\nprofile \"a\" { x = r8 }\n");
    let module = parse(&src, "chain").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("a".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("a"));
    assert!(
        profile.path_is_secret("x.a"),
        "merged base path stays material"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "deep alias chain leaked: {json}");
}

#[test]
fn array_element_schema_default_material_is_redacted() {
    // F2 (round 22): a fill inside an array element lands at
    // `arr[0].a`, not `arr.a`. The wildcard registration keeps the
    // default expression material at every index.
    let raw = "arr-default-s3cr3t-round22";
    std::env::set_var("ACTIVE_ARR_DEFAULT", raw);
    let src = "let s = env.ACTIVE_ARR_DEFAULT\nk = secret(s)\nschema = { arr = Array<Table { a = String(default = s) }> }\narr = [{}]\n";
    let module = parse(src, "array-default").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(
        profile.path_is_secret("arr[0].a"),
        "filled element is material"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "array default leaked: {json}");
    assert!(json.contains("***SECRET***"), "{json}");
}

#[test]
fn profile_container_replacement_redacts_descendants() {
    // F3 (round 22): a profile replaces a scalar material path with a
    // table. Every descendant of that replacement is material; the old
    // exact-node marking published the leaves.
    let secret = "container-source-s3cr3t-round22";
    let live = "container-value-s3cr3t-round22";
    std::env::set_var("ACTIVE_CONTAINER_SOURCE", secret);
    std::env::set_var("ACTIVE_CONTAINER_VALUE", live);
    let src = "p = env.ACTIVE_CONTAINER_SOURCE\nk = secret(p)\nx = p\nactive_profile = \"a\"\nprofile \"a\" { x = { value = env.ACTIVE_CONTAINER_VALUE } }\n";
    let module = parse(src, "container").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("a".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("a"));
    assert!(
        profile.path_is_secret("x"),
        "replacement container is material"
    );
    assert!(profile.path_is_secret("x.value"), "descendant is material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(
        !json.contains(live),
        "replacement descendant leaked: {json}"
    );
    assert!(!json.contains(secret), "source leaked: {json}");
}

#[test]
fn truncated_base_walk_stays_fail_closed_under_profile() {
    // F6 (round 22): once the base walk hits MAX_PATHS, profile paths
    // must not register as known-benign. The material override below
    // sits past the cap and must still read as material.
    let mut src = String::from("big = [");
    for i in 0..100_005 {
        if i > 0 {
            src.push(',');
        }
        src.push('0');
    }
    src.push_str("]\nlet p = env.ACTIVE_TRUNCATED\nt = secret(p)\nx = \"benign\"\nactive_profile = \"a\"\nprofile \"a\" { y = \"MATERIAL-LEAK\" }\n");
    let module = parse(&src, "truncated").expect("parse");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("a"));
    assert!(
        profile.path_is_secret("y"),
        "profile path past a truncated walk must fail closed"
    );
}

#[test]
fn escaped_dollar_profile_name_is_static() {
    // F13 (round 22): an escaped dollar parses as an all-literal
    // interpolation; the selector is still static and recorded.
    let src = "active_profile = \"\\$prod\"\nprofile \"$prod\" { x = 1 }\nx = 0\n";
    let module = parse(src, "dollar-profile").expect("parse");
    assert_eq!(
        active_core::secret::active_profile_name(&module).as_deref(),
        Some("$prod")
    );
}

#[test]
fn nested_profile_replacement_under_a_merge_redacts_descendants() {
    // N1 (round 23): the top-level field merges, but a nested value
    // replaces a scalar material path. Its subtree is material.
    let src_secret = "nested-source-r23";
    let live = "nested-live-r23";
    std::env::set_var("ACTIVE_N1_SRC", src_secret);
    std::env::set_var("ACTIVE_N1_LIVE", live);
    let src = "let p = env.ACTIVE_N1_SRC\nk = secret(p)\nx = { a = p }\nactive_profile = \"a\"\nprofile \"a\" { x = { a = { value = env.ACTIVE_N1_LIVE } } }\n";
    let module = parse(src, "nested-profile").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("a".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("a"));
    assert!(
        profile.path_is_secret("x.a"),
        "nested replacement is material"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(live), "nested replacement leaked: {json}");
    assert!(!json.contains(src_secret), "source leaked: {json}");
}

#[test]
fn array_inside_a_schema_default_is_material_at_every_index() {
    // N2 (round 23): a default that itself contains an array registers
    // concrete sub-indices; the wildcard alias must match the query.
    let raw = "default-array-r23";
    std::env::set_var("ACTIVE_N2_ARR", raw);
    let src = "let s = env.ACTIVE_N2_ARR\nk = secret(s)\nschema = { arr = Array<Table { a = Any(default = [s]) }> }\narr = [{}]\n";
    let module = parse(src, "default-array").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(
        profile.path_is_secret("arr[0].a[0]"),
        "array default element is material"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "array default leaked: {json}");
}

#[test]
fn computed_selector_marks_nodes_for_provenance_and_traces() {
    // N5 (round 23): path-only fallback left `node_is_secret` false, so
    // provenance and step traces published the computed selection.
    let src = "let p = env.ACTIVE_N5_SRC\nk = secret(p)\nsel = env.ACTIVE_N5_SEL\nx = p\nactive_profile = sel\nprofile \"b\" { x = env.ACTIVE_N5_LIVE }\n";
    let module = parse(src, "computed-nodes").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_N5_SRC".to_string(), "source-r23".to_string());
    env.insert("ACTIVE_N5_SEL".to_string(), "b".to_string());
    env.insert(
        "ACTIVE_N5_LIVE".to_string(),
        "computed-live-r23".to_string(),
    );
    let engine = Engine::new().with_env(env.clone());
    let _ = engine.evaluate(&module).expect("evaluate");
    let provenance =
        active_core::provenance::collect_provenance(&module, &env, None, None).expect("provenance");
    let live = provenance
        .env
        .iter()
        .find(|input| input.name == "ACTIVE_N5_LIVE")
        .expect("live recorded");
    assert!(live.secret, "computed selection is secret-bound");
    assert_eq!(live.value, None, "computed selection value withheld");
}

#[test]
fn truncated_profile_override_of_an_existing_path_stays_material() {
    // N3 (round 23): the base path already exists and is benign, so
    // only the fallback set can keep the override material after a
    // truncated walk.
    let mut src = String::from("big = [");
    for i in 0..100_005 {
        if i > 0 {
            src.push(',');
        }
        src.push('0');
    }
    src.push_str("]\nlet p = env.ACTIVE_N3\nt = secret(p)\nx = \"benign\"\nactive_profile = \"a\"\nprofile \"a\" { x = \"MATERIAL-LEAK\" }\n");
    let module = parse(&src, "truncated-existing").expect("parse");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("a"));
    assert!(
        profile.path_is_secret("x"),
        "existing path overridden past a truncated walk must stay material"
    );
}

#[test]
fn profile_override_of_a_material_default_is_material() {
    // N4 (round 23): a schema default can be the material source; a
    // profile that overrides that path before the fill runs replaces a
    // material value, so the default's materiality joins the base map.
    let source = "default-source-r23";
    let live = "default-live-r23";
    std::env::set_var("ACTIVE_N4_SRC", source);
    std::env::set_var("ACTIVE_N4_LIVE", live);
    let src = "let s = env.ACTIVE_N4_SRC\nk = secret(s)\nschema = { a = String(default = s) }\nactive_profile = \"a\"\nprofile \"a\" { a = env.ACTIVE_N4_LIVE }\n";
    let module = parse(src, "default-override").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("a".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("a"));
    assert!(
        profile.path_is_secret("a"),
        "material default path is material"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(live), "default override leaked: {json}");
}

#[test]
fn env_fallback_container_children_are_material_paths() {
    // F1 (round 24): `env.X ?: {…}` keeps the Env node at the path but
    // its fallback's children are real value paths. A material child
    // inside the fallback must be mapped or it exports raw.
    let raw = "env-default-s3cr3t-r24";
    std::env::set_var("ACTIVE_R24_SRC", raw);
    std::env::remove_var("ACTIVE_R24_UNSET");
    let src = "let p = env.ACTIVE_R24_SRC\nk = secret(p)\nx = env.ACTIVE_R24_UNSET ?: { s = p, b = \"VISIBLE-SIBLING\" }\ny = env.ACTIVE_R24_UNSET ?: [p, \"VISIBLE-ELEMENT\"]\n";
    let module = parse(src, "env-default").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R24_SRC".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("x.s"), "fallback child mapped");
    assert!(profile.path_is_secret("y[0]"), "fallback element mapped");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "fallback leaked: {json}");
    assert!(json.contains("VISIBLE-SIBLING"), "sibling hidden: {json}");
    assert!(json.contains("VISIBLE-ELEMENT"), "element hidden: {json}");
}

#[test]
fn material_key_is_not_auto_renamed_by_resilient_healing() {
    // F2 (round 24): H005 may not move a material value to a target
    // path the taint walk cannot follow. The rename degrades to a
    // suggestion and the value stays redacted at its source path.
    let raw = "h005-s3cr3t-r24";
    std::env::set_var("ACTIVE_R24_H005", raw);
    let src = "let p = env.ACTIVE_R24_H005\nk = secret(p)\nschema = { closed = true, password = String, k = Secret }\npasswrod = p\n";
    let module = parse(src, "h005-material").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R24_H005".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("passwrod"), "source stays material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "H005 rename leaked: {json}");
    assert!(
        !json.contains("\"password\""),
        "material key was renamed: {json}"
    );
}

#[test]
fn binding_names_do_not_make_profile_keys_material() {
    // F3 (round 24): bindings are a separate namespace. A profile key
    // colliding with a `let` name is a legitimate new data key.
    let raw = "collision-s3cr3t-r24";
    std::env::set_var("ACTIVE_R24_COLL", raw);
    let src = "let p = env.ACTIVE_R24_COLL\nk = secret(p)\nactive_profile = \"a\"\nprofile \"a\" { p = \"PROFILE-P-VISIBLE\" }\n";
    let module = parse(src, "binding-collision").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("a".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("a"));
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(
        json.contains("PROFILE-P-VISIBLE"),
        "benign key hidden: {json}"
    );
    assert!(!json.contains(raw), "source leaked: {json}");
}

#[test]
fn explicit_value_wins_over_default_materiality() {
    // F4 (round 24): a default only fills an absent path, so its
    // materiality must not redact an explicit value overridden by a
    // benign profile.
    let raw = "default-src-r24";
    std::env::set_var("ACTIVE_R24_DEFAULT", raw);
    let src = "let s = env.ACTIVE_R24_DEFAULT\nk = secret(s)\nschema = { a = String(default = s) }\na = \"EXPLICIT-BENIGN\"\nactive_profile = \"a\"\nprofile \"a\" { a = \"PROFILE-BENIGN\" }\n";
    let module = parse(src, "default-covered").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("a".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("a"));
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(
        json.contains("PROFILE-BENIGN"),
        "benign override hidden: {json}"
    );
    assert!(!json.contains(raw), "source leaked: {json}");
}

#[test]
fn annotated_table_children_stay_mapped() {
    // A transparent wrapper (ApplyConstraint) preserves the value at
    // the path; its children must still be addressable, or a material
    // child inside an annotated table exports raw.
    let raw = "annotated-s3cr3t-r25";
    std::env::set_var("ACTIVE_R25_ANN", raw);
    let src = "let p = env.ACTIVE_R25_ANN\nk = secret(p)\n# @constraint: value != null\nx = { s = p, b = 1 }\n";
    let module = parse(src, "annotated-table").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R25_ANN".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("x.s"), "annotated child mapped");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "annotated table leaked: {json}");
}
