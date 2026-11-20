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
/// the blast radius, and (for an applied step) the proof the loop
/// checked by re-evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedStep {
    pub kind: String,
    pub path: String,
    pub directive: String,
    pub safe: bool,
    pub status: StepStatus,
    pub side_effects: Vec<String>,
    /// The mechanical check behind the step (L-P53). Absent on skipped
    /// steps and on plans that never applied anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<RepairProof>,
}

/// The proof sketch carried by an applied repair step (L-P53): what it
/// removed, what it introduced, and whether anything outside its own
/// path changed. Every field is a code or a path name; values never
/// appear, so a proof can be shown to an approver without redaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RepairProof {
    /// Blocking diagnostics (`code@path`) the step was planned for.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// Blocking diagnostics that disappeared when the step applied.
    #[serde(default)]
    pub removes: Vec<String>,
    /// Blocking diagnostics that were not there before the step.
    #[serde(default)]
    pub introduces: Vec<String>,
    /// Paths outside the step's blast radius whose value changed.
    #[serde(default)]
    pub changes_elsewhere: Vec<String>,
    /// True only when the check ran and every condition held.
    #[serde(default)]
    pub verified: bool,
    /// Why the proof is not verified, or why it could not be checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Value nodes compared (bounded; visible to an approver).
    #[serde(default)]
    pub checked_paths: u64,
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
    // must not read as stuck. A blocking diagnostic with no path keeps
    // the bare code (the documented `final_codes` shape for values that
    // never had a path), so `CODE@` and `CODE` cannot both mean
    // "pathless".
    let mut codes: Vec<String> = diags
        .iter()
        .filter(|d| is_blocking(d))
        .map(|d| match d.path.as_deref() {
            Some(path) => format!("{}@{}", d.code, path),
            None => d.code.clone(),
        })
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

/// Whether `path` is `base` or sits below it.
fn path_within(path: &str, base: &str) -> bool {
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}

/// Preserve is a subtree contract: a step touching a preserved path, an
/// ancestor of one, or a descendant of one is skipped.
fn touches_preserved_path(path: &str, preserve: &[String]) -> bool {
    preserve
        .iter()
        .any(|protected| path_within(path, protected) || path_within(protected, path))
}

