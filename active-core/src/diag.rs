//! Structured diagnostics and repair records

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub path: Option<String>,
    pub span: Option<Span>,
    pub repair: Option<Repair>,
    /// 0.0 – 1.0
    pub confidence: f64,
    /// Source file the diagnostic came from. Set for multi-file documents
    /// (includes, C-025); unset for single-file parses, where tools fall
    /// back to the input filename.
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repair {
    pub original: String,
    pub suggested: String,
    pub kind: RepairKind,
    /// If true, safe to auto-apply
    pub safe: bool,
    /// Dotted paths this repair changes beyond its own target
    /// (PROPOSAL §2.1, CHECKLIST R.001): empty for purely additive
    /// fills; renames list the renamed path so agents and cherry-pick
    /// flows know the blast radius.
    #[serde(default)]
    pub side_effects: Vec<String>,
    /// Goal this repair serves when built by the repair planner
    /// (None for single diagnostics outside a plan).
    #[serde(default)]
    pub goal: Option<RepairGoal>,
    /// Sub-steps for multi-step repairs (empty for single repairs).
    #[serde(default)]
    pub steps: Vec<RepairStep>,
}

/// Goal a repair plan optimizes for (PROPOSAL §2.1, CHECKLIST R.002).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepairGoal {
    /// Reach a clean validation (default).
    Validate,
    /// Reach clean validation without touching the listed paths.
    PreserveKey(Vec<String>),
    /// Fewest, smallest steps first.
    MinimizeDiff,
}

/// One step of a multi-step repair (CHECKLIST R.002): the action in
/// directive form, its target path, and the observable effect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepairStep {
    /// `@rename("a", "b")` / `@patch("p", v)` directive text.
    pub action: String,
    /// Dotted path this step changes.
    pub target: String,
    /// Observable effect (e.g. `fill`, `rename`, `skip: preserved`).
    pub effect: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairKind {
    InsertQuote,
    RemoveTrailingComma,
    KeyTypo,
    TypeCoercion,
    MissingDefault,
    UnitNormalization,
    Other(String),
}

/// Official confidence thresholds used by the healer
pub mod thresholds {
    pub const AUTO_APPLY: f64 = 0.95;
    pub const HIGH: f64 = 0.85;
    pub const MEDIUM: f64 = 0.70;
    // below MEDIUM → never auto-apply
}
