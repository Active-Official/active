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

#[test]
fn profile_literal_at_a_material_path_seeds_payload_and_copies() {
    // Review 56: a literal supplied by a profile at a material path is
    // authored secret material; its copies and payloads redact.
    let raw = "hunter2-profile-r56";
    let src = format!(
        "t = secret(\"hunter2-target\")\nactive_profile = \"p\"\nprofile \"p\" {{ t = \"{raw}\" }}\nb = blob(\"text/plain\", \"prefix {raw} suffix\")\nu = \"prefix {raw} suffix\"\n"
    );
    let module = parse(&src, "profile-literal-seed").expect("parse");
    let mut engine = Engine::hermetic();
    engine.materialise_blobs = true;
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    let mut profile = active_core::secret::SecretProfile::analyze(&module);
    profile.absorb_environment(&engine.env);
    profile.absorb_secrets(&seeds);
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "{json}");
    assert!(!json.contains("payload_b64"), "{json}");
    let inspected = active_core::convert::inspect_path_with_profile_env(
        &module,
        &value,
        "b",
        None,
        None,
        Some(&seeds),
    )
    .expect("inspect");
    assert!(!inspected.to_string().contains(raw), "{inspected}");
    assert_eq!(inspected["tainted"], true, "{inspected}");
}

#[test]
fn profile_selected_let_literal_seeds_payload_and_copies() {
    // Review 57: a profile that selects a `let` literal at a material
    // path is authoring material there; copies and payloads redact.
    let raw = "hunter2-let-selected";
    let src = format!(
        "let region = \"{raw}\"\nt = secret(\"hunter2-target\")\nactive_profile = \"p\"\nprofile \"p\" {{ t = region }}\nb = blob(\"text/plain\", \"{raw}\")\ndup = \"{raw}\"\n"
    );
    let module = parse(&src, "profile-let-seed").expect("parse");
    let mut engine = Engine::hermetic();
    engine.materialise_blobs = true;
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    let mut profile = active_core::secret::SecretProfile::analyze(&module);
    profile.absorb_environment(&engine.env);
    profile.absorb_secrets(&seeds);
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "{json}");
    assert!(!json.contains("payload_b64"), "{json}");
    let inspected = active_core::convert::inspect_path_with_profile_env(
        &module,
        &value,
        "b",
        None,
        None,
        Some(&seeds),
    )
    .expect("inspect");
    assert!(!inspected.to_string().contains(raw), "{inspected}");
    assert_eq!(inspected["tainted"], true, "{inspected}");
}

#[test]
fn incremental_seeds_cover_memo_hits() {
    // Review 58: a memo-served node must seed like a fresh walk.
    let src = "let region = \"hunter2-region\"\nt = secret(\"hunter2-target\")\nactive_profile = \"p\"\nprofile \"p\" { t = \"${region}-suffix\" }\ndup = \"hunter2-region-suffix\"\n";
    let module = parse(src, "memo-seeds").expect("parse");
    let engine = Engine::hermetic();
    let mut memo = active_core::IncrementalMemo::default();
    let (_, _, first) = engine
        .evaluate_incremental_with_seeds(&module, &mut memo)
        .expect("first");
    assert!(first.contains_exact("hunter2-region-suffix"));
    let (_, _, second) = engine
        .evaluate_incremental_with_seeds(&module, &mut memo)
        .expect("second");
    assert!(second.contains_exact("hunter2-region-suffix"));
}

#[test]
fn profile_override_of_the_benign_slot_still_seeds() {
    // Review 58: the pre-transform "visible in its own slot" node can be
    // overridden; its text is then material everywhere.
    let raw = "hunter2-let";
    let src = format!(
        "let v = \"{raw}\"\nt = secret(\"hunter2-target\")\nactive_profile = \"p\"\nprofile \"p\" {{ t = v, other = 1 }}\nother = v\ndup = \"{raw}\"\n"
    );
    let module = parse(&src, "overridden-slot").expect("parse");
    let (value, _, seeds) = Engine::hermetic()
        .evaluate_with_seeds(&module)
        .expect("evaluate");
    let mut profile = active_core::secret::SecretProfile::analyze(&module);
    profile.absorb_environment(&std::collections::HashMap::new());
    profile.absorb_secrets(&seeds);
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains(raw), "{json}");
}

#[test]
fn nested_profile_tables_and_long_alias_chains_seed() {
    // Review 59 (HIGH): a profile value nested in a table, reached
    // through a long `let` alias chain, was never seeded because
    // visibility looked at the pre-transform tree. Both field orders
    // must behave the same, and a copy of the text must redact.
    let chain: String = (0..=20)
        .map(|i| {
            if i == 0 {
                "let a0 = \"hunter2-alias\"\n".to_string()
            } else {
                format!("let a{i} = a{}\n", i - 1)
            }
        })
        .collect();
    for profile in [
        "profile \"p\" { t = { s = a20 }, other = 1 }",
        "profile \"p\" { other = 1, t = { s = a20 } }",
    ] {
        let src = format!(
            "{chain}t = {{ s = secret(\"hunter2-target\") }}\nother = a20\nactive_profile = \"p\"\n{profile}\ndup = \"hunter2-alias\"\n"
        );
        let module = parse(&src, "nested-profile").expect("parse");
        let (value, _, seeds) = Engine::hermetic()
            .evaluate_with_seeds(&module)
            .expect("evaluate");
        let mut secret = active_core::secret::SecretProfile::analyze(&module);
        secret.absorb_secrets(&seeds);
        let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
        assert!(!json.contains("hunter2-alias"), "{profile}: {json}");
        assert!(!json.contains("hunter2-target"), "{profile}: {json}");
    }
}

