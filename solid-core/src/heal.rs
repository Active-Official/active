//! Self-healing logic (alpha stubs with real confidence thresholds)
//!
//! TODO(solid C-016): implement the three-phase production healer
//! (syntactic repair, semantic key repair, type/constraint healing) per the
//! healing policy in docs/DESIGN.md and the origin design document.

use crate::diag::{thresholds, Diagnostic, Repair, RepairKind, Severity};
use crate::ir::Module;

/// Attempt to heal common problems. Returns number of safe repairs applied.
pub fn heal(module: &mut Module) -> usize {
    let applied = 0;

    // In a full implementation this would walk the original source spans
    // and apply the three phases described in the design docs.
    // For the alpha we only record example diagnostics.

    if module.diagnostics.is_empty() {
        // TODO(solid C-016): real healing passes replace this placeholder.
        let _ = thresholds::AUTO_APPLY;
    }

    applied
}

pub fn make_key_typo_diagnostic(original: &str, suggested: &str, confidence: f64) -> Diagnostic {
    Diagnostic {
        severity: if confidence >= thresholds::HIGH {
            Severity::Warning
        } else {
            Severity::Error
        },
        code: "H001".into(),
        message: format!("unknown key '{}', did you mean '{}'?", original, suggested),
        path: Some(original.to_string()),
        span: None,
        repair: Some(Repair {
            original: original.to_string(),
            suggested: suggested.to_string(),
            kind: RepairKind::KeyTypo,
            safe: confidence >= thresholds::HIGH,
        }),
        confidence,
    }
}
