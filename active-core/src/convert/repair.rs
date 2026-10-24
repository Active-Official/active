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
/// remains. `final_codes` carries `(code, path)` pairs for
/// validation-level outcomes (`T004@port`) and bare codes for fatal
/// loop-terminal errors (`E019`, parse codes) that never had a
/// value-level path — consumers match on the code prefix, not the
/// shape. `stuck` fires on any repeat over the full code-set history
/// (consecutive repeat or A→B→A oscillation) after applied steps.
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
    let mut engine = Engine::new();
    engine.resilient = false;
    repair_with_engine(source, goal, preserve, max_steps, &engine, None)
}

/// Repair loop with caller-provided engine and optional file context.
///
/// * `engine` carries budget, profile override, and (like every other
///   tool path) strictness forced off here: the planner must see the
///   raw diagnostics, not resiliently healed ones. The materialise
///   flag and environment snapshot ride along from the caller.
/// * `file_ctx`: when set, sources parse with that file's include
///   context (includes resolve and merge); otherwise parsing stays
///   hermetic (no filesystem reads).
pub fn repair_with_engine(
    source: &str,
    goal: RepairGoal,
    preserve: &[String],
    max_steps: usize,
    engine: &Engine,
    file_ctx: Option<&std::path::Path>,
) -> RepairOutcome {
    repair_with_engine_and_options(source, goal, preserve, max_steps, engine, file_ctx, false)
}

