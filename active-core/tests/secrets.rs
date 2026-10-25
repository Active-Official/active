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
    let (value, diagnostics, seeds) = Engine::hermetic()
        .with_env(env.clone())
        .evaluate_with_seeds(&module)
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
    let report = active_core::convert::inspect_path_with_profile_env(
        &module,
        &value,
        "f",
        None,
        Some(&env),
        Some(&seeds),
    )
    .expect("inspect the blob");
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
fn profile_override_of_a_material_default_answers_for_itself() {
    // A profile that overrides a defaulted path replaces the fill, so
    // the value's own materiality decides. A plain env read is benign;
    // a material expression at the same path is still redacted (the
    // sibling tests cover that direction).
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
        !profile.path_is_secret("a"),
        "the profile answer is a benign env read"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(json.contains(live), "benign answer hidden: {json}");
    assert!(!json.contains(source), "the default source leaked: {json}");
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
fn truncated_profile_walk_invalidates_known_base_paths() {
    // F1 (round 25): when the profile walk itself trips MAX_PATHS, a
    // partially registered override must not leave the base path
    // known-benign. The overridden key is invalidated as a whole.
    let mut elements = String::from("[");
    for i in 0..50_000 {
        if i > 0 {
            elements.push(',');
        }
        elements.push('0');
    }
    elements.push(']');
    let src = format!(
        "let p = env.ACTIVE_R25W\nlet base = {elements}\nk = secret(p)\nx = {{ zzz = \"BENIGN\" }}\nactive_profile = \"a\"\nprofile \"a\" {{ x = {{ zzz = p, big = [base, base] }} }}\n"
    );
    let module = parse(&src, "wide-profile").expect("parse");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("a"));
    assert!(
        profile.path_is_secret("x.zzz"),
        "partially walked override must fail closed"
    );
}

#[test]
fn binding_shadowed_default_fill_is_material() {
    // F2 (round 25): a `let` with the same name does not occupy the
    // exported path, so the evaluator fills it. The fill node must win
    // over the binding placeholder.
    let raw = "default-shadow-s3cr3t-r25";
    std::env::set_var("ACTIVE_R25_SHADOW", raw);
    let src = "let s = env.ACTIVE_R25_SHADOW\nk = secret(s)\nlet t = \"BENIGN\"\nschema = { t = String(default = s) }\n";
    let module = parse(src, "binding-shadow").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R25_SHADOW".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("t"), "filled path is material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "binding shadow leaked: {json}");
}

#[test]
fn explicit_array_element_stays_visible_under_a_material_default() {
    // F3 (round 25): a wildcard default fills only absent elements; an
    // explicitly written element stays visible.
    let raw = "array-default-src-r25";
    std::env::set_var("ACTIVE_R25_ARR", raw);
    let src = "let s = env.ACTIVE_R25_ARR\nk = secret(s)\nschema = { arr = Array<Table { a = String(default = s) }> }\narr = [{ a = \"EXPLICIT-BENIGN\" }]\n";
    let module = parse(src, "array-covered").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R25_ARR".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("strict evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(
        !profile.path_is_secret("arr[0].a"),
        "explicit element must stay visible"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(json.contains("EXPLICIT-BENIGN"), "element hidden: {json}");
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
fn binding_rename_source_does_not_clear_tree_materiality() {
    // F1 (round 26): a `@rename` whose source is a binding is a W004
    // no-op in the evaluator. The taint map must not move the
    // destination value (which stays material).
    let raw = "rename-binding-s3cr3t-r26";
    std::env::set_var("ACTIVE_R26_REN", raw);
    let src = "let s = env.ACTIVE_R26_REN\nk = secret(s)\nlet typo = \"BENIGN\"\nreal = s\n@rename(\"typo\", \"real\")\n";
    let module = parse(src, "rename-binding").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R26_REN".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("real"), "destination stays material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "binding rename leaked: {json}");
}

#[test]
fn non_canonical_patch_indices_stay_mapped() {
    // F2 (round 26): `arr[00]`/`arr[+0]` are accepted by the evaluator
    // and canonicalize to index 0. The taint map keys the same
    // canonical path, so the patched value stays material.
    let raw = "noncanonical-s3cr3t-r26";
    std::env::set_var("ACTIVE_R26_IDX", raw);
    let src = "let s = env.ACTIVE_R26_IDX\nk = secret(s)\narr = [\"BASE-BENIGN\"]\n@patch(\"arr[00]\", s)\n";
    let module = parse(src, "noncanonical").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R26_IDX".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("arr[0]"), "canonical path material");
    assert!(profile.path_is_secret("arr[00]"), "query canonicalizes");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "non-canonical patch leaked: {json}");
}