#[test]
fn unselected_profiles_do_not_seed_literals() {
    // Review 59 (MEDIUM): only the selected profile may contribute
    // material values. `q` never runs, so `other` stays readable.
    let src = "t = secret(\"hunter2-x\")\nother = \"hunter2-other\"\nprofile \"q\" { t = \"hunter2-other\" }\n";
    let module = parse(src, "unselected-profile").expect("parse");
    let (value, _) = Engine::hermetic().evaluate(&module).expect("evaluate");
    let secret = active_core::secret::SecretProfile::analyze(&module);
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(json.contains("hunter2-other"), "{json}");
    assert!(!json.contains("hunter2-x"), "{json}");
}

#[test]
fn profile_value_exported_benignly_stays_visible() {
    // A value placed at a material path by a profile is redacted there,
    // and stays readable at a benign path that exports the same node.
    // A later patch over a material path answers for its own value.
    let src = "let v = \"hunter2-shared\"\nt = secret(\"hunter2-x\")\nother = v\nactive_profile = \"p\"\nprofile \"p\" { t = v }\n";
    let module = parse(src, "benign-shared").expect("parse");
    let (value, _) = Engine::hermetic().evaluate(&module).expect("evaluate");
    let secret = active_core::secret::SecretProfile::analyze(&module);
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(json.contains("hunter2-shared"), "{json}");

    let patched = "other = secret(\"hunter2-x\")\n@patch(\"other\", \"BENIGN-PATCH\")\n";
    let module = parse(patched, "benign-patch").expect("parse");
    let (value, _) = Engine::hermetic().evaluate(&module).expect("evaluate");
    let secret = active_core::secret::SecretProfile::analyze(&module);
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(json.contains("BENIGN-PATCH"), "{json}");
    assert!(!json.contains("hunter2-x"), "{json}");
}

#[test]
fn computed_selector_seeds_inherited_material_profile_values() {
    // Review 60: a computed selector cannot prove which profile runs, so
    // its paths never enter the evaluated-tree map. The recorded node is
    // material under every selection and must still seed a copy.
    let src = "active_profile = env.P ?: \"p\"\na = secret(\"hunter2-base\")\nprofile \"p\" { a = { x = \"hunter2-plain\" } }\ndup = \"hunter2-plain\"\n";
    let module = parse(src, "computed-seed").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("P".to_string(), "p".to_string());
    let engine = Engine::hermetic().with_env(env);
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    let mut secret = active_core::secret::SecretProfile::analyze(&module);
    secret.absorb_secrets(&seeds);
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(!json.contains("hunter2-plain"), "{json}");
    assert!(!json.contains("hunter2-base"), "{json}");
}

#[test]
fn computed_profile_seed_survives_a_sibling_transform() {
    // Review 61 (HIGH): a patch on one computed-profile element erased
    // the single recorded node path for a sibling. The maintained path
    // records keep the surviving element material.
    let src = "let x = \"hunter2-shared\"\nactive_profile = env.P ?: \"p\"\na = [secret(\"hunter2-b1\")]\nprofile \"p\" { a = [x, x] }\ndup = \"hunter2-shared\"\n@patch(\"a[0]\", \"benign\")\n";
    let module = parse(src, "computed-sibling").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("P".to_string(), "p".to_string());
    let engine = Engine::hermetic().with_env(env);
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    let mut secret = active_core::secret::SecretProfile::analyze(&module);
    secret.absorb_secrets(&seeds);
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(!json.contains("hunter2-shared"), "{json}");
}

#[test]
fn substring_probe_cache_sees_values_inserted_later() {
    // Review 61: the filtered automaton must be invalidated on insert.
    let mut redactor = active_core::secret::SecretRedactor::new();
    redactor.insert("ab");
    assert!(!redactor.contains_substring("late hunter2-late", 4));
    redactor.insert("hunter2-late");
    assert!(redactor.contains_substring("late hunter2-late", 4));
}

#[test]
fn computed_selector_override_does_not_hide_a_material_seed() {
    // Review 62 (HIGH): under a computed selector a possible profile
    // may replace the base path that made a node look benign. Such a
    // path is not a stable export, so the material value still seeds.
    let src = "active_profile = env.P ?: \"none\"\na = { k = secret(\"hunter2-base\") }\nlet t = { k = \"hunter2-copy\" }\np = t\nprofile \"pr\" { a = t, p = { k = \"replaced\" } }\ndup = \"hunter2-copy\"\n";
    let module = parse(src, "computed-visibility").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("P".to_string(), "pr".to_string());
    let engine = Engine::hermetic().with_env(env);
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    let mut secret = active_core::secret::SecretProfile::analyze(&module);
    secret.absorb_secrets(&seeds);
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(!json.contains("hunter2-copy"), "{json}");
}

