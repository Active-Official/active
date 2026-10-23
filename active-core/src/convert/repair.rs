//! Multi-step goal-directed repair (PROPOSAL §2.1, CHECKLIST Phase R).
//!
//! Turns the suggester's mechanical directives into a closed loop:
//! plan (ordered safe steps with side effects and skip reasons) →
//! apply (directives prepended to the source, re-parsed and
//! re-evaluated through the real engine) → re-plan until clean,
//! stuck, or out of steps. Stuck (same codes twice) and terminal
//! unrepairable states are first-class H006/H008 diagnostics, so
//! agent loops always know when to stop.

use serde::{Deserialize, Serialize};

use crate::diag::{Diagnostic, RepairGoal, Severity};
use crate::{parse, Engine};

/// Status of one plan step after the run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Applied,
    Skipped { reason: String },
}

/// One step of a repair plan: the directive, its safety, what happened,
/// and the blast radius.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedStep {
    pub kind: String,
    pub path: String,
    pub directive: String,
    pub safe: bool,
    pub status: StepStatus,
    pub side_effects: Vec<String>,
}

/// Outcome of a repair run: the ordered plan, the repaired source
/// (original with applied directives prepended), and the terminal
/// state. `clean` means no error- or warning-severity diagnostic
/// remains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairOutcome {
    pub goal: RepairGoal,
    pub steps: Vec<PlannedStep>,
    pub clean: bool,
    pub stuck: bool,
    pub unrepairable: bool,
    pub final_codes: Vec<String>,
    pub findings: Vec<Diagnostic>,
    pub repaired_source: String,
}

fn is_blocking(d: &Diagnostic) -> bool {
    matches!(d.severity, Severity::Error | Severity::Warning)
}

fn sorted_codes(diags: &[Diagnostic]) -> Vec<String> {
    // (code, path) pairs, not bare codes: a fill that fixes one path
    // while another path still fails the same code IS progress, and
    // must not read as stuck.
    let mut codes: Vec<String> = diags
        .iter()
        .filter(|d| is_blocking(d))
        .map(|d| format!("{}@{}", d.code, d.path.as_deref().unwrap_or("")))
        .collect();
    codes.sort();
    codes.dedup();
    codes
}

fn stuck_finding() -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        code: "H006".to_string(),
        message:
            "repair stuck: same diagnostics after another attempt; stopping instead of looping"
                .to_string(),
        path: None,
        span: None,
        repair: None,
        confidence: 1.0,
        file: None,
    }
}

fn unrepairable_finding(detail: &str) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        code: "H008".to_string(),
        message: format!("unrepairable: {detail}"),
        path: None,
        span: None,
        repair: None,
        confidence: 1.0,
        file: None,
    }
}

