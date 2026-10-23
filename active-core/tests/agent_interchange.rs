//! Agent interchange + tool-result envelope conformance
//! (spec/agent-interchange-profile.md, spec/tool-result-envelope.md).

use std::path::Path;

use active_core::{parse, parse_file, Engine, ParseOptions};

fn example() -> active_core::Module {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/agent-interchange/tool-result.active");
    parse_file(&path, ParseOptions::default()).expect("example parses")
}

#[test]
fn example_envelope_is_valid_and_capabilities_are_offered() {
    let module = example();
    assert_eq!(module.mode, active_core::Mode::Data);
    let (value, diagnostics) = Engine::new().evaluate(&module).expect("evaluates");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let envelope = match &value {
        active_core::Value::Table(map) => map.get("envelope").cloned().expect("envelope"),
        other => panic!("root must be a table, got {other:?}"),
    };
    let active_core::Value::Table(fields) = envelope else {
        panic!("envelope must be a table");
    };
    assert_eq!(
        fields.get("version").and_then(|v| v.as_str()),
        Some("1.0.0")
    );
    assert_eq!(
        fields.get("capability").and_then(|v| v.as_str()),
        Some("agent")
    );
    assert!(matches!(
        fields.get("tainted"),
        Some(active_core::Value::Bool(false))
    ));
    assert!(fields.contains_key("payload"));
    assert!(fields.contains_key("budget"));

    // Handshake: the agent profile offers every declared capability.
    let profile = active_core::convert::sandbox_profile("agent").expect("profile");
    let refused = active_core::convert::refused_capabilities(&module.capabilities, profile.tools);
    assert!(refused.is_empty(), "refused: {refused:?}");
}

#[test]
fn secret_bearing_envelope_redacts_on_export() {
    let src = "active_mode = \"data\"\nenvelope = { version = \"1.0.0\", tainted = true, payload = { token = secret(\"hunter2-envelope\") } }\n";
    let module = parse(src, "envelope").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("***SECRET***"), "{json}");
    assert!(
        !json.contains("hunter2"),
        "raw secret in envelope export: {json}"
    );
}

#[test]
fn config_mode_data_constructs_trip_the_marker_rule() {
    // The profile requires an explicit Data Mode marker; lint L010 is
    // the sender-side gate for Data constructs without one.
    let src = "id = blob(\"text/plain\", \"hi\")\n";
    let module = parse(src, "config").expect("parse");
    assert_eq!(module.mode, active_core::Mode::Config);
    let codes: Vec<String> = active_core::lint(&module)
        .into_iter()
        .map(|d| d.code)
        .collect();
    assert!(codes.iter().any(|c| c == "L010"), "{codes:?}");
}
