//! Receiver-side interchange gate (LANG-C-004, spec/agent-interchange-profile.md
//! §4).
//!
//! The gate is default-on: a peer document that claims an envelope major this
//! library does not implement, or capabilities this host does not offer, is
//! refused BEFORE evaluation, in strict and resilient modes alike. Each
//! refusal is paired with a control that proves the same document evaluates
//! when the claim is acceptable or the gate is off.

use active_core::{parse, Engine, EnvelopeGate, EvalError, Mode};

fn codes(err: &EvalError) -> Vec<String> {
    match err {
        EvalError::ValidationFailed { diagnostics } => {
            diagnostics.iter().map(|d| d.code.clone()).collect()
        }
        other => panic!("expected a validation refusal, got {other:?}"),
    }
}

fn refusal(src: &str) -> EvalError {
    let module = parse(src, "gate").expect("parses");
    // Strict: E035 is an Error that resilient mode reports alongside the
    // tree, so both gate refusals and malformed-claim refusals surface as
    // an Err here.
    Engine::strict()
        .evaluate(&module)
        .expect_err("the gate must refuse")
}

fn envelope(version: &str) -> String {
    format!(
        "active_mode = \"data\"\nenvelope = {{ version = \"{version}\", tainted = false, payload = {{ ok = true }} }}\n"
    )
}