/// Hard ceiling on applied steps at the library boundary, matching the
/// CLI's own cap: an embedder cannot ask the loop for an unbounded plan.
pub const MAX_REPAIR_STEPS: usize = 64;

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
    let max_steps = max_steps.min(MAX_REPAIR_STEPS);
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
    // L-P53: the step applied at the end of the previous iteration, and
    // the evaluation state it must be checked against.
    let mut pending: Option<usize> = None;
    let mut before_codes: Vec<String> = Vec::new();
    let mut before_value: Option<crate::Value> = None;

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
                        if let Some(index) = pending.take() {
                            steps[index].proof = Some(unverified_proof(
                                &steps[index],
                                &before_codes,
                                "the repaired source does not parse",
                            ));
                        }
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
                    if let Some(index) = pending.take() {
                        steps[index].proof = Some(unverified_proof(
                            &steps[index],
                            &before_codes,
                            "the repaired source does not parse",
                        ));
                    }
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
        let (value, diags) = match engine.evaluate(&module) {
            Ok((value, diags)) => (Some(value), diags),
            // Validation failures still yield their diagnostics: those
            // ARE the planning input. The tree is still needed for the
            // step proof (L-P53), and strict validation never mutates
            // it, so the unvalidated walk gives the same value.
            Err(crate::eval::EvalError::ValidationFailed { diagnostics }) => {
                let value = engine.evaluate_tree(&module).ok().map(|(value, _)| value);
                (value, diagnostics)
            }
            Err(e) => {
                unrepairable = true;
                if let Some(index) = pending.take() {
                    steps[index].proof = Some(unverified_proof(
                        &steps[index],
                        &before_codes,
                        &format!("re-evaluation fails fatally ({})", e.code()),
                    ));
                }
                findings.push(unrepairable_finding(&format!(
                    "evaluation fails fatally ({}); no directive can fix it",
                    e.code()
                )));
                final_codes = vec![e.code().to_string()];
                break;
            }
        };
        let codes = sorted_codes(&diags);
        // The planning profile sees the engine's environment and profile
        // selection, so a default that copies a runtime value is
        // withheld as well as an authored one. Proofs compare REDACTED
        // trees (L-P53): material never enters the comparison, so a
        // proof can never become a secret-equality oracle.
        let profile = crate::secret::SecretProfile::with_context(
            &module,
            engine.profile_override.as_deref(),
            Some(&engine.env),
            None,
        );
        let redacted_before = before_value
            .as_ref()
            .map(|value| crate::convert::redact_value(&profile, value));
        let redacted_after = value
            .as_ref()
            .map(|value| crate::convert::redact_value(&profile, value));
        // This evaluation is the "after" state of the previous step.
        if let Some(index) = pending.take() {
            steps[index].proof = Some(prove_step(
                &steps[index],
                &before_codes,
                &codes,
                redacted_before.as_ref(),
                redacted_after.as_ref(),
            ));
        }
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

        let suggestions =
            crate::convert::suggest_patches_structured_with_profile(&module, &diags, &profile);
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
                // Preserve blocks the path, its ancestors, and its
                // descendants, plus both rename endpoints: renaming a
                // parent of a preserved path still touches it.
                let touches_preserved = touches_preserved_path(&s.path, &protected)
                    || s.from
                        .as_ref()
                        .is_some_and(|p| touches_preserved_path(p, &protected))
                    || s.to
                        .as_ref()
                        .is_some_and(|p| touches_preserved_path(p, &protected));
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
                        proof: None,
                    });
                    continue;
                }
                let side_effects = match (&s.from, &s.to) {
                    // Renames name both endpoints as the blast radius;
                    // fills are purely additive (no side effects).
                    (Some(from), Some(to)) => vec![from.clone(), to.clone()],
                    _ => Vec::new(),
                };
                // The next evaluation is this step's "after" state; the
                // proof is filled in there. Both trees are redacted at
                // proof time under that evaluation's profile, so the
                // comparison never mixes profiles.
                before_codes = codes.clone();
                before_value = value.clone();
                pending = Some(steps.len());
                steps.push(PlannedStep {
                    kind: format!("{:?}", s.kind).to_lowercase(),
                    path: s.path.clone(),
                    directive: directive.clone(),
                    safe: true,
                    status: StepStatus::Applied,
                    side_effects,
                    proof: None,
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
            let mut options = crate::ParseOptions::strict();
            options.enable_includes = true;
            options.allow_parent_escapes = allow_parent_escapes;
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
        // A prefix selects the path and its subtree, never a sibling
        // whose name merely starts with the same text (`server` must not
        // select `serverBackup`).
        let prefix_ok = prefix.is_none_or(|p| path_within(&step.path, p));
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

// ----------------------------------------------------------------------
// Repair proofs (L-P53)
// ----------------------------------------------------------------------

/// Visited values allowed while comparing two trees for a proof.
const PROOF_MAX_VISITED: u64 = 65_536;
/// Changed paths a proof reports before it gives up (a bound, never a
/// silent truncation: exceeding it fails the proof).
const PROOF_MAX_REPORTED: usize = 16;
/// Depth a proof comparison may descend (a defensive bound for a
/// host-raised evaluation depth; every tree the parser builds is far
/// shallower).
const PROOF_MAX_DEPTH: usize = 512;

/// The paths a step is allowed to change: its own path plus every named
/// endpoint (a rename names both sides).
fn blast_radius(step: &PlannedStep) -> Vec<String> {
    let mut paths = vec![step.path.clone()];
    for extra in &step.side_effects {
        if !paths.contains(extra) {
            paths.push(extra.clone());
        }
    }
    paths
}

/// The path part of a `code@path` diagnostic key.
fn code_path(code: &str) -> &str {
    code.split_once('@').map(|(_, path)| path).unwrap_or("")
}

fn within_radius(code: &str, radius: &[String]) -> bool {
    let path = code_path(code);
    radius.iter().any(|base| path_within(path, base))
}

/// Whether `path` is inside the radius or is an ancestor of it (a step
/// that fills `server.port` creates `server` when absent).
fn inside_radius(path: &str, radius: &[String]) -> bool {
    radius
        .iter()
        .any(|base| path_within(path, base) || path_within(base, path))
}

fn sorted_diff(from: &[String], not_in: &[String]) -> Vec<String> {
    let mut out: Vec<String> = from
        .iter()
        .filter(|code| !not_in.contains(code))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A proof that could not run: the check is reported, never assumed.
fn unverified_proof(step: &PlannedStep, before_codes: &[String], reason: &str) -> RepairProof {
    let radius = blast_radius(step);
    RepairProof {
        addresses: before_codes
            .iter()
            .filter(|code| within_radius(code, &radius))
            .cloned()
            .collect(),
        verified: false,
        reason: Some(reason.to_string()),
        ..RepairProof::default()
    }
}

/// Check one applied step against the evaluation that followed it:
/// it must remove a diagnostic it addresses, introduce none, and change
/// no value outside its blast radius. Both trees are REDACTED before they
/// reach this function (L-P53), so the comparison never sees material.
fn prove_step(
    step: &PlannedStep,
    before_codes: &[String],
    after_codes: &[String],
    before_value: Option<&crate::Value>,
    after_value: Option<&crate::Value>,
) -> RepairProof {
    let radius = blast_radius(step);
    let addresses: Vec<String> = before_codes
        .iter()
        .filter(|code| within_radius(code, &radius))
        .cloned()
        .collect();
    let removes = sorted_diff(before_codes, after_codes);
    let introduces = sorted_diff(after_codes, before_codes);
    let mut changes_elsewhere = Vec::new();
    let mut checked_paths = 0u64;
    let compare_failure = match (before_value, after_value) {
        (Some(before), Some(after)) => compare_except(
            before,
            after,
            "",
            &radius,
            &mut changes_elsewhere,
            &mut checked_paths,
            0,
        )
        .err()
        .map(|()| {
            // A bound trip fails the proof; a partial list would read
            // like a complete one.
            changes_elsewhere.clear();
            "value comparison exceeded its bound".to_string()
        }),
        _ => Some("no evaluated tree to compare (evaluation did not complete)".to_string()),
    };
    let reason = compare_failure.or_else(|| {
        let addressed_removed = removes.iter().any(|code| addresses.contains(code));
        if addresses.is_empty() {
            Some("the step names no diagnostic it was planned for".to_string())
        } else if !addressed_removed {
            // Clearing some other diagnostic is not this step's proof:
            // the one it was planned for must be the one that went away.
            Some("the step removed none of the diagnostics it was planned for".to_string())
        } else if !introduces.is_empty() {
            Some("the step introduced new diagnostics".to_string())
        } else if !changes_elsewhere.is_empty() {
            Some("the step changed values outside its path".to_string())
        } else {
            None
        }
    });
    RepairProof {
        addresses,
        removes,
        introduces,
        changes_elsewhere,
        verified: reason.is_none(),
        reason,
        checked_paths,
    }
}

/// Whether two evaluated values are the same for proof purposes.
/// Material is opaque: two secret values are never compared (an oracle
/// in the making), and two tainted blobs compare by their identity
/// (hash and size), which the redactor preserves.
fn values_match(before: &crate::Value, after: &crate::Value) -> bool {
    match (before, after) {
        (crate::Value::Secret(_), crate::Value::Secret(_)) => true,
        (
            crate::Value::Blob {
                hash: xh,
                size: xs,
                tainted: true,
                ..
            },
            crate::Value::Blob {
                hash: yh,
                size: ys,
                tainted: true,
                ..
            },
        ) => xh == yh && xs == ys,
        _ => crate::diff::same_value(before, after),
    }
}

/// Compare two trees, recording paths outside `radius` whose value
/// changed. Paths inside the radius are skipped entirely (that is what
/// the step was allowed to touch). `Err(())` means the visit bound was
/// exceeded.
fn compare_except(
    before: &crate::Value,
    after: &crate::Value,
    path: &str,
    radius: &[String],
    changed: &mut Vec<String>,
    visited: &mut u64,
    depth: usize,
) -> Result<(), ()> {
    if depth > PROOF_MAX_DEPTH {
        return Err(());
    }
    *visited += 1;
    if *visited > PROOF_MAX_VISITED {
        return Err(());
    }
    if !path.is_empty() && radius.iter().any(|base| path_within(path, base)) {
        return Ok(());
    }
    match (before, after) {
        (crate::Value::Table(a), crate::Value::Table(b)) => {
            let mut keys: Vec<&String> = a.keys().collect();
            for key in b.keys() {
                if !a.contains_key(key) {
                    keys.push(key);
                }
            }
            keys.sort();
            keys.dedup();
            for key in keys {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) => {
                        compare_except(x, y, &child, radius, changed, visited, depth + 1)?
                    }
                    // A key the step added or removed is a change at that
                    // key's own path: inside the radius it is the step's
                    // business, outside it is a proof failure. A step whose
                    // path is deeper than the key it creates (a fill at
                    // `server.port` when `server` is absent) owns the
                    // containers it had to create, too.
                    _ if inside_radius(&child, radius) => {}
                    _ => report_change(changed, &child)?,
                }
            }
            Ok(())
        }
        (crate::Value::Array(a), crate::Value::Array(b)) => {
            if a.len() != b.len() {
                // The length change is the change; comparing a shifted
                // zip would double-report elements that only moved. A
                // container the step had to grow is in scope.
                if !inside_radius(path, radius) {
                    report_change(changed, path)?;
                }
                return Ok(());
            }
            for (index, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                compare_except(
                    x,
                    y,
                    &format!("{path}[{index}]"),
                    radius,
                    changed,
                    visited,
                    depth + 1,
                )?;
            }
            Ok(())
        }
        _ => {
            if !values_match(before, after) {
                report_change(changed, path)?;
            }
            Ok(())
        }
    }
}

fn report_change(changed: &mut Vec<String>, path: &str) -> Result<(), ()> {
    if changed.len() >= PROOF_MAX_REPORTED {
        return Err(());
    }
    changed.push(if path.is_empty() {
        "<root>".to_string()
    } else {
        path.to_string()
    });
    Ok(())
}

#[cfg(test)]
mod proof_tests {
    use super::*;
    use crate::Value;
    use indexmap::IndexMap;

    fn table(pairs: Vec<(&str, Value)>) -> Value {
        Value::Table(
            pairs
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect::<IndexMap<_, _>>(),
        )
    }

    fn owned_table(pairs: Vec<(String, Value)>) -> Value {
        Value::Table(pairs.into_iter().collect::<IndexMap<_, _>>())
    }

    fn step(path: &str, side_effects: Vec<&str>) -> PlannedStep {
        PlannedStep {
            kind: "fill".to_string(),
            path: path.to_string(),
            directive: String::new(),
            safe: true,
            status: StepStatus::Applied,
            side_effects: side_effects.into_iter().map(str::to_string).collect(),
            proof: None,
        }
    }

    #[test]
    fn a_change_outside_the_radius_fails_the_proof() {
        let before = table(vec![
            ("port", Value::Int(1)),
            ("host", Value::String("a".into())),
        ]);
        let after = table(vec![
            ("port", Value::Int(1)),
            ("host", Value::String("b".into())),
        ]);
        let proof = prove_step(
            &step("port", vec![]),
            &["T004@port".to_string()],
            &[],
            Some(&before),
            Some(&after),
        );
        assert!(!proof.verified, "{proof:?}");
        assert_eq!(proof.changes_elsewhere, vec!["host".to_string()]);
        assert_eq!(
            proof.reason.as_deref(),
            Some("the step changed values outside its path")
        );
    }

    #[test]
    fn a_change_inside_the_radius_is_the_steps_business() {
        let before = table(vec![
            ("port", Value::Int(1)),
            ("host", Value::String("a".into())),
        ]);
        let after = table(vec![
            ("port", Value::Int(8080)),
            ("host", Value::String("a".into())),
        ]);
        let proof = prove_step(
            &step("port", vec![]),
            &["T004@port".to_string()],
            &[],
            Some(&before),
            Some(&after),
        );
        assert!(proof.verified, "{proof:?}");
        assert!(proof.changes_elsewhere.is_empty());
    }

    #[test]
    fn a_rename_may_move_its_endpoint() {
        let before = table(vec![("Host", Value::String("a".into()))]);
        let after = table(vec![("host", Value::String("a".into()))]);
        let proof = prove_step(
            &step("Host", vec!["Host", "host"]),
            &["T005@Host".to_string()],
            &[],
            Some(&before),
            Some(&after),
        );
        assert!(proof.verified, "{proof:?}");
    }

    #[test]
    fn secrets_are_opaque_not_changed() {
        let before = table(vec![("token", Value::Secret(Default::default()))]);
        let after = table(vec![
            ("token", Value::Secret(Default::default())),
            ("port", Value::Int(1)),
        ]);
        let proof = prove_step(
            &step("port", vec![]),
            &["T004@port".to_string()],
            &[],
            Some(&before),
            Some(&after),
        );
        assert!(proof.verified, "{proof:?}");
        assert!(proof.changes_elsewhere.is_empty());
    }

    #[test]
    fn the_report_bound_fails_instead_of_truncating() {
        let before = owned_table((0..20).map(|i| (format!("k{i}"), Value::Int(0))).collect());
        let after = owned_table((0..20).map(|i| (format!("k{i}"), Value::Int(1))).collect());
        let mut changed = Vec::new();
        let mut visited = 0;
        let result = compare_except(
            &before,
            &after,
            "",
            &["port".to_string()],
            &mut changed,
            &mut visited,
            0,
        );
        assert!(result.is_err(), "bound must fail the comparison");
        assert_eq!(changed.len(), PROOF_MAX_REPORTED);
    }

    #[test]
    fn a_bound_trip_reports_no_partial_list() {
        let before = owned_table((0..20).map(|i| (format!("k{i}"), Value::Int(0))).collect());
        let after = owned_table((0..20).map(|i| (format!("k{i}"), Value::Int(1))).collect());
        let proof = prove_step(
            &step("port", vec![]),
            &["T004@port".to_string()],
            &[],
            Some(&before),
            Some(&after),
        );
        assert!(!proof.verified);
        assert!(proof.changes_elsewhere.is_empty(), "{proof:?}");
        assert!(proof.reason.as_deref().unwrap_or("").contains("bound"));
    }

    #[test]
    fn a_container_the_step_had_to_create_is_inside_the_radius() {
        // A fill at `server.port` creates the `server` table when it is
        // absent; that container change belongs to the step.
        let before = table(vec![]);
        let after = table(vec![("server", table(vec![("port", Value::Int(8080))]))]);
        let proof = prove_step(
            &step("server.port", vec![]),
            &["T004@server.port".to_string()],
            &[],
            Some(&before),
            Some(&after),
        );
        assert!(proof.verified, "{proof:?}");
    }

    #[test]
    fn removing_the_wrong_diagnostic_is_not_a_pass() {
        // H1: clearing some other diagnostic is not this step's proof.
        let before = table(vec![("port", Value::Int(1)), ("other", Value::Int(2))]);
        let proof = prove_step(
            &step("port", vec![]),
            &["T004@port".to_string(), "T004@other".to_string()],
            &["T004@port".to_string()],
            Some(&before),
            Some(&before),
        );
        assert!(!proof.verified, "{proof:?}");
        assert_eq!(proof.removes, vec!["T004@other".to_string()]);
        assert!(proof
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("removed none of the diagnostics"));
    }

    #[test]
    fn a_swapped_tainted_blob_outside_the_radius_fails_the_proof() {
        let blob = |hash: u8| crate::Value::Blob {
            hash: [hash; 32],
            size: 4,
            tainted: true,
            media_type: crate::secret::MARKER.to_string(),
            payload: None,
        };
        let before = table(vec![("port", Value::Int(1)), ("token", blob(1))]);
        let after = table(vec![("port", Value::Int(8080)), ("token", blob(2))]);
        let proof = prove_step(
            &step("port", vec![]),
            &["T004@port".to_string()],
            &[],
            Some(&before),
            Some(&after),
        );
        assert!(!proof.verified, "{proof:?}");
        assert_eq!(proof.changes_elsewhere, vec!["token".to_string()]);
    }

    #[test]
    fn an_array_length_change_is_reported_once() {
        let before = crate::Value::Array(vec![Value::Int(1), Value::Int(2)]);
        let after = crate::Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let mut changed = Vec::new();
        let mut visited = 0;
        compare_except(
            &before,
            &after,
            "",
            &["port".to_string()],
            &mut changed,
            &mut visited,
            0,
        )
        .expect("within bound");
        assert_eq!(changed, vec!["<root>".to_string()]);
    }

    #[test]
    fn a_cherry_pick_prefix_matches_the_subtree_not_a_sibling() {
        let steps = vec![
            PlannedStep {
                kind: "fill".to_string(),
                path: "server".to_string(),
                directive: "@patch(\"server\", 1)".to_string(),
                safe: true,
                status: StepStatus::Applied,
                side_effects: Vec::new(),
                proof: None,
            },
            PlannedStep {
                kind: "fill".to_string(),
                path: "serverBackup".to_string(),
                directive: "@patch(\"serverBackup\", 2)".to_string(),
                safe: true,
                status: StepStatus::Applied,
                side_effects: Vec::new(),
                proof: None,
            },
        ];
        let (source, applied) = apply_selected("x = 1\n", &steps, None, Some("server"));
        assert_eq!(applied, vec!["@patch(\"server\", 1)".to_string()]);
        assert!(source.contains("@patch(\"server\", 1)"));
        assert!(!source.contains("serverBackup"));
    }

    #[test]
    fn an_array_the_step_had_to_grow_is_inside_the_radius() {
        let before = table(vec![("arr", Value::Array(vec![Value::Int(1)]))]);
        let after = table(vec![(
            "arr",
            Value::Array(vec![Value::Int(1), Value::Int(2)]),
        )]);
        let proof = prove_step(
            &step("arr[1]", vec![]),
            &["T004@arr[1]".to_string()],
            &[],
            Some(&before),
            Some(&after),
        );
        assert!(proof.verified, "{proof:?}");
    }

    #[test]
    fn a_missing_tree_is_not_a_pass() {
        let proof = prove_step(
            &step("port", vec![]),
            &["T004@port".to_string()],
            &[],
            None,
            None,
        );
        assert!(!proof.verified);
        assert!(proof
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("no evaluated tree"));
    }

    #[test]
    fn nothing_removed_is_not_a_pass() {
        let before = table(vec![("port", Value::Int(1))]);
        let proof = prove_step(
            &step("port", vec![]),
            &["T004@port".to_string(), "T002@host".to_string()],
            &["T004@port".to_string(), "T002@host".to_string()],
            Some(&before),
            Some(&before),
        );
        assert!(!proof.verified);
        assert!(proof.removes.is_empty());
        assert!(proof.changes_elsewhere.is_empty());
    }
}
