//! Schema compatibility reports (PROPOSAL §2.4, CHECKLIST SC.006–SC.007).
//!
//! Drives `convert::compare_schemas` on parsed document pairs and
//! asserts the documented C04x/E040 outcomes — the SC.010 vector
//! layer for evolution, compatibility, and the new diagnostic family.

use active_core::convert::compat::{code, compare};
use active_core::parse;

fn compare_docs(old_src: &str, new_src: &str) -> Vec<active_core::convert::CompatFinding> {
    let old = parse(old_src, "compat-old").expect("old must parse");
    let new = parse(new_src, "compat-new").expect("new must parse");
    compare(&old, &new).expect("compare must run")
}

fn codes(findings: &[active_core::convert::CompatFinding]) -> Vec<(&str, &str)> {
    findings
        .iter()
        .map(|f| (f.code.as_str(), f.severity.as_str()))
        .collect()
}

#[test]
fn identical_schemas_are_compatible() {
    let src = "schema = { host = String, port = Int }\nhost = \"x\"\nport = 1\n";
    let findings = compare_docs(src, src);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn added_required_field_is_breaking_c040() {
    let old = "schema = { host = String }\nhost = \"x\"\n";
    let new = "schema = { host = String, port = Int }\nhost = \"x\"\nport = 1\n";
    let findings = compare_docs(old, new);
    assert_eq!(codes(&findings), vec![(code::ADDED_REQUIRED, "error")]);
}

#[test]
fn added_defaulted_field_is_info() {
    let old = "schema = { host = String }\nhost = \"x\"\n";
    let new = "schema = { host = String, port = Int(default = 8080) }\nhost = \"x\"\n";
    let findings = compare_docs(old, new);
    assert_eq!(codes(&findings), vec![(code::ADDED_REQUIRED, "info")]);
}

#[test]
fn removed_field_is_breaking_c041_unless_scheduled() {
    let old = "schema = { host = String, port = Int }\nhost = \"x\"\nport = 1\n";
    let bare_new = "schema = { host = String }\nhost = \"x\"\n";
    let findings = compare_docs(old, bare_new);
    assert_eq!(codes(&findings), vec![(code::REMOVED_FIELD, "error")]);

    // Scheduled removal: old marks removed_in, new version reaches it.
    let old_sched = "schema = { version = \"1.0.0\", host = String, port = Int(removed_in = \"1.1.0\") }\nhost = \"x\"\nport = 1\n";
    let new_sched = "schema = { version = \"1.1.0\", host = String }\nhost = \"x\"\n";
    let findings = compare_docs(old_sched, new_sched);
    assert_eq!(codes(&findings), vec![(code::REMOVED_FIELD, "info")]);
}

#[test]
fn type_change_is_breaking_c042() {
    let old = "schema = { port = Int }\nport = 1\n";
    let new = "schema = { port = String }\nport = \"1\"\n";
    let findings = compare_docs(old, new);
    assert_eq!(codes(&findings), vec![(code::TYPE_CHANGED, "error")]);
    assert!(
        findings[0].message.contains("Int to String"),
        "{findings:?}"
    );
}

#[test]
fn blob_tightening_is_breaking_c042() {
    let old = "schema = { f = Blob(max_size = 100) }\nf = blob(\"text/plain\", \"hello\")\n";
    let tightened = "schema = { f = Blob(max_size = 2) }\nf = blob(\"text/plain\", \"hello\")\n";
    let findings = compare_docs(old, tightened);
    assert_eq!(codes(&findings), vec![(code::TYPE_CHANGED, "error")]);
    assert!(findings[0].message.contains("max_size"), "{findings:?}");

    let remedia = "schema = { f = Blob(max_size = 100, media_type = \"image/png\") }\nf = blob(\"text/plain\", \"hello\")\n";
    let findings = compare_docs(old, remedia);
    assert_eq!(codes(&findings), vec![(code::TYPE_CHANGED, "error")]);
    assert!(findings[0].message.contains("media type"), "{findings:?}");
}

#[test]
fn version_regression_and_reader_floor_are_breaking() {
    let old = "schema = { version = \"1.2.0\", host = String }\nhost = \"x\"\n";
    let regressed = "schema = { version = \"1.1.0\", host = String }\nhost = \"x\"\n";
    let findings = compare_docs(old, regressed);
    assert_eq!(codes(&findings), vec![(code::VERSION_REGRESSED, "error")]);

    let raised =
        "schema = { version = \"1.2.0\", min_reader = \"1.3.0\", host = String }\nhost = \"x\"\n";
    let findings = compare_docs(old, raised);
    assert_eq!(codes(&findings), vec![(code::READER_FLOOR_RAISED, "error")]);

    let raised_w =
        "schema = { version = \"1.2.0\", min_writer = \"1.3.0\", host = String }\nhost = \"x\"\n";
    let findings = compare_docs(old, raised_w);
    assert_eq!(codes(&findings), vec![(code::READER_FLOOR_RAISED, "error")]);
    assert_eq!(findings[0].path, "schema.min_writer");
}

#[test]
fn breaking_findings_gate_the_cli_with_e040() {
    // The CLI contract: any error-severity finding fails closed.
    let old = "schema = { host = String }\nhost = \"x\"\n";
    let new = "schema = { host = String, port = Int }\nhost = \"x\"\nport = 1\n";
    let findings = compare_docs(old, new);
    assert!(findings.iter().any(|f| f.is_breaking()));
    // E040 is the summary code the CLI reports with the failure.
    assert_eq!(code::BREAKING_SUMMARY, "E040");
}

#[test]
fn malformed_versions_are_schema_bugs_not_silent() {
    let (_, errors) = active_core::parse_recovering(
        "schema = { version = \"nope\", host = String }\nhost = \"x\"\n",
        "compat",
        active_core::ParseOptions::strict(),
    );
    assert!(errors.iter().any(|d| d.code == "E020"), "{errors:?}");
    let (_, errors) = active_core::parse_recovering(
        "schema = { port = Int(removed_in = \"soon\") }\nport = 1\n",
        "compat",
        active_core::ParseOptions::strict(),
    );
    assert!(errors.iter().any(|d| d.code == "E020"), "{errors:?}");
}