#[test]
fn static_resolution_is_deterministic_at_the_depth_bound() {
    // Review 62: a deep chain reachable both directly and through a
    // shallow alias must resolve the same way on every analysis.
    let mut src = String::from(
        "a = secret(\"hunter2-a\")\nb = secret(\"hunter2-b\")\nlet c0 = \"hunter2-seed\"\n",
    );
    for i in 1..=1100 {
        src.push_str(&format!("let c{i} = \"${{c{}}}\"\n", i - 1));
    }
    src.push_str(
        "active_profile = \"p\"\nprofile \"p\" { a = \"${c1100}\", b = \"${c600}\" }\ncopy = \"VISIBLE-TEXT\"\n",
    );
    let module = parse(&src, "static-depth").expect("parse");
    let first = active_core::secret::SecretProfile::analyze(&module);
    for _ in 0..15 {
        let again = active_core::secret::SecretProfile::analyze(&module);
        assert_eq!(
            again.literals(),
            first.literals(),
            "seed set changed between runs"
        );
    }
}

#[test]
fn marker_containing_secret_is_redacted() {
    // Review 62: an ordinary pass treats the marker as opaque; a known
    // value that itself contains it must still match.
    let mut redactor = active_core::secret::SecretRedactor::new();
    redactor.insert("***SECRET***hunter2-tail");
    let text = "prefix ***SECRET***hunter2-tail suffix";
    assert_eq!(
        redactor.redact_text(text).as_deref(),
        Some("prefix ***SECRET*** suffix")
    );
    assert!(redactor.contains_substring(text, 4));
}

#[test]
fn computed_rename_clears_a_stale_destination_record() {
    // Review 62: a destination entry from another possible profile must
    // not seed its text after a rename moves a different node there.
    let src = "a = { x = secret(\"hunter2-base\") }\nactive_profile = env.P ?: \"p\"\nlet m = \"MATERIAL-TEXT\"\nlet q = \"STALE-TEXT\"\nprofile \"p\" { a = { x = m } }\nprofile \"q\" { a = { z = q } }\n@rename(\"a.x\", \"z\")\ndup = \"STALE-TEXT\"\n";
    let module = parse(src, "computed-rename").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("P".to_string(), "p".to_string());
    let engine = Engine::hermetic().with_env(env);
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    let mut secret = active_core::secret::SecretProfile::analyze(&module);
    secret.absorb_secrets(&seeds);
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(json.contains("STALE-TEXT"), "{json}");
    assert!(!json.contains("MATERIAL-TEXT"), "{json}");
}

#[test]
fn no_op_rename_keeps_a_computed_destination_seed() {
    // Review 63 (HIGH): a rename whose source a possible profile never
    // defines is a W004 no-op, so the destination value survives. Its
    // computed record must not be erased.
    let src = "active_profile = env.P ?: \"q\"\na = secret(\"hunter2-base\")\nprofile \"q\" { a = { z = \"STALE-TEXT\" } }\n@rename(\"a.x\", \"z\")\ndup = \"STALE-TEXT\"\n";
    let module = parse(src, "noop-rename").expect("parse");
    let (value, _) = Engine::hermetic().evaluate(&module).expect("evaluate");
    let secret = active_core::secret::SecretProfile::analyze(&module);
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(!json.contains("STALE-TEXT"), "{json}");
}

#[test]
fn computed_profile_benign_copy_stays_visible() {
    // Review 63: a computed profile value placed at a material path is
    // redacted there; a benign copy of the same value stays visible.
    let src = "active_profile = env.P ?: \"p\"\na = secret(\"hunter2-base\")\nlet m = \"MATERIAL-TEXT\"\nprofile \"p\" { a = m }\nb = m\n";
    let module = parse(src, "computed-benign-copy").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("P".to_string(), "p".to_string());
    let engine = Engine::hermetic().with_env(env);
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    let mut secret = active_core::secret::SecretProfile::analyze(&module);
    secret.absorb_secrets(&seeds);
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(!json.contains("hunter2-base"), "{json}");
    assert!(json.contains("MATERIAL-TEXT"), "{json}");
}

#[test]
fn no_op_rename_after_prior_transforms_keeps_the_destination_seed() {
    // Review 64 (HIGH): an earlier rename or remove makes the second
    // rename a W004 no-op, so the destination value survives. Its
    // computed record must not be cleared just because the source was
    // a base path.
    for patch in ["@rename(\"a.y\", \"y2\")", "@remove(\"a.y\")"] {
        let src = format!(
            "active_profile = env.P ?: \"p\"\na = {{ x = secret(\"hunter2-base\"), y = 1 }}\nprofile \"p\" {{ a = {{ x = {{ q = \"LIVE-TEXT\" }} }} }}\n{patch}\n@rename(\"a.y\", \"x\")\ndup = \"LIVE-TEXT\"\n"
        );
        let module = parse(&src, "noop-rename-prior").expect("parse");
        let (value, _) = Engine::hermetic().evaluate(&module).expect("evaluate");
        let secret = active_core::secret::SecretProfile::analyze(&module);
        let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
        assert!(!json.contains("LIVE-TEXT"), "{patch}: {json}");
    }
}