/// Run the repair loop over `source` toward `goal`.
///
/// * `preserve` lists dotted paths the loop must never touch: steps
///   naming them are recorded as skipped with the reason.
/// * `max_steps` bounds applied directives; 0 plans without applying.
/// * Directives apply by prepending to the source and re-parsing, so
///   the shipped `@patch`/`@rename` machinery is the application
///   engine — never a parallel re-implementation.
pub fn repair(
    source: &str,
    goal: RepairGoal,
    preserve: &[String],
    max_steps: usize,
) -> RepairOutcome {
    let mut steps: Vec<PlannedStep> = Vec::new();
    let mut applied: Vec<String> = Vec::new();
    let mut prev_codes: Vec<String> = Vec::new();
    let mut stuck = false;
    let mut unrepairable = false;
    let mut findings: Vec<Diagnostic> = Vec::new();
    let mut final_codes: Vec<String> = Vec::new();
    let mut clean = false;

    let mut current = source.to_string();
    for _ in 0..=max_steps {
        let module = match parse(&current, "repair") {
            Ok(m) => m,
            Err(_) => {
                unrepairable = true;
                findings.push(unrepairable_finding(
                    "source does not parse; no plan can start",
                ));
                break;
            }
        };
        // Strict evaluation: the planner must see the raw missing-key
        // and typo diagnostics with their repair payloads, not the
        // resilient engine's already-applied fixes.
        let mut engine = Engine::new();
        engine.resilient = false;
        let diags = match engine.evaluate(&module) {
            Ok((_, diags)) => diags,
            // Validation failures still yield their diagnostics: those
            // ARE the planning input. Only non-validation fatal errors
            // (cycles, budgets, leaks, unparseable trees) are terminal.
            Err(crate::eval::EvalError::ValidationFailed { diagnostics }) => diagnostics,
            Err(e) => {
                unrepairable = true;
                findings.push(unrepairable_finding(&format!(
                    "evaluation fails fatally ({}); no directive can fix it",
                    e.code()
                )));
                final_codes = vec![e.code().to_string()];
                break;
            }
        };
        let codes = sorted_codes(&diags);
        if codes.is_empty() {
            clean = true;
            final_codes = codes;
            break;
        }
        // Consecutive repeat after an application means the applied
        // directive changed nothing: with the current Fill/Rename-only
        // suggester every effective application strictly shrinks the
        // (code, path) set, so a repeat is a void application — stop
        // instead of looping (H006). Reaching this iteration at all
        // implies the previous one applied something (all other
        // no-progress paths break), so equality here always follows
        // an application.
        let repeat = codes == prev_codes;
        prev_codes = codes.clone();

        let suggestions = crate::convert::suggest_patches_structured(&module, &diags);
        // Goal ordering: MinimizeDiff prefers fills (purely additive)
        // over renames; every goal skips preserved paths and
        // already-applied directives.
        let mut ordered = suggestions;
        if goal == RepairGoal::MinimizeDiff {
            ordered.sort_by_key(|s| {
                if s.kind == crate::convert::PatchKind::Fill {
                    0u8
                } else {
                    1u8
                }
            });
        }
        let mut progressed = false;
        if applied.len() < max_steps {
            for s in &ordered {
                let directive = s.directive.clone();
                if applied.contains(&directive) {
                    continue;
                }
                if preserve.iter().any(|p| p == &s.path) {
                    steps.push(PlannedStep {
                        kind: format!("{:?}", s.kind).to_lowercase(),
                        path: s.path.clone(),
                        directive,
                        safe: false,
                        status: StepStatus::Skipped {
                            reason: "preserved path: goal forbids touching it".to_string(),
                        },
                        side_effects: vec![s.path.clone()],
                    });
                    continue;
                }
                steps.push(PlannedStep {
                    kind: format!("{:?}", s.kind).to_lowercase(),
                    path: s.path.clone(),
                    directive: directive.clone(),
                    safe: true,
                    status: StepStatus::Applied,
                    side_effects: vec![s.path.clone()],
                });
                applied.push(directive);
                progressed = true;
                // One directive per iteration: re-evaluate after every
                // change so stuck detection sees the true trajectory.
                break;
            }
        }
        if !progressed {
            if repeat {
                // Applied-but-void iteration (or re-suggested applied
                // work): the trajectory is not converging — stop
                // instead of looping (H006). Reaching an iteration at
                // all implies the previous one applied something (every
                // other no-progress path breaks), so a repeat here
                // always follows an application.
                stuck = true;
                findings.push(stuck_finding());
                final_codes = codes;
                break;
            }
            if applied.len() >= max_steps {
                // Budget exhausted with work remaining: stop without
                // blaming the document (neither stuck nor unrepairable).
                final_codes = codes;
                break;
            }
            // Remaining blocking diagnostics with no safe mapping
            // (bare T003, cycles, parse-fatals): terminal.
            unrepairable = true;
            findings.push(unrepairable_finding(
                "remaining diagnostics have no safe repair mapping",
            ));
            final_codes = codes;
            break;
        }
        // Rebuild the candidate: applied directives prepended to the
        // ORIGINAL source, in application order (deterministic).
        let mut next = String::new();
        for d in &applied {
            next.push_str(d);
            next.push('\n');
        }
        next.push_str(source);
        current = next;
    }

    RepairOutcome {
        goal,
        steps,
        clean,
        stuck,
        unrepairable,
        final_codes,
        findings,
        repaired_source: current,
    }
}
