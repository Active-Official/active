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
        RepairGoal::PreserveKey(vec!["port".to_string(), "host".to_string()]),
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
fn preserve_protects_descendants_and_ancestors() {
    // Preserve is a subtree contract, not an exact-path list: a fill on
    // `server.port` is blocked by `--preserve server`.
    let src = "schema = { server = Table { port = Int(default = 8080) } }\nserver = {}\n";
    let out = repair_document(src, RepairGoal::Validate, &["server".to_string()], 8);
    assert!(
        !out.repaired_source.contains("@patch(\"server.port\""),
        "{out:?}"
    );
    assert!(
        out.steps
            .iter()
            .any(|step| matches!(step.status, StepStatus::Skipped { .. })),
        "{out:?}"
    );
    // A preserved child also blocks touching its parent.
    let src = "schema = { a = Table { x = Int(default = 1) } }\nb = 1\n";
    let out = repair_document(src, RepairGoal::Validate, &["a.x".to_string()], 8);
    assert!(!out.repaired_source.contains("@patch(\"a\""), "{out:?}");
}

#[test]
fn preserve_goal_unions_with_explicit_preserve_list() {
    // Goal-carried paths and the explicit list protect identically: a
    // step touching either is skipped, never applied.
    let src =
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n";
    let out = repair_document(
        src,
        RepairGoal::PreserveKey(vec!["port".to_string()]),
        &["Host".to_string()],
        8,
    );
    assert!(!out.clean);
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
fn large_typo_doc_terminates_at_a_fixpoint() {
    // Ten typos plus ten missing keys: the loop must terminate with a
    // bounded number of evals, reach clean, and fixpoint (a second run
    // plans nothing). This is the termination property the H006
    // history net guarantees.
    let mut src = String::from("schema = { closed = true");
    for i in 0..10 {
        src.push_str(&format!(", k{i} = String, p{i} = Int(default = {i})"));
    }
    src.push_str(" }\n");
    for i in 0..10 {
        src.push_str(&format!("K{i} = \"x\"\n"));
    }
    let out = repair_document(&src, RepairGoal::Validate, &[], 64);
    assert!(out.clean && !out.stuck, "{out:?}");
    assert_eq!(out.steps.len(), 20, "{out:?}");
    let again = repair_document(&out.repaired_source, RepairGoal::Validate, &[], 64);
    assert!(again.clean && again.steps.is_empty());
}

#[test]
fn clean_document_plans_nothing() {
    let out = validate("port = 8080\n");
    assert!(out.clean && out.steps.is_empty());
    assert_eq!(out.repaired_source, "port = 8080\n");
}

// ------------------------------------------------------------------
// Browser model and partial application (TOOLING REPAIR-UI)
// ------------------------------------------------------------------

#[test]
fn browser_model_pairs_diagnostics_and_plan() {
    let src = "schema = { port = Int(max = 10) }\nport = 999\nunexpected = 1\n";
    let model = active_core::convert::repair::browser_model(
        src,
        active_core::diag::RepairGoal::Validate,
        &[],
        8,
    );
    assert!(model["plan"]["steps"].is_array());
    let diagnostics = model["diagnostics"].as_array().expect("diagnostics");
    assert!(diagnostics.iter().any(|d| d["code"] == "T003"), "{model}");
    // Secret material never enters the model; plain values may appear
    // in schema messages (they are authored data, not secrets).
    let secret_src = "schema = { port = Int }\nport = secret(\"hunter2-browser\")\n";
    let secret_model = active_core::convert::repair::browser_model(
        secret_src,
        active_core::diag::RepairGoal::Validate,
        &[],
        8,
    );
    assert!(
        !secret_model.to_string().contains("hunter2"),
        "raw secret in browser model: {secret_model}"
    );
}

#[test]
fn apply_selected_honors_index_and_prefix() {
    let src = "schema = { port = Int(default = 8080), host = String(default = \"x\") }\n";
    let outcome =
        active_core::convert::repair::repair(src, active_core::diag::RepairGoal::Validate, &[], 8);
    assert!(outcome.steps.len() >= 2, "{outcome:?}");
    let (first, applied) =
        active_core::convert::repair::apply_selected(src, &outcome.steps, Some(0), None);
    assert_eq!(applied.len(), 1);
    assert!(first.starts_with(&applied[0]));
    let (only_host, applied) =
        active_core::convert::repair::apply_selected(src, &outcome.steps, None, Some("host"));
    assert_eq!(applied.len(), 1);
    assert!(applied[0].contains("host"), "{applied:?}");
    assert!(only_host.contains("host"));
    let (none, applied) =
        active_core::convert::repair::apply_selected(src, &outcome.steps, None, Some("nope"));
    assert!(applied.is_empty());
    assert_eq!(none, src);
}

#[test]
fn repair_keeps_a_leading_bom_at_byte_zero() {
    let src = "\u{feff}schema = { port = Int(default = 8080) }\n";
    let outcome =
        active_core::convert::repair::repair(src, active_core::diag::RepairGoal::Validate, &[], 8);
    assert!(outcome.clean, "{outcome:?}");
    assert!(
        outcome.repaired_source.starts_with('\u{feff}'),
        "BOM moved: {:?}",
        outcome.repaired_source
    );
    assert!(outcome.repaired_source.contains("@patch(\"port\", 8080)"));
}