#[test]
fn short_path_material_value_redacts_only_whole_copies() {
    // Review 64: a short material value must redact an identical whole
    // value, while a longer string containing it stays readable.
    let src = "active_profile = \"p\"\na = secret(\"base\")\nprofile \"p\" { a = \"ABC\" }\nb = \"ABC\"\nregion = \"ABC-east-1\"\n";
    let module = parse(src, "short-material").expect("parse");
    let (value, _) = Engine::hermetic().evaluate(&module).expect("evaluate");
    let secret = active_core::secret::SecretProfile::analyze(&module);
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(!json.contains("\"b\": \"ABC\""), "{json}");
    assert!(json.contains("ABC-east-1"), "{json}");
}

#[test]
fn constraint_actual_redacts_a_short_path_material_copy() {
    // Review 64: T006's actual uses the same exact material set. `b`
    // is a distinct literal holding the same short text, so it is a
    // copy (a shared node exported benignly stays visible by design).
    let src = "active_profile = \"p\"\na = secret(\"base\")\nprofile \"p\" { a = \"XYZ\" }\n# @constraint: value != \"XYZ\"\nb = \"XYZ\"\n";
    let module = parse(src, "short-t006").expect("parse");
    let (_, diags) = Engine::hermetic()
        .evaluate(&module)
        .expect("evaluation returns diagnostics");
    let t006 = diags
        .iter()
        .find(|d| d.code == "T006")
        .expect("constraint must fail");
    assert!(!t006.message.contains("XYZ"), "{}", t006.message);
}

#[test]
fn short_material_value_redacts_blob_identity_and_payload() {
    // Review 65: the exact set applies at the blob media-type and
    // payload boundaries too.
    let src = "active_profile = \"p\"\na = secret(\"base\")\nprofile \"p\" { a = \"ABC\" }\nm = \"ABC\"\nf = blob(m, \"data\")\ng = blob(\"text/plain\", \"ABC\")\n";
    let module = parse(src, "short-blob").expect("parse");
    let mut engine = Engine::hermetic();
    engine.materialise_blobs = true;
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let secret = active_core::secret::SecretProfile::for_engine(
        &module,
        &engine,
        &active_core::secret::SecretRedactor::new(),
    );
    let json = active_core::convert::value_to_pretty_json_with(&secret, &value);
    assert!(!json.contains("\"media_type\": \"ABC\""), "{json}");
    assert!(!json.contains("payload_b64"), "{json}");
}

#[test]
fn redact_json_uses_the_exact_material_set() {
    // Review 65: public JSON redaction matches a whole short copy.
    let src = "active_profile = \"p\"\na = secret(\"base\")\nprofile \"p\" { a = \"ABC\" }\n";
    let module = parse(src, "short-json").expect("parse");
    let secret = active_core::secret::SecretProfile::analyze(&module);
    let mut json = serde_json::json!({ "copy": "ABC" });
    assert!(secret.redact_json(&mut json));
    assert_eq!(json["copy"], "***SECRET***");
}