#[test]
fn profile_override_with_a_material_expression_is_redacted() {
    // The converse of the benign-answer case: a profile that names the
    // material source itself is material on its own.
    let source = "default-source-r27";
    std::env::set_var("ACTIVE_R27_SRC", source);
    let src = "let s = env.ACTIVE_R27_SRC\nk = secret(s)\nschema = { a = String(default = \"x\") }\nactive_profile = \"a\"\nprofile \"a\" { a = s }\n";
    let module = parse(src, "default-override-material").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("a".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("a"));
    assert!(profile.path_is_secret("a"), "material override is material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(source), "material override leaked: {json}");
}

#[test]
fn rename_to_a_numeric_key_keeps_materiality() {
    // F1 (round 27): the evaluator accepts a numeric target key.
    // Canonicalization must not drop the rename and clear the taint.
    let raw = "numeric-target-s3cr3t-r27";
    std::env::set_var("ACTIVE_R27_NUM", raw);
    let src = "let s = env.ACTIVE_R27_NUM\nk = secret(s)\na = s\n@rename(\"a\", \"123\")\n";
    let module = parse(src, "rename-numeric").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R27_NUM".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("123"), "renamed target is material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "numeric rename leaked: {json}");
}

#[test]
fn profile_created_array_keeps_explicit_elements_visible() {
    // F2 (round 27): a profile that supplies the whole array answers
    // for its own elements; a material default for absent elements
    // must not hide an explicitly written benign leaf.
    let raw = "profile-array-src-r27";
    std::env::set_var("ACTIVE_R27_ARR", raw);
    let src = "let s = env.ACTIVE_R27_ARR\nk = secret(s)\nschema = { arr = Array<Table { a = String(default = s) }> }\nactive_profile = \"p\"\nprofile \"p\" { arr = [{}, { a = \"EXPLICIT-BENIGN\" }] }\n";
    let module = parse(src, "profile-array").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("p".to_string());
    let (value, _) = engine.evaluate(&module).expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("p"));
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(
        json.contains("EXPLICIT-BENIGN"),
        "explicit element hidden: {json}"
    );
    assert!(!json.contains(raw), "source leaked: {json}");
}

#[test]
fn profile_replacement_does_not_taint_shared_binding_targets() {
    // F3 (round 27): marking a replacement must not walk a shared Ref
    // target and hide an unrelated benign binding.
    let src = "let b = \"BENIGN\"\nlet s = env.ACTIVE_R27_SHARE\nk = secret(s)\nt = s\nu = b\nactive_profile = \"p\"\nprofile \"p\" { t = b }\n";
    let module = parse(src, "shared-binding").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("p".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("p"));
    assert!(
        profile.path_is_secret("t"),
        "replacement path stays material"
    );
    assert!(
        !profile.path_is_secret("u"),
        "shared binding must stay visible"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(
        json.contains("\"u\": \"BENIGN\""),
        "benign binding hidden: {json}"
    );
}

#[test]
fn binding_shared_env_at_a_material_path_is_withheld_by_provenance() {
    // F1 (review 29): a profile may name a binding that reads env.LIVE
    // at a material path. The env value must be withheld even though
    // the same node is also a benign binding.
    let src = "let e = env.ACTIVE_R29_LIVE\nlet p = env.ACTIVE_R29_SRC\nk = secret(p)\nn = p\nactive_profile = \"p\"\nprofile \"p\" { n = e }\n";
    let module = parse(src, "env-binding").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R29_LIVE".to_string(), "live-r29".to_string());
    env.insert("ACTIVE_R29_SRC".to_string(), "source-r29".to_string());
    let engine = Engine::new().with_env(env.clone());
    let _ = engine.evaluate(&module).expect("evaluate");
    let provenance = active_core::provenance::collect_provenance(&module, &env, None, Some("p"))
        .expect("provenance");
    let live = provenance
        .env
        .iter()
        .find(|input| input.name == "ACTIVE_R29_LIVE")
        .expect("live recorded");
    assert!(live.secret, "env at a material path is secret-bound");
    assert_eq!(live.value, None, "value withheld");
}

#[test]
fn data_node_shared_with_a_profile_value_stays_visible() {
    // F2 (review 29): a benign data key referenced from a material
    // profile path is not globally tainted; only the profile path is.
    let raw = "shared-data-s3cr3t-r29";
    std::env::set_var("ACTIVE_R29_SHARE", raw);
    let src = "let s = env.ACTIVE_R29_SHARE\nk = secret(s)\nn = s\ny = \"BENIGN-DATA\"\nactive_profile = \"p\"\nprofile \"p\" { n = y }\n";
    let module = parse(src, "shared-data").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("p".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("p"));
    assert!(profile.path_is_secret("n"));
    assert!(!profile.path_is_secret("y"), "data node must stay visible");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(json.contains("BENIGN-DATA"), "benign data hidden: {json}");
    assert!(!json.contains(raw), "source leaked: {json}");
}

#[test]
fn computed_selector_rename_from_an_unselected_profile_does_not_move_taint() {
    // F3 (review 29): a rename source that only an unselected profile
    // defines does not exist in the evaluated tree; the destination
    // keeps its materiality.
    let raw = "computed-rename-s3cr3t-r29";
    std::env::set_var("ACTIVE_R29_REN", raw);
    std::env::set_var("ACTIVE_R29_SEL", "a");
    let src = "let s = env.ACTIVE_R29_REN\nk = secret(s)\ny = s\nlet x = \"B\"\nsel = env.ACTIVE_R29_SEL\nactive_profile = sel\nprofile \"b\" { x = \"B\" }\n@rename(\"x\", \"y\")\n";
    let module = parse(src, "computed-rename").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R29_REN".to_string(), raw.to_string());
    env.insert("ACTIVE_R29_SEL".to_string(), "a".to_string());
    let mut engine = Engine::new().with_env(env);
    engine.profile_override = None;
    let (value, _) = engine.evaluate(&module).expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("y"));
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "computed rename leaked: {json}");
}

