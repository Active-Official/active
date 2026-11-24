//! L-P35 authority vocabulary, non-widening attenuation, receipt links.

use active_core::authority::{Capability, CapabilitySet};
use active_core::secret::SecretProfile;
use active_core::{envelope, Engine, ParseOptions};
use indexmap::IndexMap;

fn engine_with(capabilities: CapabilitySet) -> Engine {
    let mut engine = Engine::hermetic();
    engine.envelope_gate = None;
    engine.capabilities = capabilities;
    engine
}

fn document(caps: &str, body: &str) -> String {
    format!(
        "active_mode = \"data\"\nenvelope = {{\n  version = \"2.0.0\",\n  profile = \"agent\",\n  capabilities = {caps},\n  tainted = false,\n  payload = {{ ok = true }},\n}}\n{body}"
    )
}

fn eval(source: &str, capabilities: CapabilitySet) -> Result<active_core::Value, String> {
    let module = active_core::parse(source, "authority").map_err(|d| format!("{d:?}"))?;
    engine_with(capabilities)
        .evaluate(&module)
        .map(|(value, _)| value)
        .map_err(|error| error.code().to_string())
}

fn set(names: &[&str]) -> CapabilitySet {
    CapabilitySet::parse_list(names.iter().copied()).expect("set")
}

#[test]
fn narrowing_is_accepted_and_read_secrets_is_enforced() {
    let all = CapabilitySet::all();
    let clean = document("[\"evaluate\"]", "x = 1\n");
    assert!(eval(&clean, all).is_ok(), "narrow evaluate must work");

    let env = document("[\"evaluate\"]", "x = env.P\n");
    // No envelope gate; the capability gate is independent.
    assert_eq!(eval(&env, all).unwrap_err(), "E049");
    let env_with_secret = document("[\"evaluate\", \"read-secrets\"]", "x = env.P\n");
    assert!(eval(&env_with_secret, all).is_ok());
}

#[test]
fn receiver_refuses_a_capability_it_does_not_hold() {
    let widening = document("[\"evaluate\", \"apply\"]", "x = 1\n");
    let error = eval(&widening, set(&["evaluate"])).unwrap_err();
    assert_eq!(error, "E049");
}

#[test]
fn evaluate_is_required() {
    let no_eval = document("[\"apply\"]", "x = 1\n");
    assert_eq!(eval(&no_eval, CapabilitySet::all()).unwrap_err(), "E049");
}

#[test]
fn a_profile_name_never_grants_authority() {
    let env = document("[\"evaluate\"]", "x = env.P\n");
    let mut engine = engine_with(set(&["evaluate"]));
    engine.profile_override = Some("agent".into());
    let module = active_core::parse(&env, "authority").expect("parse");
    assert_eq!(engine.evaluate(&module).unwrap_err().code(), "E049");
}

fn attenuation(from: &[&str], to: &[&str], by: &str) -> IndexMap<String, active_core::Value> {
    let mut record = IndexMap::new();
    record.insert(
        "from".into(),
        active_core::Value::Array(
            from.iter()
                .map(|name| active_core::Value::String((*name).into()))
                .collect(),
        ),
    );
    record.insert(
        "to".into(),
        active_core::Value::Array(
            to.iter()
                .map(|name| active_core::Value::String((*name).into()))
                .collect(),
        ),
    );
    record.insert("by".into(), active_core::Value::String(by.into()));
    record
}

fn fields(
    caps: &[&str],
    receipt_attenuation: Option<IndexMap<String, active_core::Value>>,
) -> IndexMap<String, active_core::Value> {
    let mut fields = IndexMap::new();
    fields.insert(
        "capabilities".into(),
        active_core::Value::Array(
            caps.iter()
                .map(|name| active_core::Value::String((*name).into()))
                .collect(),
        ),
    );
    let mut receipt = IndexMap::new();
    if let Some(attenuation) = receipt_attenuation {
        receipt.insert("attenuation".into(), active_core::Value::Table(attenuation));
    }
    fields.insert("receipt".into(), active_core::Value::Table(receipt));
    fields
}