#[test]
fn constraint_source_redacts_a_runtime_secret() {
    // Review 65: the predicate source can quote a host value.
    let prior = std::env::var("ACTIVE_R65_KEY").ok();
    std::env::set_var("ACTIVE_R65_KEY", "r65-env-secret");
    let src = "api = secret(env.ACTIVE_R65_KEY)\n# @constraint: value == \"r65-env-secret\"\nname = \"x\"\n";
    let module = parse(src, "runtime-t006").expect("parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("evaluate");
    let t006 = diags
        .iter()
        .find(|d| d.code == "T006")
        .expect("constraint must fail");
    assert!(!t006.message.contains("r65-env-secret"), "{}", t006.message);
    match prior {
        Some(value) => std::env::set_var("ACTIVE_R65_KEY", value),
        None => std::env::remove_var("ACTIVE_R65_KEY"),
    }
}

#[test]
fn t006_non_boolean_message_redacts_the_source() {
    // Review 66: both failing T006 arms share the redaction.
    let src = "token = secret(\"hunter2-x\")\n# @constraint: \"hunter2-x\"\nname = \"x\"\n";
    let module = parse(src, "t006-nonbool").expect("parse");
    let (_, diags) = Engine::hermetic().evaluate(&module).expect("evaluate");
    let t006 = diags
        .iter()
        .find(|d| d.code == "T006")
        .expect("constraint must fail");
    assert!(!t006.message.contains("hunter2-x"), "{}", t006.message);
}

#[test]
fn lint_deprecated_reason_is_redacted() {
    // Review 66: L005 quotes schema prose.
    let src = "token = secret(\"s3cr3t-dep\")\nschema = { host = String(deprecated = \"use s3cr3t-dep\") }\nhost = \"x\"\n";
    let module = parse(src, "lint-dep").expect("parse");
    let diags = active_core::lint::lint(&module);
    let l005 = diags
        .iter()
        .find(|d| d.code == "L005")
        .expect("deprecated field must lint");
    assert!(!l005.message.contains("s3cr3t-dep"), "{}", l005.message);
}

#[test]
fn schema_prose_redacts_on_inspect_and_generate() {
    // Review 66: schema prose surfaces beyond docgen.
    let src = "token = secret(\"s3cr3t-dep\")\nschema = { host = String(deprecated = \"use s3cr3t-dep\") }\nhost = \"x\"\n";
    let module = parse(src, "schema-prose").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let report = active_core::convert::inspect_path_with_profile(&module, &value, "host", None)
        .expect("inspect");
    let text = serde_json::to_string(&report).expect("json");
    assert!(!text.contains("s3cr3t-dep"), "{text}");
    let schema = active_core::convert::schema_inspect(&module).expect("schema inspect");
    assert!(
        !serde_json::to_string(&schema)
            .unwrap()
            .contains("s3cr3t-dep"),
        "{schema}"
    );
    let outputs = [
        active_core::convert::schema_to_json_schema(&module).expect("json schema"),
        active_core::convert::schema_to_openapi(&module, "Config").expect("openapi"),
    ];
    for output in outputs {
        assert!(!output.to_string().contains("s3cr3t-dep"), "{output}");
    }
    for output in [
        active_core::convert::schema_to_rust(&module, "Config"),
        active_core::convert::schema_to_typescript(&module, "Config"),
        active_core::convert::schema_to_python(&module, "Config"),
        active_core::convert::schema_to_go(&module, "Config"),
    ] {
        let output = output.expect("generate");
        assert!(!output.contains("s3cr3t-dep"), "{output}");
    }
}

#[test]
fn blob_payload_embedding_a_short_material_value_is_material() {
    // Review 66: a short value inside a longer payload still taints it.
    let src = "active_profile = \"p\"\na = secret(\"base\")\nprofile \"p\" { a = \"ABC\" }\nm = \"xxABCxx\"\nf = blob(\"text/plain\", m)\n";
    let module = parse(src, "payload-substring").expect("parse");
    let mut engine = Engine::hermetic();
    engine.materialise_blobs = true;
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::for_engine(
        &module,
        &engine,
        &active_core::secret::SecretRedactor::new(),
    );
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains("payload_b64"), "{json}");
}

#[test]
fn inspect_value_block_redacts_a_short_blob_media_type() {
    // Review 66: the inspect value block must apply the same media
    // materiality as the content_addressed block.
    let src = "active_profile = \"p\"\na = secret(\"base\")\nprofile \"p\" { a = \"ABC\" }\nm = \"ABC\"\nf = blob(m, \"data\")\n";
    let module = parse(src, "inspect-short-blob").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let report = active_core::convert::inspect_path_with_profile(&module, &value, "f", None)
        .expect("inspect");
    let text = serde_json::to_string(&report).expect("json");
    assert!(!text.contains("\"media_type\":\"ABC\""), "{text}");
}

#[test]
fn inspect_keeps_structural_fields_that_match_a_secret() {
    // Review 67: structural keys of an inspect entry stay queryable.
    let src = "s = secret(\"host\")\nschema = { host = String }\nhost = \"x\"\n";
    let module = parse(src, "inspect-structure").expect("parse");
    let report =
        active_core::convert::schema_inspect_with_profile(&module, None).expect("schema inspect");
    let entry = &report["fields"][0];
    assert_eq!(entry["path"], "host", "{entry}");
    assert_eq!(entry["name"], "host", "{entry}");
    assert_eq!(entry["kind"], "field", "{entry}");
    assert_eq!(entry["type"], "String", "{entry}");
}

#[test]
fn schema_evolution_metadata_is_redacted() {
    // Review 67: versions are authored strings and may quote a secret.
    let src =
        "s = secret(\"1.2.3\")\nschema = { version = \"1.2.3\", host = String }\nhost = \"x\"\n";
    let module = parse(src, "evolution").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let schema = active_core::convert::schema_inspect(&module).expect("schema inspect");
    assert!(!schema.to_string().contains("1.2.3"), "{schema}");
    let report = active_core::convert::inspect_path_with_profile(&module, &value, "host", None)
        .expect("inspect");
    assert!(!report.to_string().contains("1.2.3"), "{report}");

    let old = parse(src, "old").expect("parse");
    let new_src =
        "s = secret(\"1.2.3\")\nschema = { version = \"1.2.2\", host = String }\nhost = \"x\"\n";
    let new = parse(new_src, "new").expect("parse");
    let findings = active_core::convert::compare_schemas(&old, &new).expect("compare");
    for finding in findings {
        assert!(!finding.message.contains("1.2.3"), "{}", finding.message);
    }
}