#[test]
fn computed_selector_does_not_suppress_a_material_default_fill() {
    // F4 (review 29): an unselected profile key must not count as an
    // export value, or the material default fill is skipped and a
    // benign binding is published.
    let raw = "computed-default-s3cr3t-r29";
    std::env::set_var("ACTIVE_R29_DEF", raw);
    std::env::set_var("ACTIVE_R29_SEL2", "a");
    let src = "let s = env.ACTIVE_R29_DEF\nk = secret(s)\nlet z = \"B\"\nschema = { z = String(default = s) }\nsel = env.ACTIVE_R29_SEL2\nactive_profile = sel\nprofile \"b\" { z = \"B\" }\n";
    let module = parse(src, "computed-default").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R29_DEF".to_string(), raw.to_string());
    env.insert("ACTIVE_R29_SEL2".to_string(), "a".to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("z"), "filled path is material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "default fill leaked: {json}");
}

#[test]
fn env_name_is_material_at_every_read_after_a_profile_selection() {
    // F1 (review 30): a profile places env.LIVE at a material path.
    // Every read of that name is material, including plain reads and
    // expression operands.
    let live = "env-name-leak-r30";
    std::env::set_var("ACTIVE_R30_LIVE", live);
    let src = "let e = env.ACTIVE_R30_LIVE\nlet p = env.ACTIVE_R30_SRC\nk = secret(p)\nn = p\nm = e\nmsg = \"pre-${e}\"\nactive_profile = \"p\"\nprofile \"p\" { n = e }\n";
    let module = parse(src, "env-name-everywhere").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("p".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("p"));
    assert!(profile.path_is_secret("m"), "second read is material");
    assert!(
        profile.path_is_secret("msg"),
        "expression embedding is material"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(live), "env name leaked: {json}");
}

#[test]
fn nested_env_in_a_material_profile_expression_is_material() {
    // F2 (review 30): an env read nested under an interpolation or an
    // env fallback inside a material profile value is material even
    // though no profile path points at it directly.
    let live = "nested-env-r30";
    std::env::set_var("ACTIVE_R30_NEST", live);
    std::env::remove_var("ACTIVE_R30_UNSET");
    let src = "let e = env.ACTIVE_R30_NEST\nlet p = env.ACTIVE_R30_SRC\nk = secret(p)\nn = p\nw = env.ACTIVE_R30_UNSET ?: e\ntext = \"x-${e}\"\nactive_profile = \"p\"\nprofile \"p\" { n = \"pre-${e}\" }\n";
    let module = parse(src, "nested-env").expect("parse");
    let mut engine = Engine::new();
    engine.profile_override = Some("p".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("p"));
    assert!(profile.path_is_secret("w"), "fallback read is material");
    assert!(profile.path_is_secret("text"), "interpolation is material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(live), "nested env leaked: {json}");
}

#[test]
fn computed_selector_rename_from_a_runtime_profile_keeps_the_target_material() {
    // F3 (review 30): the runtime selector may select the profile that
    // defines the rename source; the destination must stay material.
    let raw = "computed-runtime-rename-r30";
    std::env::set_var("ACTIVE_R30_REN", raw);
    std::env::set_var("ACTIVE_R30_SEL", "b");
    let src = "let p = env.ACTIVE_R30_REN\nk = secret(p)\ny = p\nsel = env.ACTIVE_R30_SEL\nactive_profile = sel\nprofile \"b\" { x = p }\n@rename(\"x\", \"zz\")\n";
    let module = parse(src, "computed-runtime-rename").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R30_REN".to_string(), raw.to_string());
    env.insert("ACTIVE_R30_SEL".to_string(), "b".to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(
        profile.path_is_secret("zz"),
        "renamed target stays material"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "computed rename leaked: {json}");
}

#[test]
fn computed_nested_rename_marks_the_same_parent_destination() {
    // N1 (review 31): the evaluator renames within the same parent, so
    // `a.x` -> `zz` lands at `a.zz`; the fail-closed overlay must use
    // that destination, not a bare top-level key.
    let raw = "nested-computed-rename-r31";
    std::env::set_var("ACTIVE_R31_REN", raw);
    std::env::set_var("ACTIVE_R31_SEL", "b");
    let src = "let p = env.ACTIVE_R31_REN\nk = secret(p)\nsel = env.ACTIVE_R31_SEL\nactive_profile = sel\nprofile \"b\" { a = { x = p } }\n@rename(\"a.x\", \"zz\")\n";
    let module = parse(src, "nested-computed-rename").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R31_REN".to_string(), raw.to_string());
    env.insert("ACTIVE_R31_SEL".to_string(), "b".to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("a.zz"), "same-parent destination");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "nested computed rename leaked: {json}");
}

#[test]
fn failed_patch_set_cannot_poison_a_later_rename() {
    // F1 (review 33): a failed Set (W004) never reaches the tree. Its
    // phantom path must not make a rename source exist and clear a
    // real material destination.
    let raw = "phantom-set-r33";
    std::env::set_var("ACTIVE_R33_PHANTOM", raw);
    let src = "let p = env.ACTIVE_R33_PHANTOM\nk = secret(p)\na = { q = p }\n@patch(\"a.y[0]\", p)\n@rename(\"a.y\", \"q\")\n";
    let module = parse(src, "phantom-set").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R33_PHANTOM".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("a.q"), "destination stays material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "phantom patch leaked: {json}");
}

#[test]
fn computed_selector_applies_late_env_name_materiality_to_profile_paths() {
    // F2 (review 33): the selected profile reads an env that another
    // profile places at a material path. Every read is material,
    // including profile-provided-only paths processed earlier.
    let raw = "late-env-r33";
    std::env::set_var("ACTIVE_R33_LATE", raw);
    std::env::set_var("ACTIVE_R33_SEL", "a");
    let src = "let e = env.ACTIVE_R33_LATE\nlet p = env.ACTIVE_R33_SRC\nk = secret(p)\nsel = env.ACTIVE_R33_SEL\nn = p\nactive_profile = sel\nprofile \"a\" { x = e }\nprofile \"b\" { n = e }\n";
    let module = parse(src, "late-env").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R33_LATE".to_string(), raw.to_string());
    env.insert("ACTIVE_R33_SRC".to_string(), "source-r33".to_string());
    env.insert("ACTIVE_R33_SEL".to_string(), "a".to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("x"), "late env read is material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "late env leaked: {json}");
}

#[test]
fn nested_array_default_materiality_is_per_element_and_deterministic() {
    // F3 (review 33): a default array registers one concrete path per
    // element; a material element and a benign sibling never share a
    // wildcard alias (which made the result iteration-order dependent).
    let raw = "default-array-element-r33";
    std::env::set_var("ACTIVE_R33_ARR", raw);
    let src = "let s = env.ACTIVE_R33_ARR\nk = secret(s)\nschema = { arr = Array<Table { a = Any(default = [s, \"benign-element\"]) }> }\narr = [{}]\n";
    let module = parse(src, "array-default-per-element").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R33_ARR".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("arr[0].a[0]"), "material element");
    assert!(
        !profile.path_is_secret("arr[0].a[1]"),
        "benign sibling stays visible"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "material element leaked: {json}");
    assert!(json.contains("benign-element"), "sibling hidden: {json}");
}

#[test]
fn successful_patch_set_path_is_a_rename_candidate() {
    // N1 (review 34): a Set that applies creates a real path; a later
    // same-parent rename moves its taint to the destination.
    let raw = "set-rename-r34";
    std::env::set_var("ACTIVE_R34_SET", raw);
    let src = "let p = env.ACTIVE_R34_SET\nk = secret(p)\na = {}\n@patch(\"a.moved\", p)\n@rename(\"a.moved\", \"w\")\n";
    let module = parse(src, "set-rename").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R34_SET".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(
        profile.path_is_secret("a.w"),
        "renamed Set path is material"
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "Set rename leaked: {json}");
}

#[test]
fn computed_late_materiality_follows_a_rename() {
    // N2 (review 34): the computed late path record is rewritten by a
    // later rename, so the destination is material.
    let raw = "late-rename-r34";
    std::env::set_var("ACTIVE_R34_LATE", raw);
    std::env::set_var("ACTIVE_R34_SEL", "a");
    let src = "let e = env.ACTIVE_R34_LATE\nlet p = env.ACTIVE_R34_SRC\nk = secret(p)\nsel = env.ACTIVE_R34_SEL\nn = p\nactive_profile = sel\nprofile \"a\" { q = e }\nprofile \"b\" { n = e }\n@rename(\"q\", \"w\")\n";
    let module = parse(src, "late-rename").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R34_LATE".to_string(), raw.to_string());
    env.insert("ACTIVE_R34_SRC".to_string(), "source-r34".to_string());
    env.insert("ACTIVE_R34_SEL".to_string(), "a".to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("w"), "renamed late path is material");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "late rename leaked: {json}");
}

