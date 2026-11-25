//! The redaction membrane (L-P36): the type-enforced boundary between a
//! profile-redacted tree and every serializer.
//!
//! Coverage map for `L-P36-DESIGN.md` section 7:
//! - 7.1/7.2 are `compile_fail` doctests; a positive-control doctest pins
//!   the sanctioned door (and the exact symbols the `compile_fail` cases
//!   name, so those cannot pass on an unresolved path): a doctest fails
//!   `cargo test` the moment a forbidden call compiles again.
//! - 7.3 (canonical bytes, `-0.0`/zero quantities), 7.4 (payload),
//!   7.5 (profile-material), 7.6 (projection) are pinned here.
//! - 7.7 (shipped paths keep their bytes) is pinned by the conformance
//!   harness for the C ABI and WASM surfaces, by `active-cli/tests/cli.rs`
//!   for CLI bytes, by `active-core/tests/blob.rs` and
//!   `active-core/tests/interop_wire.rs` for the exporters, and by
//!   `active-lsp/tests/lsp.rs::hover_redacts_secrets` for LSP hover.
//! - 7.8 (negative control) was run by hand: re-adding
//!   `impl Serialize for Value` made the 7.1 doctest fail, then it was
//!   removed. That doctest is itself the regression pin.
//! - 7.9: `ir.rs` carries no `payload_b64` claim and no
//!   `impl Serialize for Value`; 7.1/7.2 enforce it structurally.

use active_core::convert::{redact_value, value_to_json, value_to_json_materialised};
use active_core::secret::SecretProfile;
use active_core::{parse, Engine, Unit, Value};

/// A profile over an empty document: nothing is marked material, so the
/// membrane's own behaviour is isolated from the redaction rules.
fn bare_profile() -> SecretProfile {
    SecretProfile::analyze(&parse("", "key").expect("parse"))
}

#[test]
fn sealed_serialization_is_canonical_for_edge_numbers() {
    // 7.3: `Serialize` on the sealed value IS `value_to_json`, so the two
    // can never drift again - including the `-0.0` / zero-quantity cases
    // the deleted hand-kept impl spelled differently (design section 2).
    let profile = bare_profile();
    let cases = [
        Value::Float(-0.0),
        Value::Float(0.0),
        Value::Quantity(-0.0, Unit::from_suffix("Hz").expect("unit")),
        Value::Quantity(0.0, Unit::from_suffix("Hz").expect("unit")),
    ];
    for value in cases {
        let sealed = redact_value(&profile, &value);
        let via_serde = serde_json::to_string(&sealed).expect("serialize");
        let canonical = value_to_json(sealed.as_value()).to_string();
        assert_eq!(via_serde, canonical, "{value:?}");
    }
    // Byte-level pins of the canonical spelling: zero has one sign (G13).
    let neg_zero = redact_value(&profile, &Value::Float(-0.0));
    assert_eq!(serde_json::to_string(&neg_zero).expect("serialize"), "0.0");
    let neg_zero_qty = redact_value(
        &profile,
        &Value::Quantity(-0.0, Unit::from_suffix("Hz").expect("unit")),
    );
    assert_eq!(
        serde_json::to_string(&neg_zero_qty).expect("serialize"),
        "{\"$active.quantity\":{\"unit\":\"Hz\",\"value\":0.0}}"
    );
}

#[test]
fn a_retained_payload_crosses_only_the_sealed_materialised_door() {
    // 7.4: an untainted retained blob's bytes never reach a generic
    // serializer; they cross only through the sealed materialised form.
    let profile = bare_profile();
    let blob = Value::Blob {
        hash: [3u8; 32],
        size: 5,
        tainted: false,
        media_type: "text/plain".to_string(),
        payload: Some(b"hello".to_vec()),
    };
    let sealed = redact_value(&profile, &blob);
    let generic = serde_json::to_string(&sealed).expect("serialize");
    assert!(!generic.contains("payload_b64"), "{generic}");
    assert!(!generic.contains("aGVsbG8="), "{generic}");
    let materialised = value_to_json_materialised(&sealed);
    assert_eq!(materialised["$active.blob"]["payload_b64"], "aGVsbG8=");
}

#[test]
fn a_profile_material_plain_path_is_a_marker_through_the_seal() {
    // 7.5: a plain value that the profile selects is a marker in the
    // sealed output; the raw serializer that could emit it no longer
    // exists to call.
    let module = parse(
        "x = \"material-r5-9z\"\nk = secret(x)\nport = 8080\n",
        "material",
    )
    .expect("parse");
    let engine = Engine::hermetic();
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("eval");
    let profile = SecretProfile::for_engine(&module, &engine, &seeds);
    let sealed = redact_value(&profile, &value);
    let via_serde = serde_json::to_string(&sealed).expect("serialize");
    assert!(!via_serde.contains("material-r5-9z"), "{via_serde}");
    assert!(via_serde.contains("***SECRET***"), "{via_serde}");
    // The canonical mapping agrees with the sealed serializer.
    assert_eq!(via_serde, value_to_json(sealed.as_value()).to_string());
}

#[test]
fn projection_keeps_the_seal_and_document_relative_redaction() {
    // 7.6: `RedactedValue::project` selects a cone of the ALREADY-REDACTED
    // tree, so the whole-document sweep survives and the cone cannot be
    // re-serialized raw (the CLI `eval --path` inspect path).
    //
    // `db.plain` is a PLAIN value the profile selects because it is the
    // argument of `secret(...)`: the sweep is genuine, not a `Value::Secret`
    // shortcut already redacted before the profile ran (L-P36 review,
    // finding 5a).
    let module = parse(
        "[db]\nplain = \"proj-9z-nested\"\ntoken = secret(plain)\nport = 5432\nother = 1\n",
        "proj",
    )
    .expect("parse");
    let engine = Engine::hermetic();
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("eval");
    let profile = SecretProfile::for_engine(&module, &engine, &seeds);
    let sealed = redact_value(&profile, &value);

    let cone = sealed.project("db").expect("project db");
    let cone_json = serde_json::to_string(&cone).expect("serialize cone");
    assert!(!cone_json.contains("proj-9z-nested"), "{cone_json}");
    assert!(cone_json.contains("***SECRET***"), "{cone_json}");

    // Document-relative: the cone equals the `db` subtree of the fully
    // redacted document - no re-redaction at a new root, no leakage.
    let full = serde_json::to_value(&sealed).expect("full");
    assert_eq!(serde_json::to_value(&cone).expect("cone"), full["db"]);

    // Fail closed: a missing path errors, never a silent null.
    assert!(sealed.project("nope").is_err());
}