#[test]
fn schema_inspect_honors_a_profile_override() {
    // Review 67: `schema inspect --profile` must not silently ignore
    // the selection.
    let src = "a = secret(\"base\")\nprofile \"p\" { a = \"s3cr3t-short\" }\nschema = { host = String(deprecated = \"migrate s3cr3t-short\") }\nhost = \"x\"\n";
    let module = parse(src, "schema-profile").expect("parse");
    let report = active_core::convert::schema_inspect_with_profile(&module, Some("p"))
        .expect("schema inspect");
    assert!(!report.to_string().contains("s3cr3t-short"), "{report}");
}

#[test]
fn blob_media_substring_is_material() {
    // Review 67: a short material value embedded in a media type taints
    // the identity.
    let src = "active_profile = \"p\"\na = secret(\"base\")\nprofile \"p\" { a = \"ABC\" }\nf = blob(\"xxABCxx\", \"hello\")\n";
    let module = parse(src, "media-substring").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    let json = active_core::convert::value_to_pretty_json_with(&profile, &value);
    assert!(!json.contains("xxABCxx"), "{json}");
    let report = active_core::convert::inspect_path_with_profile(&module, &value, "f", None)
        .expect("inspect");
    assert!(!report.to_string().contains("xxABCxx"), "{report}");
}

#[test]
fn viz_and_graph_report_a_payload_resident_short_secret() {
    // Review 67: both surfaces use the shared payload check.
    let src = "active_profile = \"p\"\na = secret(\"base\")\nprofile \"p\" { a = \"ABC\" }\nm = \"xxABCxx\"\nf = blob(\"text/plain\", m)\n";
    let module = parse(src, "payload-surfaces").expect("parse");
    let mut engine = Engine::hermetic();
    engine.materialise_blobs = true;
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let viz = active_core::viz::taint_data_with_profile(&module, &value, None);
    assert!(viz.to_string().contains("\"f\""), "{viz}");
    let paths = active_core::graph::secret_tainted_paths_with(&module, &value, None);
    assert!(paths.iter().any(|path| path == "f"), "{paths:?}");
}

#[test]
fn diagnostics_redact_schema_literals() {
    // Review 67: B004/T003 quote schema prose. (T004 is structural: a
    // resolvable requires target is named, per spec/validation.md.)
    let cases: [(&str, &str, &str); 2] = [
        (
            "B004",
            "s = secret(\"s3cr3t-media\")\nschema = { b = Blob(media_type = \"s3cr3t-media\") }\nb = blob(\"text/plain\", \"x\")\n",
            "s3cr3t-media",
        ),
        (
            "T003",
            "s = secret(\"s3cr3t-fmt\")\nschema = { f = String(format = \"s3cr3t-fmt\") }\nf = \"x\"\n",
            "s3cr3t-fmt",
        ),
    ];
    for (code, src, secret) in cases {
        let module = parse(src, "diag-redact").expect("parse");
        let diags = match Engine::new().evaluate(&module) {
            Ok((_, diags)) => diags,
            Err(EvalError::ValidationFailed { diagnostics }) => diagnostics,
            Err(error) => panic!("{code}: {error:?}"),
        };
        let found = diags
            .iter()
            .find(|d| d.code == code)
            .unwrap_or_else(|| panic!("{code} missing: {diags:?}"));
        assert!(!found.message.contains(secret), "{}", found.message);
    }
}

#[test]
fn generated_defaults_patterns_and_requires_redact_secrets() {
    // Review 67: generated artifacts carry schema prose and defaults.
    let src = "s = secret(\"s3cr3t-gen\")\nschema = { host = String(default = \"s3cr3t-gen\", pattern = \"^s3cr3t-gen$\", requires = \"s3cr3t-gen\") }\n";
    let module = parse(src, "gen-prose").expect("parse");
    let json_schema = active_core::convert::schema_to_json_schema(&module).expect("json schema");
    assert!(
        !json_schema.to_string().contains("s3cr3t-gen"),
        "{json_schema}"
    );
    for output in [
        active_core::convert::schema_to_typescript(&module, "Config"),
        active_core::convert::schema_to_rust(&module, "Config"),
        active_core::convert::schema_to_python(&module, "Config"),
        active_core::convert::schema_to_go(&module, "Config"),
    ] {
        let output = output.expect("generate");
        assert!(!output.contains("s3cr3t-gen"), "{output}");
    }
}