#[test]
fn defaults_expand_over_a_patch_introduced_container() {
    // N3 (review 34): a Set may add elements; the material default
    // fills inside them and must be marked per element.
    let raw = "patch-array-default-r34";
    std::env::set_var("ACTIVE_R34_ARR", raw);
    let src = "let s = env.ACTIVE_R34_ARR\nk = secret(s)\nschema = { arr = Array<Table { a = Any(default = [s, \"benign-element\"]) }> }\narr = [{}]\n@patch(\"arr\", [{}, {}])\n";
    let module = parse(src, "patch-array-default").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R34_ARR".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("arr[1].a[0]"), "new element fill");
    assert!(!profile.path_is_secret("arr[1].a[1]"), "benign sibling");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "patch container leaked: {json}");
    assert!(json.contains("benign-element"), "sibling hidden: {json}");
}

#[test]
fn successful_set_into_an_env_fallback_table_is_a_rename_candidate() {
    // F1 (review 35): `env.M ?: {…}` evaluates to a table, so a Set
    // into it succeeds and its path is a rename candidate.
    let raw = "fallback-set-r35";
    std::env::set_var("ACTIVE_R35_P", raw);
    std::env::remove_var("ACTIVE_R35_M");
    let src = "let p = env.ACTIVE_R35_P\nk = secret(p)\nx = env.ACTIVE_R35_M ?: { a = 1 }\n@patch(\"x.b\", p)\n@rename(\"x.b\", \"c\")\n";
    let module = parse(src, "fallback-set").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R35_P".to_string(), raw.to_string());
    let (value, _) = Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("resilient evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    assert!(profile.path_is_secret("x.c"), "fallback Set rename path");
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "fallback Set leaked: {json}");
}

