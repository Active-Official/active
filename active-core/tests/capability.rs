//! Capability declarations, sandbox profiles, and the negotiation
//! handshake (PROPOSAL §3.5, CHECKLIST CAP.001–CAP.006).
//!
//! Drives the real paths: root `active_capabilities` parsing (E025 on
//! bad shapes, include markers ignored), `refused()` set difference,
//! and code↔descriptor profile parity.

use active_core::convert::{refused_capabilities, sandbox_profiles};
use active_core::parse;

#[test]
fn capabilities_parse_from_root_marker() {
    let module = parse(
        "active_capabilities = [\"eval\", \"inspect\"]\nport = 1\n",
        "cap",
    )
    .expect("must parse");
    assert_eq!(
        module.capabilities,
        vec!["eval".to_string(), "inspect".to_string()]
    );
}

#[test]
fn capabilities_require_string_arrays() {
    for src in [
        "active_capabilities = \"eval\"\n",
        "active_capabilities = [\"eval\", 42]\n",
    ] {
        let (_, errors) =
            active_core::parse_recovering(src, "cap", active_core::ParseOptions::strict());
        assert!(
            errors.iter().any(|d| d.code == "E025"),
            "{src} must be E025: {errors:?}"
        );
    }
}

#[test]
fn refused_is_set_difference_in_order() {
    let declared = vec![
        "eval".to_string(),
        "teleport".to_string(),
        "eval".to_string(),
    ];
    let refused = refused_capabilities(&declared, &["check", "eval"]);
    assert_eq!(refused, vec!["teleport"]);
    assert!(refused_capabilities(&["eval".to_string()], &["eval"]).is_empty());
}

#[test]
fn included_markers_neither_set_nor_error() {
    // Includes cannot set capabilities: a merged marker is ignored
    // entirely (no E025), and the outer document keeps its own list.
    // The `if self.in_include` guard mirrors resolve_mode (defense in
    // depth); the file-identity check does the real work, proven here.
    let dir = std::env::temp_dir().join("active-cap-include");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("inc.active"),
        "active_capabilities = [\"teleport\"]\nx = 1\n",
    )
    .expect("write inc");
    let main = dir.join("main.active");
    std::fs::write(
        &main,
        "include \"inc.active\"\nactive_capabilities = [\"eval\"]\nport = 1\n",
    )
    .expect("write main");
    let module = active_core::parse_file(&main, active_core::ParseOptions::default())
        .unwrap_or_else(|diags| panic!("must parse: {diags:?}"));
    assert_eq!(module.capabilities, vec!["eval".to_string()]);
}

#[test]
fn code_profiles_match_descriptor() {
    // Single contract, two renderings: the code profiles and
    // agent-kit/capability.schema.json must agree exactly.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../agent-kit/capability.schema.json");
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read capability schema"))
            .expect("schema parses");
    let profiles = doc["profiles"].as_object().expect("profiles object");
    let mut coded: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for p in sandbox_profiles() {
        coded.push((p.name, p.budget, p.tools.to_vec()));
    }
    assert_eq!(coded.len(), profiles.len(), "same profile count");
    for (name, budget, tools) in coded {
        let entry = profiles
            .get(name)
            .unwrap_or_else(|| panic!("descriptor lacks profile {name}"));
        assert_eq!(entry["budget"].as_str(), Some(budget), "{name}");
        let mut want: Vec<&str> = tools;
        want.sort_unstable();
        let mut have: Vec<&str> = entry["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} lacks tools"))
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        have.sort_unstable();
        assert_eq!(have, want, "profile {name} tools match");
    }
    // Every sandboxed tool is an advertised descriptor tool.
    let names: Vec<&str> = doc["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for p in sandbox_profiles() {
        for t in p.tools {
            assert!(names.contains(t), "profile tool {t} must be advertised");
        }
    }
}
