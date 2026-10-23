//! Multi-step goal-directed repair (PROPOSAL §2.1, CHECKLIST R.001–R.008).
//!
//! Drives the real loop (`convert::repair_document`) on the real path:
//! parse → strict eval → suggester → directive prepend → re-parse.
//! Terminal states (clean, stuck H006, unrepairable H008, step budget)
//! are asserted through the shipped entry points, never re-implemented.

use active_core::convert::{repair_document, StepStatus};
use active_core::diag::RepairGoal;

fn validate(src: &str) -> active_core::convert::RepairOutcome {
    repair_document(src, RepairGoal::Validate, &[], 8)
}

#[test]
fn fill_then_rename_reaches_clean() {
    // The canonical two-step loop: fill the defaulted key, rename the
    // typo, re-evaluate to clean. Directives apply in diagnostic order.
    let src =
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n";
    let out = validate(src);
    assert!(out.clean, "{out:?}");
    assert!(!out.stuck && !out.unrepairable);
    assert_eq!(out.steps.len(), 2);
    assert!(out
        .steps
        .iter()
        .all(|s| matches!(s.status, StepStatus::Applied)));
    assert!(out.steps.iter().all(|s| s.safe));
    // The repaired source re-evaluates clean strictly (no theater:
    // the loop's own output feeds a fresh strict evaluation).
    let module = active_core::parse(&out.repaired_source, "repair").expect("repaired parses");
    let mut engine = active_core::Engine::new();
    engine.resilient = false;
    let (_, diags) = engine.evaluate(&module).expect("repaired evaluates");
    assert!(
        diags
            .iter()
            .all(|d| d.severity == active_core::diag::Severity::Info
                || d.severity == active_core::diag::Severity::Hint),
        "{diags:?}"
    );
}

#[test]
fn minimize_diff_prefers_additive_fills() {
    // Same document under MinimizeDiff: the fill (purely additive)
    // leads, the rename follows.
    let src =
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n";
    let out = repair_document(src, RepairGoal::MinimizeDiff, &[], 8);
    assert!(out.clean, "{out:?}");
    assert_eq!(out.steps[0].kind, "fill");
    assert_eq!(out.steps[1].kind, "rename");
}

#[test]
fn bare_type_error_is_unrepairable_h008() {
    // T003 has no directive form: the loop must stop and say so.
    let src = "schema = { contact = String(format = \"email\") }\ncontact = \"not-an-email\"\n";
    let out = validate(src);
    assert!(!out.clean && out.unrepairable && !out.stuck);
    assert!(out.findings.iter().any(|d| d.code == "H008"), "{out:?}");
    assert!(out.final_codes.iter().any(|c| c.starts_with("T003@")));
}

#[test]
fn colliding_rename_is_terminal_not_silent() {
    // Rename target already exists: the patch engine fails closed
    // (E019) instead of overwriting. Fatal codes carry no path.
    let src = "schema = { closed = true, host = String }\nhost = \"a\"\nHost = \"x\"\n";
    let out = validate(src);
    assert!(!out.clean && out.unrepairable);
    assert!(out.final_codes.iter().any(|c| c == "E019"));
}

#[test]
fn preserved_paths_are_skipped_not_touched() {
    // PreserveKey goal: the loop must not touch preserved paths even
    // when that leaves the document unclean.
    let src =
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n";
    let out = repair_document(
        src,
        RepairGoal::PreserveKey("port,host".to_string()),
        &["port".to_string(), "host".to_string(), "Host".to_string()],
        8,
    );
    assert!(!out.clean && out.unrepairable);
    assert!(out
        .steps
        .iter()
        .all(|s| matches!(s.status, StepStatus::Skipped { .. })));
    assert!(!out.repaired_source.contains("@patch"));
    assert!(!out.repaired_source.contains("@rename"));
}

#[test]
fn step_budget_stops_without_blame() {
    // One step allowed for a two-step fix: the loop stops, reports the
    // remaining codes, and flags neither stuck nor unrepairable.
    let src =
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n";
    let out = repair_document(src, RepairGoal::Validate, &[], 1);
    assert!(!out.clean && !out.stuck && !out.unrepairable);
    assert_eq!(out.steps.len(), 1);
    assert!(!out.final_codes.is_empty());
}

#[test]
fn unparseable_source_is_unrepairable() {
    let out = validate("port = \n");
    assert!(!out.clean && out.unrepairable);
    assert!(out.findings.iter().any(|d| d.code == "H008"));
}

#[test]
fn clean_document_plans_nothing() {
    let out = validate("port = 8080\n");
    assert!(out.clean && out.steps.is_empty());
    assert_eq!(out.repaired_source, "port = 8080\n");
}