#[test]
fn profile_analysis_work_is_bounded_and_fails_closed() {
    // F1 (review 36): many profile fields sharing a large table must
    // not run an unbounded merge; the shared work bound trips and the
    // document fails closed (unknown paths are material).
    let mut big = String::from("let big = {");
    for i in 0..200 {
        if i > 0 {
            big.push(',');
        }
        big.push_str(&format!("k{i} = 1"));
    }
    big.push_str("}\n");
    let mut src = String::from("let p = env.R36_SRC\nk = secret(p)\n");
    src.push_str(&big);
    src.push_str("z = \"benign\"\nactive_profile = \"p\"\nprofile \"p\" {\n");
    for i in 0..600 {
        src.push_str(&format!("  f{i} = big\n"));
    }
    src.push_str("}\n");
    std::env::set_var("R36_SRC", "bounded-source-r36");
    let module = parse(&src, "profile-work-bound").expect("parse");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("p"));
    assert!(
        profile.path_is_secret("z"),
        "work-bound trip must fail closed"
    );
}

#[test]
fn work_bound_trip_marks_node_and_env_surfaces_material() {
    // N1 (review 37): on a profile work-bound trip the document fails
    // closed for node/name-based consumers too, not only path exports.
    let mut src = String::from("let s = env.R37_SRC\nk = secret(s)\n");
    src.push_str("let big = {");
    for i in 0..200 {
        if i > 0 {
            src.push(',');
        }
        src.push_str(&format!("k{i} = 1"));
    }
    src.push_str("}\nz = \"benign\"\nactive_profile = \"p\"\nprofile \"p\" {\n");
    for i in 0..3000 {
        src.push_str(&format!("  f{i} = big\n"));
    }
    src.push_str("  z = env.R37_TRACE\n}\n");
    std::env::set_var("R37_SRC", "bound-source-r37");
    let module = parse(&src, "work-bound-node").expect("parse");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("p"));
    assert!(profile.path_is_secret("z"), "path fails closed");
    assert!(
        profile.env_is_material("R37_TRACE"),
        "env name fails closed for provenance"
    );
}

#[test]
fn redaction_work_is_bounded_for_a_huge_secret_set() {
    // Review 38: post-evaluation redaction must stay bounded even with
    // a pathological secret set; exhaustion redacts wholesale.
    let mut redactor = active_core::secret::SecretRedactor::new();
    for i in 0..10_000 {
        redactor.insert(format!("secret-literal-{i}"));
    }
    let text = "a".repeat(200_000);
    assert!(redactor.redact_text(&text).is_none(), "no match");
}

#[test]
fn redaction_charges_comparator_work() {
    // Review 39: a long non-matching prefix must not rescan per cursor
    // beyond the pass budget; exhaustion redacts wholesale.
    let mut redactor = active_core::secret::SecretRedactor::new();
    let mut secret = "a".repeat(500_000);
    secret.push('b');
    redactor.insert(secret);
    let text = "a".repeat(1_000_000);
    assert!(
        redactor.redact_text(&text).is_none(),
        "linear scan is not a match"
    );
}

#[test]
fn redaction_passes_are_call_order_independent() {
    // Review 39: two exports with one profile must be byte-identical
    // even when a pass exhausts the comparison budget.
    let big = "a".repeat(300_000);
    let src = format!(
        "big = \"{big}\"\ns0 = secret(\"secret-0\")\ns1 = secret(\"secret-1\")\ns2 = secret(\"secret-2\")\ns3 = secret(\"secret-3\")\ns4 = secret(\"secret-4\")\ns5 = secret(\"secret-5\")\ns6 = secret(\"secret-6\")\ns7 = secret(\"secret-7\")\ns8 = secret(\"secret-8\")\ns9 = secret(\"secret-9\")\n"
    );
    let module = parse(&src, "redaction-pass").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    let first = active_core::convert::value_to_pretty_json_with(&profile, &value);
    let second = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert_eq!(first, second, "exports must not depend on call order");
    assert!(!first.contains("secret-0"), "{first}");
}

#[test]
fn redaction_budget_does_not_overcharge_short_candidates() {
    // Review 40: a candidate longer than the remainder fails on its
    // length; charging its full length destroyed benign exports.
    let mut redactor = active_core::secret::SecretRedactor::new();
    redactor.insert("a".repeat(40));
    redactor.insert("b".repeat(40));
    let benign = "c".repeat(32_000);
    assert!(
        redactor.redact_text(&benign).is_none(),
        "benign text must stay unmarked"
    );
}