#[test]
fn enum_members_redact_on_every_schema_surface() {
    // Review 68: an enum member is authored text.
    let src =
        "s = secret(\"s3cr3t-enum\")\nschema = { e = Enum(\"s3cr3t-enum\", \"ok\") }\ne = \"ok\"\n";
    let module = parse(src, "enum-redact").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let report = active_core::convert::schema_inspect(&module).expect("schema inspect");
    assert!(!report.to_string().contains("s3cr3t-enum"), "{report}");
    let path = active_core::convert::inspect_path_with_profile(&module, &value, "e", None)
        .expect("inspect");
    assert!(!path.to_string().contains("s3cr3t-enum"), "{path}");
    let items = active_core::convert::collect_items(&module, &value);
    let md = active_core::convert::doc_to_markdown("Config", &items);
    assert!(!md.contains("s3cr3t-enum"), "{md}");
    let json_schema = active_core::convert::schema_to_json_schema(&module).expect("json");
    assert!(
        !json_schema.to_string().contains("s3cr3t-enum"),
        "{json_schema}"
    );
    for output in [
        active_core::convert::schema_to_typescript(&module, "Config"),
        active_core::convert::schema_to_rust(&module, "Config"),
        active_core::convert::schema_to_python(&module, "Config"),
        active_core::convert::schema_to_go(&module, "Config"),
    ] {
        let output = output.expect("generate");
        assert!(!output.contains("s3cr3t-enum"), "{output}");
    }
}

#[test]
fn generated_material_pattern_is_withheld_not_redacted() {
    // Review 68: a redacted regex is invalid syntax; drop it.
    let src = "s = secret(\"s3cr3t-gen\")\nschema = { host = String(pattern = \"^s3cr3t-gen$\") }\nhost = \"x\"\n";
    let module = parse(src, "pattern-valid").expect("parse");
    let json_schema = active_core::convert::schema_to_json_schema(&module).expect("json");
    assert!(
        !json_schema.to_string().contains("pattern"),
        "{json_schema}"
    );
    let ts = active_core::convert::schema_to_typescript(&module, "Config").expect("ts");
    assert!(!ts.contains("regex"), "{ts}");
    assert!(!ts.contains("s3cr3t-gen"), "{ts}");
}

#[test]
fn plan_and_repair_withhold_a_profile_material_default() {
    // Review 68: the planning profile carries the selected profile.
    let src = "a = secret(\"base\")\nprofile \"p\" { a = \"s3cr3t-default\" }\nschema = { tls = Bool, cert = String(requires = \"tls\", default = \"s3cr3t-default\") }\ntls = true\n";
    let module = parse(src, "plan-profile").expect("parse");
    let mut engine = Engine::hermetic();
    engine.profile_override = Some("p".to_string());
    let (_, diags) = engine.evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::with_context(
        &module,
        Some("p"),
        Some(&engine.env),
        None,
    );
    let suggestions =
        active_core::convert::suggest_patches_structured_with_profile(&module, &diags, &profile);
    for suggestion in suggestions {
        assert!(
            !suggestion.directive.contains("s3cr3t-default"),
            "{}",
            suggestion.directive
        );
    }
    let outcome = active_core::convert::repair_with_engine(
        src,
        active_core::diag::RepairGoal::Validate,
        &[],
        10,
        &engine,
        None,
    );
    for step in &outcome.steps {
        assert!(
            !step.directive.contains("s3cr3t-default"),
            "{}",
            step.directive
        );
    }
    for finding in &outcome.findings {
        assert!(
            !finding.message.contains("s3cr3t-default"),
            "{}",
            finding.message
        );
    }
}

#[test]
fn plan_withholds_an_environment_material_default() {
    // Review 68: the same guard covers host values.
    let src = "s = secret(env.ACTIVE_R68_TOKEN)\nschema = { tls = Bool, cert = String(requires = \"tls\", default = \"TOKENVALUE\") }\ntls = true\n";
    let module = parse(src, "plan-env").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R68_TOKEN".to_string(), "TOKENVALUE".to_string());
    let engine = Engine::hermetic().with_env(env);
    let (_, diags) = engine.evaluate(&module).expect("evaluate");
    let profile =
        active_core::secret::SecretProfile::with_context(&module, None, Some(&engine.env), None);
    let suggestions =
        active_core::convert::suggest_patches_structured_with_profile(&module, &diags, &profile);
    for suggestion in suggestions {
        assert!(
            !suggestion.directive.contains("TOKENVALUE"),
            "{}",
            suggestion.directive
        );
    }
    let outcome = active_core::convert::repair_with_engine(
        src,
        active_core::diag::RepairGoal::Validate,
        &[],
        10,
        &engine,
        None,
    );
    for step in &outcome.steps {
        assert!(!step.directive.contains("TOKENVALUE"), "{}", step.directive);
    }
    for finding in &outcome.findings {
        assert!(
            !finding.message.contains("TOKENVALUE"),
            "{}",
            finding.message
        );
    }
}

#[test]
fn diagnostics_redact_environment_schema_literals() {
    // Review 68: host values need the engine environment.
    let src = "s = secret(env.ACTIVE_R68_FMT)\nschema = { f = String(format = \"TOKENVALUE\") }\nf = \"x\"\n";
    let module = parse(src, "diag-env").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("ACTIVE_R68_FMT".to_string(), "TOKENVALUE".to_string());
    let engine = Engine::hermetic().with_env(env);
    let diags = match engine.evaluate(&module) {
        Ok((_, diags)) => diags,
        Err(EvalError::ValidationFailed { diagnostics }) => diagnostics,
        Err(error) => panic!("{error:?}"),
    };
    for diagnostic in &diags {
        assert!(
            !diagnostic.message.contains("TOKENVALUE"),
            "{}",
            diagnostic.message
        );
    }
}

