//! L-P05: `--pure` evaluation. The refusals live in the engine, so these
//! tests drive `Engine` directly (the FFI/WASM path is the same code).

use active_core::eval::Budget;
use active_core::{parse, parse_file, Engine, EvalError, Mode, ParseOptions};
use std::path::PathBuf;

fn pure_engine() -> Engine {
    Engine::new()
        .with_env(std::collections::HashMap::new())
        .with_pure(true)
}

fn parse_ok(src: &str) -> active_core::Module {
    parse(src, "purity").unwrap_or_else(|diags| panic!("parse: {diags:?}"))
}

fn expect_refusal(engine: &Engine, module: &active_core::Module) -> EvalError {
    match engine.evaluate(module) {
        Err(error) => {
            assert_eq!(error.code(), "E047", "{error:?}");
            error
        }
        Ok((_, diags)) => panic!("pure evaluation must refuse; got {diags:?}"),
    }
}

#[test]
fn an_env_read_is_refused_and_named() {
    let engine = pure_engine();
    let module = parse_ok("v = env.PURITY_PROBE ?: 1\n");
    let error = expect_refusal(&engine, &module);
    assert!(error.to_string().contains("PURITY_PROBE"), "{error}");
    assert!(error.to_string().contains("env read"), "{error}");
    // The same document evaluates cleanly without the mode.
    let (value, diags) = Engine::new()
        .with_env(std::collections::HashMap::new())
        .evaluate(&module)
        .expect("plain evaluation");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(
        active_core::convert::value_to_pretty_json(&value),
        "{\n  \"v\": 1\n}"
    );
}

#[test]
fn a_secret_env_locator_is_refused() {
    // A `secret.env` locator is an Env node in the arena, so the same
    // pre-scan catches it (the secret machinery is not a purity bypass).
    let engine = pure_engine();
    let module = parse_ok("token = secret.env(\"PURITY_TOKEN\")\n");
    let error = expect_refusal(&engine, &module);
    assert!(error.to_string().contains("PURITY_TOKEN"), "{error}");
}

#[test]
fn a_profile_override_is_refused_but_the_document_profile_is_allowed() {
    let mut engine = pure_engine();
    engine.profile_override = Some("prod".to_string());
    let module = parse_ok("profile \"prod\" { port = 2 }\nactive_profile = \"prod\"\nport = 1\n");
    let error = expect_refusal(&engine, &module);
    assert!(error.to_string().contains("profile override"), "{error}");
    assert!(error.to_string().contains("prod"), "{error}");
    // The document's own selection is document input: no override, no
    // refusal, and the profile applies.
    let (value, _) = pure_engine()
        .evaluate(&module)
        .expect("pure with document profile");
    assert!(
        active_core::convert::value_to_pretty_json(&value).contains("\"port\": 2"),
        "the document's own profile applies"
    );
}

#[test]
fn an_unpinned_include_is_refused() {
    let dir = std::env::temp_dir().join("active-purity-include");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("inc.active"), "host = \"db\"\n").expect("include");
    let main: PathBuf = dir.join("main.active");
    std::fs::write(&main, "include \"inc.active\"\n\nport = 1\n").expect("main");
    let module = parse_file(&main, ParseOptions::default()).expect("parse");
    let error = expect_refusal(&pure_engine(), &module);
    assert!(error.to_string().contains("unpinned include"), "{error}");
    assert!(error.to_string().contains("inc.active"), "{error}");
}

#[test]
fn a_pinned_include_is_allowed_under_purity() {
    use sha2::{Digest, Sha256};
    let dir = std::env::temp_dir().join("active-purity-pinned");
    std::fs::create_dir_all(&dir).expect("dir");
    let body = "host = \"db\"\n";
    std::fs::write(dir.join("inc.active"), body).expect("include");
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let main: PathBuf = dir.join("main.active");
    std::fs::write(
        &main,
        format!("include \"inc.active\" sha256:{digest}\n\nport = 1\n"),
    )
    .expect("main");
    let module = parse_file(&main, ParseOptions::default()).expect("parse");
    let (value, diags) = pure_engine()
        .evaluate(&module)
        .expect("a pinned include is pure");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(active_core::convert::value_to_pretty_json(&value).contains("db"));
}

#[test]
fn a_pure_document_evaluates_identically_with_and_without_the_mode() {
    let module =
        parse_ok("schema = { port = Int(min = 1), host = String }\nport = 8080\nhost = \"x\"\n");
    let plain = Engine::new()
        .with_env(std::collections::HashMap::new())
        .evaluate(&module)
        .expect("plain")
        .0;
    let pure = pure_engine().evaluate(&module).expect("pure").0;
    let plain_json = active_core::convert::value_to_pretty_json(&plain);
    let pure_json = active_core::convert::value_to_pretty_json(&pure);
    assert_eq!(plain_json, pure_json, "bytes differ");
    let budget = Budget::default();
    assert_eq!(
        active_core::envelope::bound_content_hash(
            &plain,
            None,
            None,
            &budget,
            Mode::Config,
            Some("agent"),
            false
        ),
        active_core::envelope::bound_content_hash(
            &pure,
            None,
            None,
            &budget,
            Mode::Config,
            Some("agent"),
            false
        ),
        "content hash differs"
    );
}

#[test]
fn the_refusal_names_are_deterministic() {
    // Profile override wins over include and env, matching the fixed
    // pre-scan order (documented in `pure_refusal`). This also exercises
    // the seeds entry point, which shares the same walk.
    let mut engine = pure_engine();
    engine.profile_override = Some("p".to_string());
    let module = parse_ok("v = env.X ?: 1\n");
    let error = expect_refusal(&engine, &module);
    assert!(error.to_string().contains("profile override"), "{error}");
    let seeds_error = match engine.evaluate_with_seeds(&module) {
        Err(error) => error,
        Ok(_) => panic!("seeds evaluation must refuse"),
    };
    assert_eq!(seeds_error.to_string(), error.to_string());
}