#[test]
fn redaction_never_scans_a_candidate_longer_than_the_remainder() {
    // Review 41: a 2001-byte candidate against a 2000-byte benign
    // remainder must fail on length without scanning or tripping the
    // budget.
    let mut redactor = active_core::secret::SecretRedactor::new();
    let mut secret = "a".repeat(2000);
    secret.push('b');
    redactor.insert(secret);
    let benign = "a".repeat(2000);
    assert!(
        redactor.redact_text(&benign).is_none(),
        "length mismatch must not mark benign text"
    );
}

#[test]
fn redaction_keeps_document_scale_benign_exports_intact() {
    // Review 41: a normal multi-thousand-field export with a couple of
    // secrets must not exhaust the pass budget and destroy benign
    // values.
    let mut redactor = active_core::secret::SecretRedactor::new();
    for ch in [
        'z', 'y', 'x', 'w', 'v', 'u', 't', 's', 'r', 'p', 'o', 'n', 'm', 'l', 'k', 'j', 'i', 'h',
        'g', 'f', 'e', 'd', 'c', 'b', 'Z', 'Y', 'X', 'W', 'V', 'U', 'T', 'S',
    ] {
        redactor.insert(ch.to_string().repeat(40));
    }
    let benign = "q".repeat(200);
    for _ in 0..10_000 {
        assert!(
            redactor.redact_text(&benign).is_none(),
            "benign value marked at document scale"
        );
    }
}

#[test]
fn profile_bound_does_not_false_redact_a_benign_document() {
    // Review 42: 100 scalar overrides over 10k benign keys is normal
    // work, not a bound trip.
    let mut src = String::from("active_profile = \"p\"\n");
    for i in 0..10_000 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    src.push_str("profile \"p\" {\n");
    for i in 0..100 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    src.push_str("}\n");
    let module = parse(&src, "profile-bound-benign").expect("parse");
    let profile = active_core::secret::SecretProfile::analyze_with_profile(&module, Some("p"));
    assert!(
        !profile.path_is_secret("k9999"),
        "benign document must not fail closed"
    );
}

#[test]
fn redaction_never_uses_the_longest_secret_as_a_length_gate() {
    // Review 44 (CRITICAL): a long secret must not disable redaction of
    // every shorter secret in a shorter text.
    let long = "L".repeat(200);
    let src = format!(
        "long = secret(\"{long}\")\nshort = secret(\"hunter2-r44\")\ncopy = \"hunter2-r44\"\nplain = \"ok\"\n"
    );
    let module = parse(&src, "mixed-length").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains("hunter2-r44"), "{json}");
    assert!(json.contains("ok"), "{json}");
}

#[test]
fn redaction_interleaved_insert_and_redact_is_amortized() {
    // Review 44: inserting one secret then redacting used to rebuild
    // the automaton every call (quadratic). 2 000 alternations must
    // finish promptly and match the final set exactly.
    let mut redactor = active_core::secret::SecretRedactor::new();
    let started = std::time::Instant::now();
    for index in 0..2_000 {
        redactor.insert(format!("token-{index:04}"));
        let text = format!("value token-{index:04} tail");
        let redacted = redactor.redact_text(&text).expect("current secret matches");
        assert!(
            !redacted.contains(&format!("token-{index:04}")),
            "{redacted}"
        );
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "interleaved insert/redact took {:?}",
        started.elapsed()
    );
    assert!(redactor.redact_text("token-0000").is_some());
}

#[test]
fn redaction_set_equality_ignores_insertion_order() {
    // Review 44: equality is a set comparison, not insertion order.
    let mut first = active_core::secret::SecretRedactor::new();
    first.insert("alpha-secret");
    first.insert("beta-secret");
    let mut second = active_core::secret::SecretRedactor::new();
    second.insert("beta-secret");
    second.insert("alpha-secret");
    assert_eq!(first, second);
    second.insert("gamma-secret");
    assert_ne!(first, second);
}

#[test]
fn redaction_keeps_the_prefix_when_the_first_match_follows_a_marker() {
    // Review 44: a lazily allocated output buffer must still carry the
    // segments (and markers) before the first match.
    let mut redactor = active_core::secret::SecretRedactor::new();
    redactor.insert("hunter2-after-marker");
    let marker = active_core::secret::MARKER;
    let text = format!("head {marker} middle {marker} tail hunter2-after-marker end");
    let redacted = redactor.redact_text(&text).expect("late match");
    assert_eq!(
        redacted,
        format!("head {marker} middle {marker} tail {marker} end")
    );
    let both = format!("hunter2-after-marker {marker} hunter2-after-marker");
    assert_eq!(
        redactor.redact_text(&both),
        Some(format!("{marker} {marker} {marker}"))
    );
}