#[test]
fn nested_table_metadata_is_redacted() {
    // Review 68: nested schema tables carry their own metadata.
    let src = "s = secret(\"1.2.3\")\nschema = { db = Table { version = \"1.2.3\", min_reader = \"1.2.3\", host = String } }\ndb = { host = \"x\" }\n";
    let module = parse(src, "nested-meta").expect("parse");
    let report = active_core::convert::schema_inspect(&module).expect("schema inspect");
    assert!(!report.to_string().contains("1.2.3"), "{report}");
}

#[test]
fn docgen_api_redacts_value_text() {
    // Review 68: the public item builder may receive a raw value.
    let src = "a = \"s3cr3t-locator-value\"\nk = secret(a)\nb = true\n";
    let module = parse(src, "doc-value").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let items = active_core::convert::collect_items(&module, &value);
    let md = active_core::convert::doc_to_markdown("Config", &items);
    assert!(!md.contains("s3cr3t-locator-value"), "{md}");
}

#[test]
fn requires_targets_stay_structural() {
    // Review 68: a requires target is a reference, like a field name.
    let src = "s = secret(\"tls\")\nschema = { tls = Bool, cert = String(requires = \"tls\") }\ntls = true\n";
    let module = parse(src, "requires-structural").expect("parse");
    let report = active_core::convert::schema_inspect(&module).expect("schema inspect");
    let fields = report["fields"].as_array().expect("fields");
    let cert = fields
        .iter()
        .find(|entry| entry["name"] == "cert")
        .expect("cert entry");
    assert_eq!(cert["requires"], "tls", "{cert}");
}

#[test]
fn parse_diagnostic_does_not_echo_a_version_value() {
    // Review 69: parse errors abort before evaluation redaction.
    let src = "s = secret(\"s3cr3tAlpha\")\nschema = { version = \"s3cr3tAlpha\", a = String }\na = \"x\"\n";
    let diags = parse(src, "parse-echo").expect_err("version must be rejected");
    for diagnostic in diags {
        assert!(
            !diagnostic.message.contains("s3cr3tAlpha"),
            "{}",
            diagnostic.message
        );
    }
}

#[test]
fn inspect_does_not_echo_a_non_numeric_bound() {
    // Review 69: a bound slot takes a number, never authored text.
    let src = "s = secret(\"s3cr3tAlpha\")\nschema = { a = Int(min = \"s3cr3tAlpha\") }\na = 1\n";
    let module = parse(src, "bound-slot").expect("parse");
    let report = active_core::convert::schema_inspect(&module).expect("schema inspect");
    assert!(!report.to_string().contains("s3cr3tAlpha"), "{report}");
}

#[test]
fn t004_names_a_resolvable_requires_target() {
    // Review 69: the spec requires naming the target when it resolves.
    let src = "s = secret(\"tls\")\nschema = { tls = Bool, cert = String(requires = \"tls\") }\ntls = true\n";
    let module = parse(src, "requires-t004").expect("parse");
    let diags = match Engine::hermetic().evaluate(&module) {
        Ok((_, diags)) => diags,
        Err(EvalError::ValidationFailed { diagnostics }) => diagnostics,
        Err(error) => panic!("{error:?}"),
    };
    let t004 = diags.iter().find(|d| d.code == "T004").expect("T004");
    assert!(t004.message.contains("'tls'"), "{}", t004.message);
}

#[test]
fn generated_machine_slots_withhold_material_values() {
    // Review 69: a marker in a format/media/default slot is invalid.
    let src = "s = secret(\"s3cr3t-gen\")\nschema = { g = String(format = \"s3cr3t-gen\", default = \"s3cr3t-gen\"), f = Blob(media_type = \"s3cr3t-gen\") }\ng = \"x\"\nf = blob(\"text/plain\", \"x\")\n";
    let module = parse(src, "machine-slots").expect("parse");
    let json_schema = active_core::convert::schema_to_json_schema(&module).expect("json");
    let text = json_schema.to_string();
    assert!(!text.contains("s3cr3t-gen"), "{text}");
    assert!(!text.contains("\"format\""), "{text}");
    assert!(!text.contains("x-active-media-type"), "{text}");
}

#[test]
fn viz_repair_keeps_the_directive_path_structural() {
    // Review 69: planning withholds material fills; the path must stay
    // applicable.
    let src = "s = secret(\"cert\")\nschema = { cert = String(default = \"v\") }\n";
    let module = parse(src, "viz-repair-path").expect("parse");
    let (_, diags) = Engine::hermetic().evaluate(&module).expect("evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    let suggestions =
        active_core::convert::suggest_patches_structured_with_profile(&module, &diags, &profile);
    for suggestion in &suggestions {
        if suggestion.directive.contains("cert") {
            assert!(
                !suggestion.directive.contains("***SECRET***"),
                "{}",
                suggestion.directive
            );
        }
    }
}