#[test]
fn receipt_records_and_checks_the_narrowing_link() {
    let all = ["evaluate", "read-secrets", "inspect", "apply", "sign"];
    let good = fields(
        &["evaluate"],
        Some(attenuation(&all, &["evaluate"], "local")),
    );
    envelope::check_attenuation(
        &good,
        match good.get("receipt") {
            Some(active_core::Value::Table(table)) => table,
            _ => unreachable!(),
        },
        "clean",
        &active_core::envelope::CheckContext {
            held: CapabilitySet::all(),
            granted: set(&["evaluate"]),
            budget: &active_core::eval::Budget::default(),
            profile: None,
            material: None,
            trust: None,
        },
    )
    .expect("a valid narrowing record");

    let widening = fields(
        &["evaluate"],
        Some(attenuation(&["evaluate"], &["evaluate", "apply"], "local")),
    );
    let error = envelope::check_attenuation(
        &widening,
        match widening.get("receipt") {
            Some(active_core::Value::Table(table)) => table,
            _ => unreachable!(),
        },
        "clean",
        &active_core::envelope::CheckContext {
            held: CapabilitySet::all(),
            granted: set(&["evaluate"]),
            budget: &active_core::eval::Budget::default(),
            profile: None,
            material: None,
            trust: None,
        },
    )
    .expect_err("widening link must fail");
    assert_eq!(error.code(), "E042");
}

#[test]
fn an_attenuation_record_is_required_when_capabilities_are_present() {
    let missing = fields(&["evaluate"], None);
    let error = envelope::check_attenuation(
        &missing,
        match missing.get("receipt") {
            Some(active_core::Value::Table(table)) => table,
            _ => unreachable!(),
        },
        "clean",
        &active_core::envelope::CheckContext {
            held: CapabilitySet::all(),
            granted: set(&["evaluate"]),
            budget: &active_core::eval::Budget::default(),
            profile: None,
            material: None,
            trust: None,
        },
    )
    .expect_err("attenuation required");
    assert_eq!(error.code(), "E042");
}

#[test]
fn a_two_hop_chain_keeps_the_narrow_set() {
    let all = ["evaluate", "read-secrets", "inspect", "apply", "sign"];
    let hop_a = attenuation(&all, &["evaluate"], "local");
    let hop_b = attenuation(&["evaluate"], &["evaluate"], "local");
    for (caps, record) in [(vec!["evaluate"], hop_a), (vec!["evaluate"], hop_b)] {
        let map = fields(&caps, Some(record));
        envelope::check_attenuation(
            &map,
            match map.get("receipt") {
                Some(active_core::Value::Table(table)) => table,
                _ => unreachable!(),
            },
            "clean",
            &active_core::envelope::CheckContext {
                held: CapabilitySet::all(),
                granted: set(&caps),
                budget: &active_core::eval::Budget::default(),
                profile: None,
                material: None,
                trust: None,
            },
        )
        .expect("each link narrows");
    }
    let _ = (Capability::Evaluate, ParseOptions::default());
}

#[test]
fn producer_cannot_grant_authority_it_does_not_hold() {
    let error =
        envelope::attenuation_value(set(&["evaluate"]), set(&["evaluate", "apply"]), "local")
            .expect_err("widening producer");
    assert_eq!(error.code(), "E049");

    let ok = envelope::attenuation_value(CapabilitySet::all(), set(&["evaluate"]), "local")
        .expect("narrowing link");
    let active_core::Value::Table(record) = ok else {
        panic!("record");
    };
    let map = fields(&["evaluate"], Some(record));
    envelope::check_attenuation(
        &map,
        match map.get("receipt") {
            Some(active_core::Value::Table(table)) => table,
            _ => unreachable!(),
        },
        "clean",
        &active_core::envelope::CheckContext {
            held: CapabilitySet::all(),
            granted: set(&["evaluate"]),
            budget: &active_core::eval::Budget::default(),
            profile: None,
            material: None,
            trust: None,
        },
    )
    .expect("producer link validates");
}