#[test]
fn redaction_does_not_rescan_an_absent_level_per_match() {
    // Review 47: a batch with no match must be retired for the rest of
    // the segment, not rescanned for every match another batch finds.
    let mut redactor = active_core::secret::SecretRedactor::new();
    redactor.insert("a".to_string());
    redactor.insert("b".to_string());
    redactor.insert("c".to_string());
    let text = "c".repeat(40_000);
    let started = std::time::Instant::now();
    let redacted = redactor.redact_text(&text).expect("c matches");
    assert_eq!(
        redacted.matches(active_core::secret::MARKER).count(),
        40_000
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "absent batches rescanned: {:?}",
        started.elapsed()
    );
}

#[test]
fn redaction_reuses_a_distant_batch_match() {
    // Review 48: a batch whose match sits far right must be cached, not
    // re-scanned from every earlier match of another batch.
    let mut redactor = active_core::secret::SecretRedactor::new();
    // Insertion order shapes the batch layout (b,c in one, a alone).
    redactor.insert("b".to_string());
    redactor.insert("c".to_string());
    redactor.insert("a".to_string());
    let mut text = "a".repeat(40_000);
    text.push('b');
    let started = std::time::Instant::now();
    let redacted = redactor.redact_text(&text).expect("matches");
    assert_eq!(
        redacted.matches(active_core::secret::MARKER).count(),
        40_001
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "distant batch match rescanned: {:?}",
        started.elapsed()
    );
}

#[test]
fn profile_only_material_env_seeds_without_a_secret_node() {
    // Review 48: a fail-closed profile border (the path bound trips)
    // marks env reads material without any `secret(...)` node; seeds
    // must still be collected and cover an out-of-tree copy.
    let raw = "r48-profile-env";
    let mut src = String::from("arr = [");
    for index in 0..100_050 {
        if index > 0 {
            src.push(',');
        }
        src.push('0');
    }
    src.push_str("]\nactive_profile = \"p\"\nprofile \"p\" { token = env.ACTIVE_R48_TOKEN }\nlet copy = env.ACTIVE_R48_TOKEN\nout = copy\n");
    let module = parse(&src, "profile-only-env").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R48_TOKEN".to_string(), raw.to_string());
    let mut engine = Engine::hermetic().with_env(env.clone());
    engine.budget = active_core::Budget::trusted();
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    assert!(!seeds.is_empty(), "fail-closed env read must seed");
    let inspected = active_core::convert::inspect_path_with_profile_env(
        &module,
        &value,
        "out",
        None,
        Some(&env),
        Some(&seeds),
    )
    .expect("inspect");
    assert!(!inspected.to_string().contains(raw), "{inspected}");
    assert_eq!(inspected["tainted"], true, "{inspected}");
}

#[test]
fn redaction_scans_absent_long_patterns_once_per_segment() {
    // Review 45: an absent long pattern must be searched once, not
    // once per short-pattern match.
    let mut redactor = active_core::secret::SecretRedactor::new();
    redactor.insert("x".to_string());
    for index in 0..3 {
        redactor.insert(format!("{}{index}", "z".repeat(5_000)));
    }
    let text = "x".repeat(180_000);
    let started = std::time::Instant::now();
    let redacted = redactor.redact_text(&text).expect("short secret matches");
    assert_eq!(
        redacted.matches(active_core::secret::MARKER).count(),
        180_000,
        "every short occurrence redacts"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "absent long patterns rescanned: {:?}",
        started.elapsed()
    );
}

#[test]
fn redaction_interleaved_insert_and_redact_scales() {
    // Review 45: folding follows the base set, so 8 000 alternations
    // stay far below the old quadratic bound.
    let mut redactor = active_core::secret::SecretRedactor::new();
    let started = std::time::Instant::now();
    for index in 0..8_000 {
        redactor.insert(format!("token-{index:05}"));
        let text = format!("value token-{index:05} tail");
        assert!(redactor.redact_text(&text).is_some());
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "interleaved insert/redact took {:?}",
        started.elapsed()
    );
}

#[test]
fn redaction_handles_a_pattern_longer_than_the_automaton_limit() {
    // Review 44: patterns over MAX_AC_PATTERN stay out of the
    // automaton and are scanned linearly, still longest-first.
    let mut redactor = active_core::secret::SecretRedactor::new();
    let long = "q".repeat(5_000);
    redactor.insert(long.clone());
    redactor.insert("short-secret".to_string());
    let text = format!("pre {long} mid short-secret post");
    let redacted = redactor.redact_text(&text).expect("both match");
    assert!(!redacted.contains("short-secret"), "{redacted}");
    assert!(!redacted.contains(&long), "long secret leaked");
    assert_eq!(redacted.matches(active_core::secret::MARKER).count(), 2);
}