/// Full loop with caller-provided engine, file context, and include
/// policy. `allow_parent_escapes` mirrors every other tool path
/// (E015 otherwise); trust verification stays the caller's gate
/// (checked once on the original file before planning).
#[allow(clippy::too_many_arguments)]
pub fn repair_with_engine_and_options(
    source: &str,
    goal: RepairGoal,
    preserve: &[String],
    max_steps: usize,
    engine: &Engine,
    file_ctx: Option<&std::path::Path>,
    allow_parent_escapes: bool,
) -> RepairOutcome {
    // The goal's own path list unions with the explicit slice: both
    // spellings protect the same paths (a mismatch between them would
    // silently narrow protection).
    let mut protected: Vec<String> = preserve.to_vec();
    if let RepairGoal::PreserveKey(paths) = &goal {
        for p in paths {
            if !protected.contains(p) {
                protected.push(p.clone());
            }
        }
    }
    let mut steps: Vec<PlannedStep> = Vec::new();
    let mut applied: Vec<String> = Vec::new();
    // Full code-set history, not just the previous set: an
    // A→B→A oscillation is stuck just as much as A→A, and the
    // consecutive-only check would evade it.
    let mut seen: std::collections::HashSet<Vec<String>> = std::collections::HashSet::new();
    let mut stuck = false;
    let mut unrepairable = false;
    let mut findings: Vec<Diagnostic> = Vec::new();
    let mut final_codes: Vec<String> = Vec::new();
    let mut clean = false;

    let mut current = source.to_string();
    // The caller's engine minus resilience: budgets, profile override,
    // environment, and materialise flag ride along, but planning
    // evaluation is always strict so the loop sees raw diagnostics.
    let mut strict = engine.clone();
    strict.resilient = false;
    let engine = &strict;
    for _ in 0..=max_steps {
        // Parse errors keep their diagnostics: the first code names
        // the failure (value-free codes only, never messages).
        let parse_diags = |diags: &[Diagnostic]| {
            let mut codes: Vec<String> = diags.iter().map(|d| d.code.clone()).collect();
            codes.sort();
            codes.dedup();
            codes
        };
        let module = match file_ctx {
            Some(path) => {
                let name = path.to_string_lossy().to_string();
                let mut options = crate::ParseOptions::strict();
                options.allow_parent_escapes = allow_parent_escapes;
                match crate::parse_text_with_file(&current, &name, options, path) {
                    Ok(m) => m,
                    Err(diags) => {
                        unrepairable = true;
                        let codes = parse_diags(&diags);
                        findings.push(unrepairable_finding(&format!(
                            "source does not parse (first error {})",
                            codes.first().map(String::as_str).unwrap_or("?")
                        )));
                        final_codes = codes;
                        break;
                    }
                }
            }
            None => match parse(&current, "repair") {
                Ok(m) => m,
                Err(diags) => {
                    unrepairable = true;
                    let codes = parse_diags(&diags);
                    findings.push(unrepairable_finding(&format!(
                        "source does not parse (first error {})",
                        codes.first().map(String::as_str).unwrap_or("?")
                    )));
                    final_codes = codes;
                    break;
                }
            },
        };
        // Strict planning evaluation on the caller's engine (budgets,
        // profile override, and environment ride along; resilient mode
        // stays off): the planner must see the raw missing-key and
        // typo diagnostics with their repair payloads, not already
        // applied fixes.
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
        // Repeat detection over the full history: any previously seen
        // code set means the trajectory is cycling (consecutive repeat
        // or A→B→A oscillation) after applications that changed
        // nothing net — stop instead of looping (H006). Reaching an
        // iteration implies the previous one applied something (every
        // other no-progress path breaks), so a repeat always follows
        // an application.
        if !seen.insert(codes.clone()) {
            stuck = true;
            findings.push(stuck_finding());
            final_codes = codes;
            break;
        }

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
                // Preserve blocks the path AND both rename endpoints: a
                // rename onto a preserved path still touches it.
                let touches_preserved = protected.iter().any(|p| {
                    p == &s.path || s.from.as_ref() == Some(p) || s.to.as_ref() == Some(p)
                });
                if touches_preserved {
                    steps.push(PlannedStep {
                        kind: format!("{:?}", s.kind).to_lowercase(),
                        path: s.path.clone(),
                        directive,
                        safe: false,
                        status: StepStatus::Skipped {
                            reason: "preserved path: goal forbids touching it".to_string(),
                        },
                        side_effects: match (&s.from, &s.to) {
                            (Some(from), Some(to)) => vec![from.clone(), to.clone()],
                            _ => vec![s.path.clone()],
                        },
                    });
                    continue;
                }
                let side_effects = match (&s.from, &s.to) {
                    // Renames name both endpoints as the blast radius;
                    // fills are purely additive (no side effects).
                    (Some(from), Some(to)) => vec![from.clone(), to.clone()],
                    _ => Vec::new(),
                };
                steps.push(PlannedStep {
                    kind: format!("{:?}", s.kind).to_lowercase(),
                    path: s.path.clone(),
                    directive: directive.clone(),
                    safe: true,
                    status: StepStatus::Applied,
                    side_effects,
                });
                applied.push(directive);
                progressed = true;
                // One directive per iteration: re-evaluate after every
                // change so stuck detection sees the true trajectory.
                break;
            }
        }
        if !progressed {
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
        // ORIGINAL source, in application order (deterministic), with a
        // leading BOM kept at byte 0 (directives after it).
        current = prepend_directives(source, &applied);
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

/// Unified browser model (TOOLING REPAIR-UI.001): the repair plan plus
/// the initial diagnostics that justified it. Values never appear;
/// codes, paths, severities, and messages only.
pub fn browser_model(
    source: &str,
    goal: RepairGoal,
    preserve: &[String],
    max_steps: usize,
) -> serde_json::Value {
    let mut engine = Engine::new();
    engine.resilient = false;
    browser_model_with(source, goal, preserve, max_steps, &engine, None, false)
}

/// File-aware browser model: the plan runs with the caller's engine and
/// file context, and diagnostics parse with includes resolved exactly
/// like `active repair` (so includes cannot make the browser disagree
/// with the CLI).
pub fn browser_model_with(
    source: &str,
    goal: RepairGoal,
    preserve: &[String],
    max_steps: usize,
    engine: &Engine,
    file_ctx: Option<&std::path::Path>,
    allow_parent_escapes: bool,
) -> serde_json::Value {
    let outcome = repair_with_engine_and_options(
        source,
        goal,
        preserve,
        max_steps,
        engine,
        file_ctx,
        allow_parent_escapes,
    );
    let parsed = match file_ctx {
        Some(path) => {
            let options = crate::ParseOptions {
                enable_includes: true,
                allow_parent_escapes,
                ..Default::default()
            };
            crate::parse_text_recovering_with_file(source, &path.to_string_lossy(), options, path)
        }
        None => match crate::parse(source, "browser") {
            Ok(module) => (module, Vec::new()),
            Err(diags) => return model_with_diagnostics(outcome, diags),
        },
    };
    let (module, parse_diags) = parsed;
    // Combine recovering parse diagnostics with evaluation diagnostics,
    // deduping by rendered record: evaluation diagnostics may repeat a
    // parse record, and neither source may be dropped (a parse error
    // the plan still reports must show up in the browser).
    let mut diagnostics: Vec<serde_json::Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let push = |rows: &mut Vec<serde_json::Value>,
                seen: &mut std::collections::HashSet<String>,
                diagnostic: crate::Diagnostic| {
        let row = serde_json::json!({
            "code": diagnostic.code,
            "severity": diagnostic.severity,
            "message": diagnostic.message,
            "path": diagnostic.path,
            "span": diagnostic.span,
            "file": diagnostic.file,
        });
        let key = row.to_string();
        if seen.insert(key) {
            rows.push(row);
        }
    };
    for diagnostic in parse_diags {
        push(&mut diagnostics, &mut seen, diagnostic);
    }
    match engine.evaluate(&module) {
        Ok((_, diags)) => {
            for diagnostic in diags {
                push(&mut diagnostics, &mut seen, diagnostic);
            }
        }
        Err(crate::eval::EvalError::ValidationFailed { diagnostics: ds }) => {
            for diagnostic in ds {
                push(&mut diagnostics, &mut seen, diagnostic);
            }
        }
        Err(error) => {
            diagnostics.push(serde_json::json!({
                "code": error.code(),
                "severity": "error",
                "message": error.to_string(),
                "path": null,
                "span": error.span(),
                "file": error.file(),
            }));
        }
    }
    let mut plan = serde_json::to_value(&outcome).unwrap_or(serde_json::Value::Null);
    if let Some(object) = plan.as_object_mut() {
        object.remove("repaired_source");
    }
    serde_json::json!({
        "diagnostics": diagnostics,
        "plan": plan,
    })
}

fn model_with_diagnostics(
    outcome: RepairOutcome,
    diagnostics: Vec<crate::Diagnostic>,
) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = diagnostics
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "code": d.code,
                "severity": d.severity,
                "message": d.message,
                "path": d.path,
                "span": d.span,
                "file": d.file,
            })
        })
        .collect();
    let mut plan = serde_json::to_value(&outcome).unwrap_or(serde_json::Value::Null);
    if let Some(object) = plan.as_object_mut() {
        object.remove("repaired_source");
    }
    serde_json::json!({"diagnostics": rows, "plan": plan})
}

/// Apply a selected subset of planned steps to the original source:
/// safe, applied steps inside `max_index` (inclusive) and/or matching
/// `prefix` are prepended in plan order (the same composition the
/// repair loop uses). Returns the source plus the directives actually
/// applied, for auditability.
pub fn apply_selected(
    source: &str,
    steps: &[PlannedStep],
    max_index: Option<usize>,
    prefix: Option<&str>,
) -> (String, Vec<String>) {
    let mut applied = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let index_ok = max_index.is_none_or(|limit| index <= limit);
        let prefix_ok = prefix.is_none_or(|p| step.path.starts_with(p));
        if index_ok && prefix_ok && step.safe && matches!(step.status, StepStatus::Applied) {
            applied.push(step.directive.clone());
        }
    }
    (prepend_directives(source, &applied), applied)
}

/// Prepend directives to source while keeping a leading BOM at byte 0
/// (the lexer skips it only at the start of input).
fn prepend_directives(source: &str, directives: &[String]) -> String {
    let (bom, body) = match source.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", source),
    };
    let mut out = String::from(bom);
    for directive in directives {
        out.push_str(directive);
        out.push('\n');
    }
    out.push_str(body);
    out
}
