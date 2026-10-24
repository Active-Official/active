//! Content-addressed values + Blob (PROPOSAL §1.1, spec/blob.md,
//! CHECKLIST Phase CA): identity, materialisation, taint, budgets,
//! B00x diagnostics — all through the real parse/evaluate paths.

use active_core::convert::{require_materialised, value_to_json};
use active_core::{parse, Engine, EvalError, Value};

const HELLO_DIGEST: &str = "ef8a8525ffd1d32141916f461ccb49c92ee84d57279145268364fb7c1c179eea";

fn evaluated(src: &str) -> (Value, Vec<active_core::Diagnostic>) {
    let module = parse(src, "blob").expect("must parse");
    Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("must evaluate: {e:?}"))
}

fn evaluated_strict(src: &str) -> Result<Value, EvalError> {
    let module = parse(src, "blob").expect("must parse");
    let mut engine = Engine::new();
    engine.resilient = false;
    engine.evaluate(&module).map(|(v, _)| v)
}

fn blob_of(value: &Value) -> (&[u8; 32], u64, bool, &str, &Option<Vec<u8>>) {
    match value {
        Value::Table(map) => match map.get("f").expect("field f") {
            Value::Blob {
                hash,
                size,
                tainted,
                media_type,
                payload,
            } => (hash, *size, *tainted, media_type, payload),
            other => panic!("expected blob, got {other:?}"),
        },
        other => panic!("expected table, got {other:?}"),
    }
}

#[test]
fn identity_matches_pinned_digest() {
    // spec/blob.md §2: the canonical digest for ("text/plain", "hello").
    let (value, _) = evaluated("f = blob(\"text/plain\", \"hello\")\n");
    let (hash, size, tainted, media, payload) = blob_of(&value);
    assert_eq!(active_core::blob::hash_hex(hash), HELLO_DIGEST);
    assert_eq!(size, 5);
    assert!(!tainted);
    assert_eq!(media, "text/plain");
    // Identity only: no payload without explicit materialisation.
    assert!(payload.is_none(), "payload must not be retained by default");
    let json = value_to_json(&value);
    assert_eq!(
        json["f"]["$active.blob"]["hash"],
        format!("blake3:{HELLO_DIGEST}")
    );
    assert_eq!(json["f"]["$active.blob"]["size"], 5);
    assert!(json["f"]["$active.blob"].get("payload_b64").is_none());
}

#[test]
fn materialised_flag_embeds_payload() {
    let module = parse("f = blob(\"text/plain\", \"hello\")\n", "blob").expect("must parse");
    let mut engine = Engine::new();
    engine.materialise_blobs = true;
    let (value, _) = engine.evaluate(&module).expect("must evaluate");
    require_materialised(&value).expect("demand must succeed");
    let json = value_to_json(&value);
    assert_eq!(json["f"]["$active.blob"]["payload_b64"], "aGVsbG8=");
}

#[test]
fn demand_without_flag_fails_closed_b005() {
    // Evaluated identity-only, then materialisation demanded after
    // the fact: B005 instead of a silent identity emission.
    let (value, _) = evaluated("f = blob(\"text/plain\", \"hello\")\n");
    let err = require_materialised(&value).expect_err("must refuse");
    assert_eq!(err.code(), "B005");
}

#[test]
fn secret_derived_blob_is_tainted_and_never_materialises() {
    let module =
        parse("f = blob(\"text/plain\", secret(\"s3cr3t\"))\n", "blob").expect("must parse");
    let mut engine = Engine::new();
    engine.materialise_blobs = true;
    let (value, _) = engine.evaluate(&module).expect("must evaluate");
    let (_, _, tainted, _, payload) = blob_of(&value);
    assert!(tainted, "secret-derived blob must be tainted");
    assert!(payload.is_none(), "tainted blobs never retain payloads");
    let err = require_materialised(&value).expect_err("must refuse");
    assert_eq!(err.code(), "B003");
    // No secret material anywhere on the export path.
    let json = value_to_json(&value);
    let text = serde_json::to_string(&json).expect("valid json");
    assert!(!text.contains("s3cr3t"), "raw secret leaked: {text}");
    assert_eq!(json["f"]["$active.blob"]["tainted"], true);
    let debug = format!("{value:?}");
    assert!(!debug.contains("s3cr3t"), "debug leaked: {debug}");
}