#[test]
fn runtime_env_secret_is_swept_from_payloads_and_copies() {
    // Review 45: a runtime (env-backed) secret has no authored literal
    // in the profile; its text must still be found in a payload, a
    // media type, and a plain string copy.
    let raw = "sd-token-env-r45";
    let src = format!(
        "m = env.TOKEN\nk = secret(m)\nf = blob(\"text/plain\", \"pre {raw} post\")\ncopy = \"pre {raw} post\"\n"
    );
    let module = parse(&src, "runtime-secret-payload").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("TOKEN".to_string(), raw.to_string());
    let mut engine = Engine::hermetic().with_env(env);
    engine.materialise_blobs = true;
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "{json}");
    assert!(!json.contains("payload_b64"), "{json}");
    let toml = active_core::convert::value_to_toml_string_with(&profile, &value)
        .expect("toml")
        .0;
    assert!(!toml.contains(raw), "{toml}");
    let inspected = active_core::convert::inspect_path(&module, &value, "f").expect("inspect blob");
    assert!(!inspected.to_string().contains(raw), "{inspected}");
    assert!(
        !inspected.to_string().contains("payload_b64"),
        "{inspected}"
    );
    assert_eq!(inspected["tainted"], true, "{inspected}");
}

#[test]
fn let_bound_runtime_secret_is_swept_from_payloads_and_inspect() {
    // Review 46: a `let`-bound secret never enters the exported tree,
    // so the environment's material values must seed the redactor.
    let raw = "sd-token-let-r46";
    let src = "let s = env.ACTIVE_R46_A\nlet k = secret(s)\nlet b = env.ACTIVE_R46_B\nf = blob(\"text/plain\", b)\ncopy = b\n";
    let module = parse(src, "runtime-secret-let").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R46_A".to_string(), raw.to_string());
    env.insert("ACTIVE_R46_B".to_string(), raw.to_string());
    let mut engine = Engine::hermetic().with_env(env.clone());
    engine.materialise_blobs = true;
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    let mut profile = active_core::secret::SecretProfile::analyze(&module);
    profile.absorb_environment(&env);
    profile.absorb_secrets(&seeds);
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "{json}");
    assert!(!json.contains("payload_b64"), "{json}");
    let inspected = active_core::convert::inspect_path_with_profile_env(
        &module,
        &value,
        "f",
        None,
        Some(&env),
        Some(&seeds),
    )
    .expect("inspect");
    assert!(!inspected.to_string().contains(raw), "{inspected}");
    assert!(
        !inspected.to_string().contains("payload_b64"),
        "{inspected}"
    );
    assert_eq!(inspected["tainted"], true, "{inspected}");
}

#[test]
fn computed_let_secret_is_swept_from_exports_and_inspect() {
    // Review 47: a material interpolation bound to a `let` name never
    // enters the exported tree; the evaluator seeds must cover it.
    let src = "let v = \"PIN-${1234}\"\nlet k = secret(v)\ncopy = \"PIN-1234\"\n";
    let module = parse(src, "computed-let").expect("parse");
    let (value, _, seeds) = Engine::hermetic()
        .evaluate_with_seeds(&module)
        .expect("evaluate");
    let mut profile = active_core::secret::SecretProfile::analyze(&module);
    profile.absorb_secrets(&seeds);
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains("PIN-1234"), "{json}");
    let inspected = active_core::convert::inspect_path_with_profile_env(
        &module,
        &value,
        "copy",
        None,
        None,
        Some(&seeds),
    )
    .expect("inspect");
    assert!(!inspected.to_string().contains("PIN-1234"), "{inspected}");
    assert_eq!(inspected["tainted"], true, "{inspected}");
}

#[test]
fn blob_identity_and_payload_sweep_authored_secret_literals() {
    // Review 42: a blob's media type and payload can carry a known
    // secret literal; both are swept/fail-closed.
    let raw = "hunter2-blob-r42";
    let src = format!(
        "m = \"{raw}\"\nk = secret(m)\nf = blob(m, \"data\")\ng = blob(\"text/plain\", \"pre {raw} post\")\n"
    );
    let module = parse(&src, "blob-literal").expect("parse");
    let mut engine = Engine::new();
    engine.materialise_blobs = true;
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    let redacted = active_core::convert::redact_value(&profile, &value);
    let json = active_core::convert::value_to_pretty_json(&redacted);
    assert!(!json.contains(raw), "blob literal leaked: {json}");
}

#[test]
fn taint_probe_materialises_without_charging_alias_copies() {
    // Review 50: read-only taint passes need payload bytes but must not
    // fail a document that exported within its budget.
    let src = "d = \"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\"\nb = blob(\"text/plain\", d)\nr1 = b\nr2 = b\nr3 = b\nr4 = b\nr5 = b\n";
    let module = parse(src, "taint-probe").expect("parse");
    let evaluate_with = |limit: u64, materialise: bool, probe: bool| {
        let mut engine = Engine::hermetic();
        engine.budget.max_total_value_size = limit;
        engine.materialise_blobs = materialise;
        engine.taint_probe = probe;
        engine.evaluate(&module).is_ok()
    };
    // Find the exact minimal budget at which the plain export fits.
    let mut high = 1u64;
    while !evaluate_with(high, false, false) {
        high = high.saturating_mul(2);
    }
    let mut low = 1u64;
    while low < high {
        let middle = (low + high) / 2;
        if evaluate_with(middle, false, false) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    let limit = low;
    assert!(
        !evaluate_with(limit, true, false),
        "plain materialisation must charge alias copies at the exact baseline"
    );
    assert!(
        evaluate_with(limit, true, true),
        "taint probe must fit the document's exact baseline budget"
    );
}