#[test]
fn a_clean_payload_with_an_unrelated_root_secret_is_accepted() {
    // LANG-F-004: the claim describes the PAYLOAD. A secret elsewhere in
    // the document must not make the receiver reject a clean payload.
    let src = "active_mode = \"data\"\nother = secret(\"root-secret\")\nenvelope = { version = \"1.0.0\", tainted = false, payload = { ok = true } }\n";
    let module = parse(src, "gate").expect("parse");
    let (_, diags) = Engine::hermetic().evaluate(&module).expect("evaluates");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn a_tainted_payload_with_a_true_claim_is_accepted() {
    let src = "active_mode = \"data\"\ntoken = secret(\"payload-secret\")\nenvelope = { version = \"1.0.0\", tainted = true, payload = token }\n";
    let module = parse(src, "gate").expect("parse");
    let (_, diags) = Engine::hermetic().evaluate(&module).expect("evaluates");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn a_plain_locator_copy_in_the_payload_fails_a_false_claim() {
    // LANG-F-004 (review): the locator source is a plain string, so the
    // payload carries material without a Secret-kinded value. A false
    // claim must be refused.
    let src = "active_mode = \"data\"\nkey = \"hunter2-material\"\ntoken = secret(key)\n\
               envelope = { version = \"1.0.0\", tainted = false, payload = { copy = key } }\n";
    let err = refusal(src);
    assert_eq!(codes(&err), ["E035"], "{err:?}");

    // The matching claim is accepted: the check verifies the assertion
    // rather than rejecting tainted payloads outright.
    let src = "active_mode = \"data\"\nkey = \"hunter2-material\"\ntoken = secret(key)\n\
               envelope = { version = \"1.0.0\", tainted = true, payload = { copy = key } }\n";
    let module = parse(src, "gate").expect("parse");
    let (_, diags) = Engine::hermetic().evaluate(&module).expect("evaluates");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn an_interpolated_locator_copy_in_the_payload_fails_a_false_claim() {
    // A locator assembled by interpolation is material without being a
    // static literal, so the runtime seed set is one of the sources that
    // marks its copy.
    let src = "active_mode = \"data\"\nn = 7\nlet key = \"hunter2-runtime-${n}\"\n\
               token = secret(key)\n\
               envelope = { version = \"1.0.0\", tainted = false, payload = { copy = key } }\n";
    let err = refusal(src);
    assert_eq!(codes(&err), ["E035"], "{err:?}");
}

#[test]
fn an_unrelated_plain_value_in_the_payload_stays_clean() {
    // Control for the locator rule: a plain value that is NOT used as a
    // locator is ordinary data, so a clean payload keeps a false claim.
    let src = "active_mode = \"data\"\nkey = \"hunter2-material\"\ntoken = secret(key)\n\
               public = \"documented-default\"\n\
               envelope = { version = \"1.0.0\", tainted = false, payload = { v = public } }\n";
    let module = parse(src, "gate").expect("parse");
    let (_, diags) = Engine::hermetic().evaluate(&module).expect("evaluates");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn a_profile_replacement_inherits_payload_taint_for_non_string_values() {
    // The selected profile replaces the material payload with an integer.
    // Materiality follows the path, so the export still renders the marker
    // there even though the value carries no material text; the claim
    // check must agree with the export in both directions. Config mode:
    // an interchange document (Data mode) refuses profiles outright with
    // E036, while a version-bearing envelope-shaped table is still policed.
    let base = "active_profile = \"prod\"\n\
                envelope = { version = \"1.0.0\", tainted = CLAIM, payload = secret(\"sensitive\") }\n\
                profile \"prod\" { envelope { payload = 42 } }\n";
    let err = refusal(&base.replace("CLAIM", "false"));
    // W003 records the profile's type change; E035 is the taint refusal.
    assert_eq!(codes(&err), ["W003", "E035"], "{err:?}");

    let module = parse(&base.replace("CLAIM", "true"), "gate").expect("parse");
    let engine = Engine::hermetic();
    let (value, diags, seeds) = engine
        .evaluate_with_seeds(&module)
        .expect("the accurate claim evaluates");
    assert!(
        diags
            .iter()
            .all(|d| d.severity == active_core::Severity::Warning),
        "{diags:?}"
    );
    let profile = active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds);
    let json = active_core::convert::value_to_json_with(&profile, &value);
    assert_eq!(
        json["envelope"]["payload"], "***SECRET***",
        "the export redacts the profile-replaced payload: {json}"
    );
}

#[test]
fn a_material_blob_media_type_taints_the_identity_and_the_claim() {
    // The media type equals a secret literal: the identity is material
    // even though the bytes are public. Construction must taint it (the
    // v1 hash would commit to the plaintext media string), so the claim
    // check sees a tainted payload and the export redacts the identity.
    let base = "active_mode = \"data\"\nkey = secret(\"x-private-type\")\n\
                envelope = { version = \"1.0.0\", tainted = CLAIM, \
                payload = blob(\"x-private-type\", \"public\") }\n";
    let err = refusal(&base.replace("CLAIM", "false"));
    assert_eq!(codes(&err), ["E035"], "{err:?}");

    let module = parse(&base.replace("CLAIM", "true"), "gate").expect("parse");
    let engine = Engine::hermetic();
    let (value, diags, seeds) = engine
        .evaluate_with_seeds(&module)
        .expect("the accurate claim evaluates");
    assert!(diags.is_empty(), "{diags:?}");
    let profile = active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds);
    let json = active_core::convert::value_to_json_with(&profile, &value);
    let identity = &json["envelope"]["payload"]["$active.blob"];
    assert_eq!(identity["tainted"], true, "{json}");
    assert_eq!(identity["media_type"], "***SECRET***", "{json}");
    // A salted v2 identity, not the public v1 hash of the plaintext pair.
    let plain = active_core::blob::hash_hex(&active_core::blob::hash("x-private-type", b"public"));
    assert_ne!(
        identity["hash"].as_str().unwrap_or_default(),
        format!("blake3:{plain}"),
        "a material media type must not hash under the public v1 domain: {json}"
    );
}

#[test]
fn a_material_payload_key_fails_a_false_claim() {
    // A table key is data too. Here the payload key spells the secret
    // literal, so the payload carries material even though every VALUE in
    // it is benign; the export must render the marker in the key slot.
    let base = "active_mode = \"data\"\nother = secret(\"token\")\n\
                envelope = { version = \"1.0.0\", tainted = CLAIM, \
                payload = { token = true, ok = 1 } }\n";
    let err = refusal(&base.replace("CLAIM", "false"));
    assert_eq!(codes(&err), ["E035"], "{err:?}");

    let module = parse(&base.replace("CLAIM", "true"), "gate").expect("parse");
    let engine = Engine::hermetic();
    let (value, diags, seeds) = engine
        .evaluate_with_seeds(&module)
        .expect("the accurate claim evaluates");
    assert!(diags.is_empty(), "{diags:?}");
    let profile = active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds);
    let json = active_core::convert::value_to_json_with(&profile, &value);
    let payload = json["envelope"]["payload"]
        .as_object()
        .expect("payload is a table");
    assert!(
        payload.contains_key("***SECRET***"),
        "the material key must render as the marker: {json}"
    );
    assert!(!payload.contains_key("token"), "{json}");
    assert_eq!(payload["ok"], 1, "a benign sibling stays visible: {json}");
}

#[test]
fn a_missing_taint_claim_is_refused() {
    // The field table marks tainted required: an envelope that makes no
    // verifiable claim must fail closed, not pass because it is silent.
    let src =
        "active_mode = \"data\"\nenvelope = { version = \"1.0.0\", payload = { ok = true } }\n";
    let err = refusal(src);
    assert_eq!(codes(&err), ["E035"], "{err:?}");
}

#[test]
fn a_malformed_taint_claim_is_refused() {
    let src = "active_mode = \"data\"\nenvelope = { version = \"1.0.0\", tainted = \"no\", payload = { ok = true } }\n";
    let err = refusal(src);
    assert_eq!(codes(&err), ["E035"], "{err:?}");
}

#[test]
fn an_envelope_without_a_payload_cannot_verify_taint() {
    let src = "active_mode = \"data\"\nenvelope = { version = \"1.0.0\", tainted = false }\n";
    let err = refusal(src);
    assert_eq!(codes(&err), ["E035"], "{err:?}");
}

#[test]
fn a_config_document_with_a_data_table_named_envelope_is_not_policed() {
    // Without Data mode or a version member, `envelope` is ordinary data;
    // the claim rule applies to interchange envelopes only.
    let src = "envelope = { x = 1 }\n";
    let module = parse(src, "gate").expect("parse");
    let (_, diags) = Engine::hermetic().evaluate(&module).expect("evaluates");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn a_v1_envelope_evaluates_cleanly() {
    let module = parse(&envelope("1.0.0"), "gate").expect("parse");
    assert_eq!(module.mode, Mode::Data);
    let (_, diags) = Engine::hermetic().evaluate(&module).expect("evaluates");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn unknown_envelope_majors_are_refused_before_evaluation() {
    for version in ["3.0.0", "9.9.9", "0.1.0", "42.0.0"] {
        let err = refusal(&envelope(version));
        assert_eq!(codes(&err), ["E033"], "{version}: {err:?}");
        assert!(
            err.to_string().contains("validation failed"),
            "the refusal is an evaluation error: {err}"
        );
    }
    // The message names the supported majors, so a sender can act on it.
    let err = refusal(&envelope("3.0.0"));
    let EvalError::ValidationFailed { diagnostics } = &err else {
        unreachable!()
    };
    assert!(
        diagnostics[0].message.contains("supported: 1, 2"),
        "{}",
        diagnostics[0].message
    );
}

#[test]
fn a_missing_or_malformed_version_is_refused() {
    let missing = "active_mode = \"data\"\nenvelope = { payload = { ok = true } }\n";
    assert_eq!(codes(&refusal(missing)), ["E033"]);

    for bad in ["one.0.0", "1.0", "1.0.0.0", "1.x.0", ""] {
        let err = refusal(&envelope(bad));
        assert_eq!(codes(&err), ["E033"], "{bad:?}: {err:?}");
    }
}

#[test]
fn a_non_string_version_is_refused() {
    let src = "active_mode = \"data\"\nenvelope = { version = 1, payload = { ok = true } }\n";
    assert_eq!(codes(&refusal(src)), ["E033"]);
}

#[test]
fn an_envelope_aliased_from_a_let_is_still_gated() {
    // `blk = { ... }` then `envelope = blk`: the version claim travels with
    // the table, so the alias must not be a way around the gate.
    let src = "active_mode = \"data\"\nlet blk = { version = \"9.0.0\" }\nenvelope = blk\n";
    let err = refusal(src);
    assert_eq!(codes(&err), ["E033"], "{err:?}");
}

#[test]
fn unsupported_capabilities_are_refused_and_offered_ones_pass() {
    let refused = "active_mode = \"data\"\nactive_capabilities = [\"eval\", \"teleport\"]\nx = 1\n";
    let err = refusal(refused);
    assert_eq!(codes(&err), ["E034"], "{err:?}");
    let EvalError::ValidationFailed { diagnostics } = &err else {
        unreachable!()
    };
    assert!(
        diagnostics[0].message.contains("not offered")
            && !diagnostics[0].message.contains("teleport"),
        "the capability name must not be echoed: {}",
        diagnostics[0].message
    );

    // Control: a capability the host offers passes, and so does a document
    // declaring none.
    for src in [
        "active_mode = \"data\"\nactive_capabilities = [\"eval\"]\nx = 1\n",
        "active_mode = \"data\"\nx = 1\n",
    ] {
        let module = parse(src, "gate").expect("parse");
        assert!(Engine::hermetic().evaluate(&module).is_ok(), "{src}");
    }
}

#[test]
fn refused_capabilities_are_reported_once_each_in_document_order() {
    let src = "active_mode = \"data\"\nactive_capabilities = [\"zeta\", \"eval\", \"zeta\", \"alpha\"]\nx = 1\n";
    let err = refusal(src);
    let EvalError::ValidationFailed { diagnostics } = &err else {
        unreachable!()
    };
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    for diagnostic in diagnostics {
        assert!(diagnostic.message.contains("not offered"));
        assert!(!diagnostic.message.contains("zeta"));
        assert!(!diagnostic.message.contains("alpha"));
    }
}

#[test]
fn config_mode_documents_are_not_gated() {
    // Config Mode documents are not interchange envelopes (§2 rule 1): the
    // same envelope shape without the mode marker evaluates.
    let src =
        "envelope = { version = \"9.9.9\", tainted = false, payload = { ok = true } }\nx = 1\n";
    let module = parse(src, "gate").expect("parse");
    assert_eq!(module.mode, Mode::Config);
    assert!(Engine::hermetic().evaluate(&module).is_ok());

    // Capabilities without the marker are also outside the profile.
    let src = "active_capabilities = [\"teleport\"]\nx = 1\n";
    let module = parse(src, "gate").expect("parse");
    assert!(Engine::hermetic().evaluate(&module).is_ok());
}

#[test]
fn the_gate_is_fatal_in_resilient_mode_too() {
    let module = parse(&envelope("9.9.9"), "gate").expect("parse");
    let mut engine = Engine::hermetic();
    engine.resilient = true;
    let err = engine
        .evaluate(&module)
        .expect_err("resilient mode still refuses");
    assert_eq!(codes(&err), ["E033"]);
}

#[test]
fn hosts_narrow_the_offered_set_with_the_shipped_policy() {
    // A host that implements only `check` refuses a document asking for
    // `eval`, while the shipped policy offers it.
    let src = "active_mode = \"data\"\nactive_capabilities = [\"eval\"]\nx = 1\n";
    let module = parse(src, "gate").expect("parse");

    let mut narrowed = Engine::hermetic();
    narrowed.envelope_gate = Some(EnvelopeGate::with_tools(["check"]));
    let err = narrowed
        .evaluate(&module)
        .expect_err("narrowed host refuses eval");
    assert_eq!(codes(&err), ["E034"]);

    // The shipped policy offers every tool this library ships.
    assert!(Engine::hermetic().evaluate(&module).is_ok());
}

#[test]
fn the_gate_can_be_disabled_for_a_trusted_receiver_and_nothing_else_changes() {
    let module = parse(&envelope("9.9.9"), "gate").expect("parse");
    let mut engine = Engine::hermetic();
    engine.envelope_gate = None;
    let (value, diags) = engine.evaluate(&module).expect("gate off: evaluates");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(matches!(value, active_core::Value::Table(_)));
}

#[test]
fn the_gate_runs_before_evaluation_work() {
    // A document with a type error AND a bad envelope version reports the
    // gate refusal only: evaluation never starts.
    let src = "active_mode = \"data\"\nenvelope = { version = \"9.9.9\" }\n\
               schema = { closed = false, a = Int }\na = \"not an int\"\n";
    let err = refusal(src);
    let EvalError::ValidationFailed { diagnostics } = &err else {
        unreachable!()
    };
    assert_eq!(codes(&err), ["E033"], "{diagnostics:?}");
}

#[test]
fn shipped_gate_offers_exactly_the_descriptor_tools() {
    // Single contract, two renderings: the shipped gate's tool set must be
    // the capability descriptor's `tools` names, in order.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../agent-kit/capability.schema.json");
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("descriptor readable"))
            .expect("descriptor json");
    let mut descriptor: Vec<String> = doc["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().expect("tool name").to_string())
        .collect();
    descriptor.sort();
    // Order is presentation; membership is the contract.
    let mut shipped = EnvelopeGate::shipped().host_tools;
    shipped.sort();
    assert_eq!(shipped, descriptor);
}

#[test]
fn a_cyclic_version_alias_is_refused_without_recursing() {
    // Review finding on LANG-C-004: the gate runs BEFORE evaluation, so the
    // S001 cycle check has not run. A `version` that aliases its way around
    // a loop must be refused, not walked recursively into a stack overflow.
    let src = "active_mode = \"data\"\nenvelope = { version = v }\nv = v\n";
    let module = parse(src, "gate").expect("parse");
    let err = Engine::hermetic()
        .evaluate(&module)
        .expect_err("a cyclic version cannot be negotiated");
    assert_eq!(codes(&err), ["E033"], "{err:?}");
}

#[test]
fn a_patch_cannot_rewrite_the_envelope_after_the_gate() {
    // The pre-gate sees 1.0.0; @patch rewrites the version. The
    // post-evaluation check must refuse it (L-T06 review C1).
    let src = "active_mode = \"data\"\nenvelope = { version = \"1.0.0\", payload = { ok = true } }\n@patch(\"envelope.version\", \"9.9.9\")\n";
    let module = parse(src, "gate").expect("parse");
    let err = Engine::new()
        .evaluate(&module)
        .expect_err("a patched envelope version must not pass");
    let text = format!("{err:?}");
    assert!(
        text.contains("E033") || text.contains("E019") || text.contains("E027"),
        "expected a fail-closed refusal, got {text}"
    );
}

#[test]
fn a_patch_cannot_remove_a_v2_receipt() {
    let bound = "sha256:cfd046a5febb85c8546852b2f25585a22d0e5746f22377225d0f0f6408f39f25";
    let src = format!(
        "active_mode = \"data\"\nenvelope = {{ version = \"2.0.0\", tainted = false, payload = {{ ok = true }}, receipt = {{ outcome = \"clean\", result = \"{bound}\" }}, result = \"{bound}\" }}\n@remove(\"envelope.receipt\")\n"
    );
    let module = parse(&src, "gate").expect("parse");
    let err = Engine::new()
        .evaluate(&module)
        .expect_err("patched-out receipt must not pass");
    let text = format!("{err:?}");
    assert!(
        text.contains("E042") || text.contains("E019") || text.contains("E027"),
        "expected a fail-closed refusal, got {text}"
    );
}

#[test]
fn a_profile_cannot_inject_an_envelope_after_the_gate() {
    let src = "active_mode = \"data\"\nactive_profile = \"prod\"\nprofile \"prod\" { envelope = { version = \"9.9.9\" } }\nx = 1\n";
    let module = parse(src, "gate").expect("parse");
    let err = Engine::new()
        .evaluate(&module)
        .expect_err("profile-injected envelope must not pass");
    let text = format!("{err:?}");
    assert!(
        text.contains("E033") || text.contains("E027"),
        "expected a fail-closed refusal, got {text}"
    );
}

#[test]
fn a_tampered_payload_with_a_stale_result_is_refused() {
    // The signature/result may be genuine for the original payload, but
    // the payload was changed: the post-evaluation bound check must catch
    // it (L-T06 review C2).
    let stale = "sha256:cfd046a5febb85c8546852b2f25585a22d0e5746f22377225d0f0f6408f39f25";
    let src = format!(
        "active_mode = \"data\"\nenvelope = {{ version = \"2.0.0\", capability = \"agent\", tainted = false, payload = {{ ok = true, port = 9090 }}, receipt = {{ outcome = \"clean\", mode = \"data\", result = \"{stale}\", budget = {{ nodes = 100000, depth = 256, array_len = 100000, keys = 100000, wall_ms = 100, value_size = 10000000, total_size = 100000000 }} }}, result = \"{stale}\" }}\n"
    );
    let module = parse(&src, "gate").expect("parse");
    let err = Engine::new()
        .evaluate(&module)
        .expect_err("a stale result must not verify");
    let text = format!("{err:?}");
    assert!(text.contains("E044"), "expected E044, got {text}");
}

#[test]
fn gate_diagnostics_never_echo_document_text() {
    // The version/capability values are document data and can be
    // secret-derived; E033/E034 use fixed wording (L-T06 review C3).
    let cases = [
        (
            "version",
            "active_mode = \"data\"\nkey = \"s3cr3t-version\"\ntoken = secret(key)\nenvelope = { version = key }\n",
            "s3cr3t-version",
        ),
        (
            "capability",
            "active_mode = \"data\"\ntoken = secret(\"s3cr3t-capability\")\nactive_capabilities = [\"s3cr3t-capability\"]\nx = 1\n",
            "s3cr3t-capability",
        ),
    ];
    for (label, src, material) in cases {
        let err = refusal(src);
        let text = format!("{err:?}");
        assert!(!text.contains(material), "{label}: material leaked: {text}");
        assert!(
            text.contains("E033") || text.contains("E034"),
            "{label}: expected a gate refusal, got {text}"
        );
    }
}

#[test]
fn a_v1_envelope_cannot_smuggle_v2_fields() {
    let src = "active_mode = \"data\"\nenvelope = { version = \"1.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"clean\" } }\n";
    let err = refusal(src);
    assert!(codes(&err).iter().any(|code| code == "E033"), "{err:?}");
}