#[test]
fn malformed_blob_call_is_b001() {
    let (_, errors) = active_core::parse_recovering(
        "f = blob(42, \"hello\")\n",
        "blob",
        active_core::ParseOptions::strict(),
    );
    assert!(
        errors.iter().any(|d| d.code == "B001"),
        "expected B001, got {errors:?}"
    );
}

#[test]
fn max_size_breach_is_b002_and_media_mismatch_is_b004() {
    let module = parse(
        "schema = { a = Blob(max_size = 2), b = Blob(media_type = \"image/png\") }\na = blob(\"text/plain\", \"hello\")\nb = blob(\"text/plain\", \"hello\")\n",
        "blob",
    )
    .expect("must parse");
    let mut engine = Engine::new();
    engine.resilient = false;
    let err = engine.evaluate(&module).expect_err("must fail validation");
    let diags = match err {
        EvalError::ValidationFailed { diagnostics } => diagnostics,
        other => panic!("expected validation failure, got {other:?}"),
    };
    let codes: Vec<&str> = diags.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"B002"), "expected B002, got {codes:?}");
    assert!(codes.contains(&"B004"), "expected B004, got {codes:?}");
    for d in &diags {
        assert!(!d.message.contains("hello"), "payload in message: {d:?}");
    }
}

#[test]
fn blob_field_rejects_non_blob_with_t002() {
    let err = evaluated_strict("schema = { f = Blob }\nf = 42\n").expect_err("must fail");
    assert_eq!(err.code(), "T002");
}

#[test]
fn blob_materialisation_is_charged_to_size_budget() {
    // 5 payload bytes against a 2-byte materialised-size budget.
    let module = parse("f = blob(\"text/plain\", \"hello\")\n", "blob").expect("must parse");
    let mut engine = Engine::new();
    engine.materialise_blobs = true;
    engine.budget = active_core::Budget {
        max_value_size: 2,
        ..active_core::Budget::default()
    };
    let err = engine.evaluate(&module).expect_err("must trip X006");
    assert_eq!(err.code(), "X006");
}

#[test]
fn blob_source_round_trips_through_fmt() {
    let src = "f = blob(\"text/plain\", \"hello\")\n";
    let module = parse(src, "blob").expect("must parse");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    assert!(out.contains("blob(\"text/plain\", \"hello\")"), "{out}");
}

#[test]
fn blob_aliases_are_charged_per_clone() {
    // N aliases of a materialised blob must charge N payloads:
    // the 1MB-payload-times-1000-aliases class dies here, not in OOM.
    // The budget is chosen so the SAME payload fits alone (and with
    // five aliases at twice the limit) while ten aliases trip: only
    // cumulative per-clone charging can explain the difference.
    fn evaluate(aliases: usize, limit: u64) -> Result<(), String> {
        let mut src = String::from(
            "f = blob(\"text/plain\", \"0123456789012345678901234567890123456789\
012345678901234567890123456789012345678901234567890123456789\")\n",
        );
        for i in 0..aliases {
            src.push_str(&format!("a{i} = f\n"));
        }
        let module = parse(&src, "blob").expect("must parse");
        let mut engine = Engine::new();
        engine.materialise_blobs = true;
        engine.budget = active_core::Budget {
            max_total_value_size: limit,
            ..active_core::Budget::default()
        };
        engine
            .evaluate(&module)
            .map(|_| ())
            .map_err(|e| e.code().to_string())
    }
    assert!(evaluate(0, 1600).is_ok(), "one blob must fit");
    assert_eq!(
        evaluate(5, 1600),
        Err("X006".to_string()),
        "five clones trip"
    );
    assert!(
        evaluate(5, 3200).is_ok(),
        "five clones fit at twice the limit"
    );
    assert_eq!(
        evaluate(10, 3200),
        Err("X006".to_string()),
        "ten clones trip"
    );
}

