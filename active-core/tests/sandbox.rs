//! L-P28: hermetic sandbox over an explicit input closure.

use active_core::eval::EvalError;
use active_core::sandbox::{IncludeFile, InputClosure};
use active_core::{Engine, ParseOptions, Sandbox};
use std::collections::BTreeMap;
use std::sync::Arc;

fn digest(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

fn include(bytes: &str) -> IncludeFile {
    IncludeFile {
        bytes: Arc::from(bytes),
        sha256: digest(bytes.as_bytes()),
    }
}

fn closure(
    document: &str,
    includes: &[(&str, &str)],
    env: &[(&str, Option<&str>)],
    profile: Option<&str>,
) -> InputClosure {
    InputClosure::new(
        Arc::<str>::from(document),
        "closure.active",
        includes
            .iter()
            .map(|(key, bytes)| (key.to_string(), include(bytes)))
            .collect(),
        env.iter()
            .map(|(name, value)| (name.to_string(), value.map(str::to_string)))
            .collect::<BTreeMap<_, _>>(),
        profile.map(str::to_string),
    )
    .expect("closure")
}

#[test]
fn declared_env_is_the_only_environment() {
    let document = "p = env.P ?: \"default\"\n";
    let sandbox = Sandbox::new(closure(document, &[], &[("P", Some("declared"))], None)).unwrap();
    let module = sandbox.parse(ParseOptions::default()).unwrap();
    let (value, diags) = sandbox.evaluate(&Engine::hermetic(), &module).unwrap();
    assert!(diags.is_empty());
    assert_eq!(active_core::convert::value_to_json(&value)["p"], "declared");
}

#[test]
fn declared_absent_env_uses_the_document_default() {
    let document = "p = env.P ?: \"default\"\n";
    let sandbox = Sandbox::new(closure(document, &[], &[("P", None)], None)).unwrap();
    let module = sandbox.parse(ParseOptions::default()).unwrap();
    let (value, _) = sandbox.evaluate(&Engine::hermetic(), &module).unwrap();
    assert_eq!(active_core::convert::value_to_json(&value)["p"], "default");
}

#[test]
fn an_undeclared_env_read_is_e048() {
    let document = "p = env.OTHER\n";
    let sandbox = Sandbox::new(closure(document, &[], &[("P", Some("x"))], None)).unwrap();
    let module = sandbox.parse(ParseOptions::default()).unwrap();
    let error = sandbox
        .evaluate(&Engine::hermetic(), &module)
        .expect_err("undeclared env must refuse");
    assert_eq!(error.code(), "E048");
    assert!(matches!(error, EvalError::SandboxRefused { .. }));
}

#[test]
fn closure_includes_are_served_without_the_filesystem() {
    let document = "include \"lib.active\"\n";
    let sandbox = Sandbox::new(closure(
        document,
        &[("lib.active", "answer = 42\n")],
        &[],
        None,
    ))
    .unwrap();
    let module = sandbox.parse(ParseOptions::default()).unwrap();
    let (value, _) = sandbox.evaluate(&Engine::hermetic(), &module).unwrap();
    assert_eq!(active_core::convert::value_to_json(&value)["answer"], 42);
}

#[test]
fn a_missing_include_is_e048() {
    let document = "include \"missing.active\"\n";
    let sandbox = Sandbox::new(closure(document, &[], &[], None)).unwrap();
    let diagnostics = sandbox
        .parse(ParseOptions::default())
        .expect_err("missing include must refuse");
    assert!(
        diagnostics.iter().any(|d| d.code == "E048"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_closure_digest_mismatch_is_a_construction_error() {
    let mut includes = BTreeMap::new();
    let bytes = Arc::<str>::from("x = 1\n");
    includes.insert(
        "lib.active".to_string(),
        IncludeFile {
            bytes,
            sha256: [0u8; 32],
        },
    );
    let error = InputClosure::new(
        Arc::<str>::from("include \"lib.active\"\n"),
        "closure.active",
        includes,
        BTreeMap::new(),
        None,
    )
    .expect_err("digest mismatch");
    assert!(matches!(
        error,
        active_core::sandbox::ClosureError::IncludeDigest(_)
    ));
}

#[test]
fn profile_override_must_be_declared_by_the_document() {
    let document = "profile \"p\" {\n  a = 2\n}\na = 1\n";
    let sandbox = Sandbox::new(closure(document, &[], &[], Some("p"))).unwrap();
    let module = sandbox.parse(ParseOptions::default()).unwrap();
    let (value, _) = sandbox.evaluate(&Engine::hermetic(), &module).unwrap();
    assert_eq!(active_core::convert::value_to_json(&value)["a"], 2);

    let missing = Sandbox::new(closure(document, &[], &[], Some("nope"))).unwrap();
    let diagnostics = missing
        .parse(ParseOptions::default())
        .expect_err("undeclared profile must refuse");
    assert!(diagnostics.iter().any(|d| d.code == "E048"));
}

#[test]
fn a_host_fallback_is_refused() {
    let document = "p = env.P ?: \"default\"\n";
    let sandbox = Sandbox::new(closure(document, &[], &[("P", None)], None)).unwrap();
    let module = sandbox.parse(ParseOptions::default()).unwrap();
    let engine = Engine::hermetic().with_env_fallback(Arc::new(|_| Some("host".to_string())));
    assert!(engine.env_fallback.is_some(), "fallback must be set");
    let error = sandbox
        .evaluate(&engine, &module)
        .expect_err("fallback must refuse");
    assert_eq!(error.code(), "E048");
}

#[test]
fn the_closure_id_is_deterministic_and_content_bound() {
    let first = closure("a = 1\n", &[], &[("P", Some("x"))], None);
    let second = closure("a = 1\n", &[], &[("P", Some("x"))], None);
    let third = closure("a = 2\n", &[], &[("P", Some("x"))], None);
    assert_eq!(first.id(), second.id());
    assert_ne!(first.id(), third.id());
}

#[test]
fn same_closure_sandboxed_and_unsandboxed_bytes_diagnostics_hash_match() {
    use active_core::convert::{redact_value, value_to_json};
    use active_core::secret::SecretProfile;
    let directory = std::env::temp_dir().join("active-sandbox-equality");
    std::fs::create_dir_all(&directory).expect("dir");
    let doc_path = directory.join("main.active");
    let inc_path = directory.join("lib.active");
    std::fs::write(&inc_path, "answer = 42\n").expect("include");
    std::fs::write(&doc_path, "include \"lib.active\"\np = env.P ?: \"d\"\n").expect("doc");
    let closure = closure(
        "include \"lib.active\"\np = env.P ?: \"d\"\n",
        &[("lib.active", "answer = 42\n")],
        &[("P", Some("same"))],
        None,
    );
    let sandbox = Sandbox::new(closure).expect("sandbox");
    let sandboxed_module = sandbox.parse(ParseOptions::default()).expect("parse");
    let (sandboxed, sandboxed_diags) = sandbox
        .evaluate_redacted(&Engine::hermetic(), &sandboxed_module)
        .expect("sandboxed eval");

    let module = active_core::parse_file(&doc_path, ParseOptions::default()).expect("plain parse");
    let mut engine = Engine::hermetic();
    engine.env.insert("P".into(), "same".into());
    let (plain, plain_diags, seeds) = engine.evaluate_with_seeds(&module).expect("plain eval");
    let profile = SecretProfile::for_engine(&module, &engine, &seeds);
    let plain = redact_value(&profile, &plain);

    assert_eq!(
        serde_json::to_string(&value_to_json(&sandboxed)).unwrap(),
        serde_json::to_string(&value_to_json(&plain)).unwrap()
    );
    assert_eq!(
        sandboxed_diags
            .iter()
            .map(|d| (d.code.clone(), d.path.clone()))
            .collect::<Vec<_>>(),
        plain_diags
            .iter()
            .map(|d| (d.code.clone(), d.path.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        active_core::provenance::content_hash(&sandboxed),
        active_core::provenance::content_hash(&plain)
    );
}

#[test]
fn a_sandboxed_run_never_reuses_an_unsandboxed_memo() {
    let document = "p = env.P ?: \"d\"\n";
    let module = active_core::parse(document, "memo").expect("parse");
    let mut engine = Engine::hermetic();
    engine.env.insert("P".into(), "unsandboxed".into());
    let mut memo = active_core::IncrementalMemo::default();
    let (first, _) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("plain eval");
    assert_eq!(
        active_core::convert::value_to_json(&first)["p"],
        "unsandboxed"
    );

    let sandbox =
        Sandbox::new(closure(document, &[], &[("P", Some("sandboxed"))], None)).expect("sandbox");
    let sandboxed_module = sandbox.parse(ParseOptions::default()).expect("parse");
    let (second, _) = sandbox
        .evaluate_incremental(&Engine::hermetic(), &sandboxed_module, &mut memo)
        .expect("sandboxed eval");
    assert_eq!(
        active_core::convert::value_to_json(&second)["p"],
        "sandboxed",
        "a sandboxed run must not reuse the unsandboxed memo"
    );
}

#[test]
fn a_loader_digest_mismatch_is_e048() {
    let bytes = std::sync::Arc::<str>::from("answer = 42\n");
    let closure = InputClosure {
        document: std::sync::Arc::<str>::from("include \"lib.active\"\n"),
        source_name: "main.active".into(),
        includes: BTreeMap::from([(
            "lib.active".to_string(),
            IncludeFile {
                bytes,
                sha256: [0u8; 32],
            },
        )]),
        env: BTreeMap::new(),
        profile_override: None,
    };
    let sandbox = Sandbox::new(closure).expect("literal closure");
    let diagnostics = sandbox
        .parse(ParseOptions::default())
        .expect_err("loader digest mismatch must refuse");
    assert!(
        diagnostics.iter().any(|d| d.code == "E048"),
        "{diagnostics:?}"
    );
}

#[test]
fn cross_closure_module_is_e048_on_every_entry() {
    let a = Sandbox::new(closure("p = 1\n", &[], &[], None)).expect("a");
    let b = Sandbox::new(closure("p = 2\n", &[], &[], None)).expect("b");
    let module = b.parse(ParseOptions::default()).expect("parse");
    let engine = Engine::hermetic();
    assert_eq!(a.evaluate(&engine, &module).unwrap_err().code(), "E048");
    assert_eq!(
        a.evaluate_with_seeds(&engine, &module).unwrap_err().code(),
        "E048"
    );
    assert_eq!(
        a.evaluate_with_steps(&engine, &module).unwrap_err().code(),
        "E048"
    );
    let mut memo = active_core::IncrementalMemo::default();
    assert_eq!(
        a.evaluate_incremental(&engine, &module, &mut memo)
            .unwrap_err()
            .code(),
        "E048"
    );
}

#[test]
fn debug_forms_never_print_closure_material() {
    let sandbox = Sandbox::new(closure(
        "s = secret(\"super-secret-value\")\np = env.P\n",
        &[("lib.active", "include-bytes-marker = 1\n")],
        &[("P", Some("env-secret-value"))],
        None,
    ))
    .expect("sandbox");
    let module = sandbox.parse(ParseOptions::default()).expect("parse");
    let closure_debug = format!("{sandbox:?}");
    let module_debug = format!("{module:?}");
    for text in [&closure_debug, &module_debug] {
        assert!(!text.contains("super-secret-value"));
        assert!(!text.contains("env-secret-value"));
        assert!(!text.contains("include-bytes-marker"));
    }
}

#[test]
fn pure_wins_over_the_sandbox_guard_e047_precedence() {
    let document = "p = env.UNDECLARED\n";
    let sandbox = Sandbox::new(closure(document, &[], &[], None)).expect("sandbox");
    let module = sandbox.parse(ParseOptions::default()).expect("parse");
    let engine = Engine::hermetic().with_pure(true);
    let error = sandbox
        .evaluate(&engine, &module)
        .expect_err("pure must refuse");
    assert_eq!(error.code(), "E047", "pure precedence");
}

#[test]
fn strict_manifest_rejects_duplicate_and_mistyped_fields() {
    let manifest = |json: &str| {
        InputClosure::from_manifest(json, Some("a = 1\n"), |_| Err("no host reads".to_string()))
    };
    for json in [
        r#"{"version":1,"env":[],"env":[{"name":"P","set":true,"value":"x"}]}"#,
        r#"{"version":1,"env":[{"name":"P","value":123}]}"#,
        r#"{"version":1,"env":[{"name":"P","set":false,"extra":1}]}"#,
        r#"{"version":1,"includes":[{"path":"a.active","sha256":"00","bytes_b64":1}]}"#,
        r#"{"version":1,"unknown":true}"#,
    ] {
        let error = manifest(json).expect_err(json);
        assert!(
            matches!(error, active_core::ClosureError::Manifest(_)),
            "{json}: {error:?}"
        );
    }
}

#[test]
fn pure_beats_fallback_and_profile_refusals() {
    let document = "p = env.P ?: \"d\"\n";
    let sandbox = Sandbox::new(closure(document, &[], &[("P", None)], None)).expect("sandbox");
    let module = sandbox.parse(ParseOptions::default()).expect("parse");
    let engine = Engine::hermetic()
        .with_pure(true)
        .with_env_fallback(Arc::new(|_| Some("host".to_string())));
    assert_eq!(
        sandbox.evaluate(&engine, &module).unwrap_err().code(),
        "E047"
    );

    let profile_sandbox = Sandbox::new(closure(
        "profile \"p\" {\n  a = 2\n}\na = 1\n",
        &[],
        &[],
        Some("p"),
    ))
    .expect("sandbox");
    let profile_module = profile_sandbox
        .parse(ParseOptions::default())
        .expect("parse");
    let mut engine = Engine::hermetic().with_pure(true);
    engine.profile_override = Some("other".to_string());
    assert_eq!(
        profile_sandbox
            .evaluate(&engine, &profile_module)
            .unwrap_err()
            .code(),
        "E047"
    );
}

#[test]
fn a_path_aliasing_the_synthetic_root_is_refused() {
    let document = "include \".active-root\"\n";
    let sandbox = Sandbox::new(closure(document, &[(".active-root", "x = 1\n")], &[], None))
        .expect("closure");
    let diagnostics = sandbox
        .parse(ParseOptions::default())
        .expect_err("root alias must refuse");
    assert!(
        diagnostics.iter().any(|d| d.code == "E048"),
        "{diagnostics:?}"
    );
}