#[test]
fn post_evaluation_widened_claim_is_refused() {
    // The patched tree claims read-secrets while the receiver holds and
    // granted only evaluate. This is the post-transform check's input.
    let all = ["evaluate", "read-secrets", "inspect", "apply", "sign"];
    let widened = fields(
        &["evaluate", "read-secrets"],
        Some(attenuation(&all, &["evaluate", "read-secrets"], "local")),
    );
    let error = envelope::check_attenuation(
        &widened,
        match widened.get("receipt") {
            Some(active_core::Value::Table(table)) => table,
            _ => unreachable!(),
        },
        "clean",
        &active_core::envelope::CheckContext {
            held: set(&["evaluate"]),
            granted: set(&["evaluate"]),
            budget: &active_core::eval::Budget::default(),
            profile: None,
            material: None,
            trust: None,
        },
    )
    .expect_err("widened claim");
    assert_eq!(error.code(), "E049");
}

#[test]
fn an_aliased_envelope_cannot_bypass_the_capability_gate() {
    for (caps, expected) in [
        (r#"["apply"]"#, "E049"),
        (r#"["evaluate", "apply"]"#, "E049"),
    ] {
        let source = format!(
            "active_mode = \"data\"\nbase = {{ version = \"2.0.0\", profile = \"agent\", capabilities = {caps}, tainted = false, payload = {{ ok = true }} }}\nenvelope = base\nx = 1\n"
        );
        let module = active_core::parse(&source, "alias").expect("parse");
        let mut engine = Engine::hermetic();
        engine.capabilities = set(&["evaluate"]);
        assert_eq!(
            engine.evaluate(&module).unwrap_err().code(),
            expected,
            "{caps}"
        );
    }
}

#[test]
fn malformed_capability_lists_are_e042_with_a_fixed_message() {
    for caps in [
        "[\"evaluate\", \"evaluate\"]",
        "[\"admin\"]",
        "\"evaluate\"",
    ] {
        let source = format!(
            "active_mode = \"data\"\nenvelope = {{ version = \"2.0.0\", profile = \"agent\", capabilities = {caps}, tainted = false, payload = {{}} }}\n"
        );
        let module = active_core::parse(&source, "malformed").expect("parse");
        let error = engine_with(CapabilitySet::all())
            .evaluate(&module)
            .expect_err("malformed caps");
        assert_eq!(error.code(), "E042", "{caps}");
        assert!(!format!("{error}").contains("admin"), "name echoed");
    }
}

#[test]
fn the_shipped_gate_accepts_a_valid_attenuated_envelope() {
    let source = r#"active_mode = "data"
envelope = { version = "2.0.0", profile = "agent", capabilities = ["evaluate"],
             tainted = false,
             receipt = { outcome = "failed", failure_class = "other", mode = "data",
                         attenuation = { from = ["evaluate", "read-secrets", "inspect", "apply", "sign"],
                                         to = ["evaluate"], by = "local" } } }
x = 1
"#;
    let module = active_core::parse(source, "gate").expect("parse");
    let mut engine = Engine::hermetic();
    engine.envelope_gate = Some(active_core::EnvelopeGate::shipped());
    engine.capabilities = CapabilitySet::all();
    engine.evaluate(&module).expect("valid attenuated envelope");
}

#[test]
fn attenuation_by_must_match_sender_or_local() {
    let all = ["evaluate", "read-secrets", "inspect", "apply", "sign"];
    let mut map = fields(
        &["evaluate"],
        Some(attenuation(&all, &["evaluate"], "agent:other")),
    );
    let mut sender = IndexMap::new();
    sender.insert("id".into(), active_core::Value::String("agent:me".into()));
    map.insert("sender".into(), active_core::Value::Table(sender));
    let error = envelope::check_attenuation(
        &map,
        match map.get("receipt") {
            Some(active_core::Value::Table(table)) => table,
            _ => unreachable!(),
        },
        "clean",
        &active_core::envelope::CheckContext {
            held: CapabilitySet::all(),
            granted: set(&["evaluate"]),
            budget: &active_core::eval::Budget::default(),
            profile: None,
            material: None,
            trust: None,
        },
    )
    .expect_err("by mismatch");
    assert_eq!(error.code(), "E042");
}

#[test]
fn a_failed_receipt_may_omit_the_attenuation_link() {
    // Review M2: the spec requires the link for a clean outcome only.
    let fields = fields(&["evaluate"], None);
    let receipt = match fields.get("receipt") {
        Some(active_core::Value::Table(table)) => table,
        _ => unreachable!(),
    };
    let budget = active_core::eval::Budget::default();
    envelope::check_attenuation(
        &fields,
        receipt,
        "failed",
        &active_core::envelope::CheckContext {
            held: CapabilitySet::all(),
            granted: set(&["evaluate"]),
            budget: &budget,
            profile: None,
            material: None,
            trust: None,
        },
    )
    .expect("failed receipt without attenuation");
    let error = envelope::check_attenuation(
        &fields,
        receipt,
        "clean",
        &active_core::envelope::CheckContext {
            held: CapabilitySet::all(),
            granted: set(&["evaluate"]),
            budget: &budget,
            profile: None,
            material: None,
            trust: None,
        },
    )
    .expect_err("clean receipt requires attenuation");
    assert_eq!(error.code(), "E042");
}

#[test]
fn a_non_table_envelope_is_e033_before_and_after_the_walk() {
    // Pre-eval: the source `envelope` is a non-table literal, caught by the
    // source pre-scan.
    let source = "active_mode = \"data\"\nenvelope = 5\n";
    let module = active_core::parse(source, "non-table").expect("parse");
    let mut engine = Engine::hermetic();
    engine.envelope_gate = Some(active_core::EnvelopeGate::shipped());
    let error = engine.evaluate(&module).expect_err("non-table envelope");
    assert_eq!(error.code(), "E033");

    // Post-eval: the source carries no envelope (the pre-scan sees none and
    // passes), and a profile injects a non-table `envelope`. The evaluated
    // tree is checked again and must refuse with the same code, path, and
    // wording (review M6b).
    let injected =
        "active_mode = \"data\"\nactive_profile = \"prod\"\nprofile \"prod\" { envelope = 42 }\nx = 1\n";
    let module = active_core::parse(injected, "injected-non-table").expect("parse");
    let error = Engine::hermetic()
        .evaluate(&module)
        .expect_err("profile-injected non-table envelope");
    let active_core::EvalError::ValidationFailed { diagnostics } = &error else {
        panic!("expected a validation failure, got {error:?}");
    };
    assert_eq!(diagnostics[0].code, "E033", "{diagnostics:?}");
    assert_eq!(diagnostics[0].path.as_deref(), Some("envelope"));
    assert!(
        diagnostics[0].message.contains("must be a table"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_strict_gate_requires_capabilities_on_v2() {
    // Review M7: omission and stripping are both refused under policy.
    let source = r#"active_mode = "data"
envelope = { version = "2.0.0", profile = "agent", tainted = false, payload = null,
             receipt = { outcome = "failed", failure_class = "other" } }
"#;
    let module = active_core::parse(source, "strict-gate").expect("parse");
    let mut engine = Engine::hermetic();
    engine.envelope_gate = Some(active_core::EnvelopeGate::shipped().with_required_capabilities());
    let error = engine.evaluate(&module).expect_err("missing capabilities");
    assert_eq!(error.code(), "E042");
}

#[test]
fn malformed_capabilities_are_e042_after_the_walk_too() {
    // Review M4: shape defects keep one code on both sides.
    let mut fields = fields(&["evaluate"], None);
    fields.insert(
        "capabilities".into(),
        active_core::Value::String("evaluate".into()),
    );
    let receipt = match fields.get("receipt") {
        Some(active_core::Value::Table(table)) => table,
        _ => unreachable!(),
    };
    let budget = active_core::eval::Budget::default();
    let error = envelope::check_attenuation(
        &fields,
        receipt,
        "failed",
        &active_core::envelope::CheckContext {
            held: CapabilitySet::all(),
            granted: set(&["evaluate"]),
            budget: &budget,
            profile: None,
            material: None,
            trust: None,
        },
    )
    .expect_err("malformed caps");
    assert_eq!(error.code(), "E042");
}

#[test]
fn attenuation_by_failures_share_one_message() {
    // Review H7: a material-looking `by` must not be distinguishable
    // from a wrong `by`.
    let all = ["evaluate", "read-secrets", "inspect", "apply", "sign"];
    let messages = |by: &str| {
        let map = fields(&["evaluate"], Some(attenuation(&all, &["evaluate"], by)));
        let receipt = match map.get("receipt") {
            Some(active_core::Value::Table(table)) => table,
            _ => unreachable!(),
        };
        let budget = active_core::eval::Budget::default();
        let error = envelope::check_attenuation(
            &map,
            receipt,
            "clean",
            &active_core::envelope::CheckContext {
                held: CapabilitySet::all(),
                granted: set(&["evaluate"]),
                budget: &budget,
                profile: None,
                material: None,
                trust: None,
            },
        )
        .expect_err("by mismatch");
        match error {
            active_core::EvalError::ValidationFailed { diagnostics } => diagnostics
                .into_iter()
                .map(|diagnostic| (diagnostic.code, diagnostic.message))
                .collect::<Vec<_>>(),
            other => vec![(other.code().to_string(), format!("{other}"))],
        }
    };
    assert_eq!(messages("agent:other"), messages("bad\nby"));
}

#[test]
fn a_material_by_shares_the_wrong_by_message() {
    // Review H7: `by` is attacker-controlled, so a material value must be
    // indistinguishable from a wrong one. The material clause is reachable
    // only once the equality check passes; the test above uses
    // `material: None`, which never drives it.
    let module =
        active_core::parse("x = secret(\"relay-by-literal\")\n", "material-by").expect("parse");
    let engine = Engine::hermetic();
    let (_value, _diags, seeds) = engine
        .evaluate_with_seeds(&module)
        .expect("the secret module evaluates");
    let profile = SecretProfile::for_engine(&module, &engine, &seeds);
    assert!(
        profile.is_material_text("relay-by-literal"),
        "the literal must be material for this test to drive the branch"
    );

    let all = ["evaluate", "read-secrets", "inspect", "apply", "sign"];
    let messages = |by: &str, sender: Option<&str>, material: Option<&SecretProfile>| {
        let mut map = fields(&["evaluate"], Some(attenuation(&all, &["evaluate"], by)));
        if let Some(id) = sender {
            let mut record = IndexMap::new();
            record.insert("id".into(), active_core::Value::String(id.into()));
            map.insert("sender".into(), active_core::Value::Table(record));
        }
        let receipt = match map.get("receipt") {
            Some(active_core::Value::Table(table)) => table,
            _ => unreachable!(),
        };
        let budget = active_core::eval::Budget::default();
        envelope::check_attenuation(
            &map,
            receipt,
            "clean",
            &active_core::envelope::CheckContext {
                held: CapabilitySet::all(),
                granted: set(&["evaluate"]),
                budget: &budget,
                profile: None,
                material,
                trust: None,
            },
        )
        .map_err(|error| match error {
            active_core::EvalError::ValidationFailed { diagnostics } => diagnostics
                .into_iter()
                .map(|diagnostic| (diagnostic.code, diagnostic.message))
                .collect::<Vec<_>>(),
            other => vec![(other.code().to_string(), format!("{other}"))],
        })
    };

    // The equality check passes (by == sender.id), so only the material
    // clause can refuse, and its message must equal the plain wrong-`by`
    // message exactly.
    let material_refusal = messages("relay-by-literal", Some("relay-by-literal"), Some(&profile));
    let wrong_by = messages("agent:other", Some("agent:me"), Some(&profile));
    assert_eq!(
        material_refusal, wrong_by,
        "a material `by` must share the wrong-`by` message"
    );
    assert_eq!(material_refusal.unwrap_err()[0].0, "E042");

    // Negative control for the branch itself: without a profile the same
    // record is accepted, so the material clause is what refused.
    assert!(messages("relay-by-literal", Some("relay-by-literal"), None).is_ok());
}