#[test]
fn blob_toml_and_yaml_carry_identity_shape() {
    let (value, _) = evaluated("f = blob(\"text/plain\", \"hello\")\n");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml");
    assert!(toml.contains("$active.blob"), "{toml}");
    assert!(
        toml.contains("blake3:ef8a8525ffd1d32141916f461ccb49c92ee84d57279145268364fb7c1c179eea"),
        "{toml}"
    );
    assert!(
        !toml.contains("payload_b64"),
        "identity only without the flag: {toml}"
    );
    let yaml = active_core::convert::value_to_yaml_string(&value).expect("yaml");
    assert!(yaml.contains("$active.blob"), "{yaml}");
}

#[test]
fn blob_let_bound_variables_pass_b001_and_evaluate() {
    // Valid taint-via-alias must not false-positive B001 at parse.
    let (value, diags) = evaluated("let m = \"text/plain\"\nlet s = \"hello\"\nf = blob(m, s)\n");
    assert!(diags.iter().all(|d| d.code != "B001"), "{diags:?}");
    let (hash, _, _, _, _) = blob_of(&value);
    assert_eq!(active_core::blob::hash_hex(hash), HELLO_DIGEST);
}

#[test]
fn blob_arity_errors_are_b001_not_generic() {
    // The primary arity code is blob-specific (B001). Recovery may add
    // companion diagnostics (the codebase norm — cf. secret() arity),
    // but B001 leads and evaluation still reports instead of panicking.
    for src in [
        "f = blob(\"text/plain\")\n",
        "f = blob(\"a\", \"b\", \"c\")\n",
        "f = blob\n",
    ] {
        let (_, errors) =
            active_core::parse_recovering(src, "blob", active_core::ParseOptions::strict());
        let codes: Vec<&str> = errors.iter().map(|d| d.code.as_str()).collect();
        assert!(!codes.is_empty(), "{src} must error");
        assert_eq!(codes[0], "B001", "{src} must lead with B001, got {codes:?}");
    }
}

#[test]
fn blob_negative_or_wrong_typed_max_size_is_schema_bug() {
    let module = parse(
        "schema = { f = Blob(max_size = -1), g = Blob(media_type = 42) }\nf = blob(\"text/plain\", \"hello\")\ng = blob(\"text/plain\", \"hello\")\n",
        "blob",
    )
    .expect("must parse");
    let mut engine = Engine::new();
    engine.resilient = false;
    let err = engine.evaluate(&module).expect_err("schema bugs must fail");
    let diags = match err {
        EvalError::ValidationFailed { diagnostics } => diagnostics,
        other => panic!("expected validation failure, got {other:?}"),
    };
    assert!(
        diags.iter().any(|d| d.message.contains("invalid max_size")),
        "{diags:?}"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("invalid media_type")),
        "{diags:?}"
    );
}

#[test]
fn materialise_flag_flip_clears_incremental_memo() {
    // The memo token includes the materialise flag: flipping it can
    // never serve a stale identity-only tree as materialised.
    let module = parse("f = blob(\"text/plain\", \"hello\")\n", "blob").expect("must parse");
    let mut memo = active_core::IncrementalMemo::default();
    let mut engine = Engine::new();
    let (v1, _) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("first eval");
    assert!(!active_core::convert::value_to_json(&v1)
        .to_string()
        .contains("payload_b64"));
    engine.materialise_blobs = true;
    let (v2, _) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("second eval");
    assert!(active_core::convert::value_to_json(&v2)
        .to_string()
        .contains("payload_b64"));
}
