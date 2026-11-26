//! Evaluator — topological evaluation of the IR
//!
//! - Cycle detection
//! - Topological order
//! - Full string interpolation
//! - Unit normalization (time → ms, bytes → raw bytes)
//! - Secret redaction / taint
//! - Basic arithmetic and logic

use petgraph::algo::toposort;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use thiserror::Error;

use crate::diag::Span;
use crate::diag::{Diagnostic, Severity};
use crate::ir::*;
use crate::parser::{validate_patch_path, PathSeg};
use crate::secret::{SecretProfile, SecretRedactor};
use crate::types::Unit;
use crate::unify::{MergeBudgetKind, MergeError, MergeProvenance};

pub mod cost;
pub mod demand;
pub use cost::{CostClass, CostModel, CostNode};
pub use demand::{ChargeReport, Closure, Demand, DemandOutcome};

/// Host-provided environment lookup (F-016): consulted on map miss before
/// defaults apply. Mirrors the map-hit charging exactly (bytes as delta),
/// so lazy host values cost what eager values cost. `None` preserves all
/// legacy behavior (map, then default, then null).
///
/// The `arc_with_non_send_sync` lint is allowed here with justification:
/// the evaluator is single-threaded by construction and the FFI contract
/// confines modules to the calling thread, so this Arc is never shared
/// across threads; it exists so handles can cheaply hand the lookup down.
#[allow(clippy::arc_with_non_send_sync)]
pub type EnvLookup = std::sync::Arc<dyn Fn(&str) -> Option<String>>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EvalError {
    #[error("cycle detected in dependency graph")]
    CycleDetected,
    #[error("missing root")]
    MissingRoot,
    #[error("node {0:?} not found")]
    MissingNode(NodeId),
    #[error("type error: {message}")]
    TypeError {
        message: String,
        span: Option<Span>,
        file: Option<String>,
    },
    /// A patch application failure with its own stable code (E019):
    /// rename conflicts and invalid paths discovered at apply time.
    /// Missing remove/rename targets warn (W004) instead of failing.
    #[error("patch error: {message}")]
    PatchError {
        message: String,
        span: Option<Span>,
        file: Option<String>,
    },
    /// A profile body tried to rewrite document metadata that the mode
    /// and capability handshake already read (LANG-A-009). Distinct from
    /// `PatchError`: no patch directive was involved, and agents that
    /// match on error kinds must not misclassify it.
    #[error("{message}")]
    ProfileError {
        message: String,
        span: Option<Span>,
        file: Option<String>,
    },
    /// L-P35: a content authority was not granted for this document.
    #[error("capability '{capability}' is not granted for this document")]
    CapabilityDenied {
        capability: &'static str,
        span: Option<Span>,
    },
    /// L-P28: a sandboxed evaluation refused an input outside its
    /// InputClosure (E048). The message names the input family, never a
    /// value.
    #[error("{message}")]
    SandboxRefused {
        message: String,
        span: Option<Span>,
        file: Option<String>,
    },
    /// L-P05: a pure evaluation refused an ambient input (E047). The
    /// message names the input (env variable, include spelling, or
    /// profile) and the refusal family.
    #[error("{message}")]
    PureRefused {
        message: String,
        span: Option<Span>,
        file: Option<String>,
    },
    #[error("secret used in unsafe context")]
    SecretLeak { file: Option<String> },
    /// Blob materialisation failures (spec/blob.md §8): tainted
    /// materialisation refused (B003) or payload not retained (B005).
    /// Value-free by construction: only the code, path, and the fixed
    /// reason travel — never payload bytes.
    #[error("blob materialisation refused ({code}): {message}")]
    BlobError {
        code: &'static str,
        message: String,
        span: Option<Span>,
        file: Option<String>,
    },
    /// Schema validation failures (C-031, spec/validation.md) and
    /// constraint failures (C-034, spec/annotations.md): every
    /// violation diagnostic, collected in deterministic order. Strict
    /// mode returns these instead of a value tree; resilient mode
    /// keeps them as ordinary diagnostics alongside the tree.
    #[error("validation failed")]
    ValidationFailed { diagnostics: Vec<Diagnostic> },
    /// Execution budget violations (C-037, spec/budgets.md): always
    /// fatal, in strict and resilient modes alike — budgets protect
    /// the process, not the document.
    #[error("evaluation budget exceeded: {kind} (limit {limit})")]
    BudgetExceeded {
        kind: BudgetKind,
        limit: u64,
        /// Diagnostics collected before the abort (currently the Data Mode
        /// progressive warnings, W013). A trip replaces the diagnostic list
        /// with this error, so without them an agent watching W013 as its
        /// "approaching a ceiling" signal would never see it on a slow host
        /// (LANG-C-009). Read with [`EvalError::attached_warnings`].
        warnings: Vec<Diagnostic>,
        span: Option<Span>,
        file: Option<String>,
    },
}

/// Which budget tripped (C-037). Display strings name the budget;
/// the error message adds the configured limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetKind {
    Nodes,
    WallClock,
    Depth,
    ArrayLength,
    Keys,
    /// Materialized value size (reference expansion included).
    ValueSize,
    /// Cumulative materialized size across evaluation (DP total and
    /// runtime accountant): trips X006 like ValueSize, but names the
    /// cumulative bound in messages and warnings.
    TotalSize,
}

impl std::fmt::Display for BudgetKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BudgetKind::Nodes => write!(f, "node budget"),
            BudgetKind::WallClock => write!(f, "time budget"),
            BudgetKind::Depth => write!(f, "depth budget"),
            BudgetKind::ArrayLength => write!(f, "array budget"),
            BudgetKind::Keys => write!(f, "key budget"),
            BudgetKind::ValueSize => write!(f, "value size budget"),
            BudgetKind::TotalSize => write!(f, "cumulative value size budget"),
        }
    }
}

/// Runtime resource limits enforced by the evaluator accountant
/// A zero field means the shipped default, not unlimited. To disable a
/// limit, use `Budget::unlimited()` explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Maximum IR nodes visited (arena-length pre-check).
    pub max_nodes: u64,
    /// Maximum value nesting depth (reference-aware).
    pub max_depth: u64,
    /// Longest single array.
    pub max_array_len: u64,
    /// Total value-table keys across the arena (schema tables
    /// excluded: they are metadata, validated separately).
    pub max_keys: u64,
    /// Wall-clock milliseconds for the whole evaluation.
    pub max_wall_ms: u64,
    /// Maximum materialized value size in data units (scalars and
    /// string bytes), counting reference expansion: each `Ref` clone
    /// costs its target's bound. Kills exponential blowup
    /// (billion-laughs class) before anything materializes.
    pub max_value_size: u64,
    /// Maximum cumulative materialized size across the whole
    /// evaluation (same units). Per-node bounds cap each clone;
    /// this caps their sum — many aliases of one large value trip
    /// here even when every node is individually fine.
    pub max_total_value_size: u64,
}

impl Default for Budget {
    /// Shipped profile: tight enough for CI and serverless loops,
    /// with headroom over the C-055 10k-node performance target.
    fn default() -> Self {
        Self {
            max_nodes: 100_000,
            max_depth: 256,
            max_array_len: 100_000,
            max_keys: 100_000,
            max_wall_ms: 100,
            max_value_size: 10_000_000,
            max_total_value_size: 100_000_000,
        }
    }
}

/// Sentry value for `Budget`: a field set to this is explicitly unlimited.
///
/// A zero field resolves to the SHIPPED DEFAULT (see `Budget::resolve`), so
/// disabling a limit requires this value or `Budget::unlimited()`; no one
/// disables a ceiling with a zeroed struct by accident.
pub const UNLIMITED: u64 = u64::MAX;

impl Budget {
    /// Every limit disabled, explicitly. Prefer this to a zeroed struct:
    /// zeros mean "shipped default" under [`Budget::resolve`].
    pub fn unlimited() -> Self {
        Self {
            max_nodes: UNLIMITED,
            max_depth: UNLIMITED,
            max_array_len: UNLIMITED,
            max_keys: UNLIMITED,
            max_wall_ms: UNLIMITED,
            max_value_size: UNLIMITED,
            max_total_value_size: UNLIMITED,
        }
    }

    /// Resolve zero fields to the shipped default (`Budget::default()`), so
    /// one meaning holds on every surface: a zero field is "use the
    /// default", never "disable". `UNLIMITED` fields stay unlimited.
    pub fn resolve(self) -> Self {
        let shipped = Budget::default();
        let pick = |v: u64, d: u64| if v == 0 { d } else { v };
        Self {
            max_nodes: pick(self.max_nodes, shipped.max_nodes),
            max_depth: pick(self.max_depth, shipped.max_depth),
            max_array_len: pick(self.max_array_len, shipped.max_array_len),
            max_keys: pick(self.max_keys, shipped.max_keys),
            max_wall_ms: pick(self.max_wall_ms, shipped.max_wall_ms),
            max_value_size: pick(self.max_value_size, shipped.max_value_size),
            max_total_value_size: pick(self.max_total_value_size, shipped.max_total_value_size),
        }
    }

    /// True when this limit is the explicit unlimited sentinel.
    pub fn is_unlimited(limit: u64) -> bool {
        limit == UNLIMITED
    }
}

impl Budget {
    /// Strict profile: tight enough for CI and serverless loops, with
    /// headroom over the 10k-node performance target. Identical to
    /// `Default`; named so agents and CLIs select it explicitly.
    pub fn strict() -> Self {
        Self::default()
    }

    /// Interactive profile: roomier for REPLs, playgrounds, and human
    /// iteration (larger docs, longer wall time), still bounded.
    pub fn interactive() -> Self {
        Self {
            max_nodes: 1_000_000,
            max_depth: 512,
            max_array_len: 1_000_000,
            max_keys: 1_000_000,
            max_wall_ms: 2_000,
            max_value_size: 100_000_000,
            max_total_value_size: 1_000_000_000,
        }
    }

    /// Trusted profile: operator-trusted input (vendored standard
    /// library, generated configs) at production scale. Generous but
    /// finite — the `UNLIMITED` sentinel is never a preset; embedders
    /// accept that risk field-by-field, never by profile name.
    pub fn trusted() -> Self {
        Self {
            max_nodes: 10_000_000,
            max_depth: 2048,
            max_array_len: 10_000_000,
            max_keys: 10_000_000,
            max_wall_ms: 30_000,
            max_value_size: 1_000_000_000,
            max_total_value_size: 10_000_000_000,
        }
    }
    /// Absolute evaluation deadline, or None when the wall-clock
    /// budget is disabled (`max_wall_ms == 0`). Saturating: absurd
    /// configured durations degrade to no deadline instead of
    /// panicking on `Instant` overflow.
    ///
    /// Platform delta (G-003): `Instant` aborts on
    /// wasm32-unknown-unknown, so wall-clock degrades to disabled there
    /// (this function returns None). Count, depth, array, key, and
    /// value-size budgets still bound execution, and the `regex` crate
    /// guarantees linear-time matching, so no wall-clock backstop is
    /// needed for termination on that target; hosts enforce their own
    /// timeouts (Worker terminate pattern). See `wall_supported`.
    pub fn wall_deadline(&self) -> Option<Instant> {
        // `UNLIMITED` is the explicit "no clock" value. A raw zero is
        // resolved to the shipped default here too (fail closed: a caller
        // that skipped Budget::resolve must not get an unbounded walk).
        let limit = if self.max_wall_ms == 0 {
            Budget::default().max_wall_ms
        } else {
            self.max_wall_ms
        };
        if limit == UNLIMITED || !Self::wall_supported() {
            None
        } else {
            Instant::now().checked_add(std::time::Duration::from_millis(limit))
        }
    }

    /// False on targets without a monotonic clock (wasm32-unknown-unknown)
    /// and under Miri (its mock clock advances with interpreted work, so
    /// any wall budget would trip spuriously while count, depth, array,
    /// key, and value-size budgets still bind execution there):
    /// `wall_deadline` degrades to None in both cases. Hosts must check
    /// this when they promise wall-clock enforcement to their own callers.
    pub fn wall_supported() -> bool {
        #[cfg(any(target_arch = "wasm32", miri))]
        {
            false
        }
        #[cfg(not(any(target_arch = "wasm32", miri)))]
        {
            true
        }
    }
}

impl EvalError {
    /// Stable diagnostic code for this error class (see docs/diagnostics.md).
    pub fn code(&self) -> &str {
        match self {
            EvalError::CycleDetected => "S001",
            EvalError::MissingRoot => "S002",
            EvalError::MissingNode(_) => "S003",
            EvalError::TypeError { .. } => "T001",
            EvalError::PatchError { .. } => "E019",
            EvalError::ProfileError { .. } => "E027",
            EvalError::CapabilityDenied { .. } => "E049",
            EvalError::SandboxRefused { .. } => "E048",
            EvalError::PureRefused { .. } => "E047",
            EvalError::SecretLeak { .. } => "S004",
            EvalError::BlobError { code, .. } => code,
            EvalError::BudgetExceeded { kind, .. } => match kind {
                BudgetKind::Nodes => "X001",
                BudgetKind::WallClock => "X002",
                BudgetKind::Depth => "X003",
                BudgetKind::ArrayLength => "X004",
                BudgetKind::Keys => "X005",
                BudgetKind::ValueSize => "X006",
                BudgetKind::TotalSize => "X006",
            },
            EvalError::ValidationFailed { diagnostics } => {
                // Pass the first diagnostic's stable code through
                // verbatim: every validation family (T002-T006, B002,
                // B004) keeps its own code on the summary line. The
                // old fixed list collapsed T005/T006 to T002, so log
                // greps and the report_eval_error contract saw the
                // wrong code (LANG-B-020).
                diagnostics.first().map_or("T002", |d| d.code.as_str())
            }
        }
    }

    /// Source span when the error can be attributed to a node position.
    pub fn span(&self) -> Option<&Span> {
        match self {
            EvalError::TypeError { span, .. }
            | EvalError::PatchError { span, .. }
            | EvalError::ProfileError { span, .. } => span.as_ref(),
            EvalError::PureRefused { span, .. }
            | EvalError::SandboxRefused { span, .. }
            | EvalError::CapabilityDenied { span, .. } => span.as_ref(),
            EvalError::BlobError { span, .. } => span.as_ref(),
            EvalError::BudgetExceeded { span, .. } => span.as_ref(),
            EvalError::ValidationFailed { diagnostics } => {
                diagnostics.first().and_then(|d| d.span.as_ref())
            }
            _ => None,
        }
    }

    /// Source file when the error can be attributed to a node position
    /// (multi-file documents via includes, C-025).
    pub fn file(&self) -> Option<&str> {
        match self {
            EvalError::TypeError { file, .. }
            | EvalError::PatchError { file, .. }
            | EvalError::ProfileError { file, .. } => file.as_deref(),
            EvalError::BlobError { file, .. } => file.as_deref(),
            EvalError::PureRefused { file, .. } => file.as_deref(),
            EvalError::SandboxRefused { file, .. } => file.as_deref(),
            EvalError::SecretLeak { file } => file.as_deref(),
            EvalError::BudgetExceeded { file, .. } => file.as_deref(),
            EvalError::ValidationFailed { diagnostics } => {
                diagnostics.first().and_then(|d| d.file.as_deref())
            }
            _ => None,
        }
    }

    /// Attach a source position to a type error (used by the evaluation
    /// loop, which knows the failing node's id).
    fn type_error(message: impl Into<String>, module: &Module, node: NodeId) -> Self {
        EvalError::TypeError {
            message: message.into(),
            span: module.node_spans.get(&node).cloned(),
            file: module.node_files.get(&node).cloned(),
        }
    }

    /// Attach a source position to a budget error (the offending node
    /// when attribution is possible; global conditions carry none).
    /// The configured limit travels with the error for the message.
    pub(crate) fn budget_exceeded(
        kind: BudgetKind,
        limit: u64,
        module: &Module,
        node: Option<NodeId>,
    ) -> Self {
        EvalError::BudgetExceeded {
            kind,
            limit,
            warnings: Vec::new(),
            span: node.and_then(|n| module.node_spans.get(&n).cloned()),
            file: node.and_then(|n| module.node_files.get(&n).cloned()),
        }
    }
    /// Diagnostics that preceded an abort: the Data Mode progressive
    /// warnings (W013) collected before a budget trip. Empty for every
    /// other error kind, and for a clean evaluation.
    pub fn attached_warnings(&self) -> &[Diagnostic] {
        match self {
            EvalError::BudgetExceeded { warnings, .. } => warnings,
            _ => &[],
        }
    }

    /// Attach a source position to a patch error (rename conflicts and
    /// invalid paths discovered at apply time).
    fn patch_error(message: impl Into<String>, module: &Module, node: NodeId) -> Self {
        EvalError::PatchError {
            message: message.into(),
            span: module.node_spans.get(&node).cloned(),
            file: module.node_files.get(&node).cloned(),
        }
    }

    /// Attach a source file to a secret-leak error.
    fn secret_leak(module: &Module, node: NodeId) -> Self {
        EvalError::SecretLeak {
            file: module.node_files.get(&node).cloned(),
        }
    }
}

/// Value path -> the layer that wrote it last (LANG-F-014):
/// `profile:<name>` or `patch:<op>`. Empty when no merge or patch ran.
pub type LayerMap = std::collections::BTreeMap<String, String>;

#[derive(Clone)]
pub struct Engine {
    pub resilient: bool,
    /// CLI `--profile` override: when set, selects this profile instead
    /// of the document's `active_profile` key (spec §3.5).
    pub profile_override: Option<String>,
    /// L-P35: the authorities this host holds. Default all five for
    /// local compatibility; a host narrows explicitly. Profiles never
    /// imply authorities.
    pub capabilities: crate::authority::CapabilitySet,
    /// Runtime resource limits (spec/budgets.md, C-037). Violations
    /// are always fatal, in both modes.
    pub budget: Budget,
    /// The closed environment snapshot: the ONLY external input
    /// evaluation may read (spec/reproducibility.md §1, C-050).
    /// `new()` snapshots the process environment (CLI-compatible);
    /// `with_env` injects a fixed map (tests, agents, replays).
    pub env: std::collections::HashMap<String, String>,
    /// Lazy host lookup consulted on snapshot miss, before defaults
    /// (F-016). `None` preserves legacy behavior exactly.
    pub env_fallback: Option<EnvLookup>,
    /// L-P05: refuse ambient input before evaluation — any `env.NAME`
    /// read (including a `secret.env` locator), any include without a
    /// content pin, and any profile override. The document's own profile
    /// block is part of the document and stays allowed. A refusal is
    /// `E047`; a legitimate environment snapshot is still refused by
    /// this mode, because the promise is "reads no ambient input".
    pub pure: bool,
    /// Blob payload materialisation (spec/blob.md §5): when true,
    /// untainted blob values retain their payload bytes (charged
    /// against the materialised-size budgets); otherwise values carry
    /// identity only. Tainted blobs never retain payloads either way.
    pub materialise_blobs: bool,
    /// The version of the reader THIS host acts as (`"1.4.0"`), when the
    /// host declares one. `min_reader`/`min_writer` are advisory without
    /// it; with it, a document whose `min_reader` is above this version
    /// is refused with T007 (LANG-B-021). The C ABI does not carry a host
    /// version, so FFI hosts leave this `None` until a future ABI major.
    pub reader_version: Option<String>,
    /// Read-only taint probe: materialise payloads but do not charge
    /// the per-alias payload copy. Analysis surfaces (inspect, taint
    /// queries, audit/lock hashing) need the bytes to notice a
    /// payload-resident secret but never emit them, so alias copies
    /// must not fail a document that exported within its budget.
    pub taint_probe: bool,
    /// Salt for TAINTED blob identities (spec/blob.md section 1, LANG-C-010).
    /// `None` (the default) derives a per-document salt from the document's
    /// shape, so identities are reproducible for a given input; a host that
    /// wants a per-process random salt sets one here. Untainted blobs always
    /// use the v1 domain and this value does not touch them.
    pub blob_salt: Option<[u8; 32]>,
    /// Receiver-side interchange gate (spec/agent-interchange-profile.md
    /// §4, LANG-C-004): refuses an unknown envelope major and
    /// capabilities this host does not offer BEFORE any evaluation work.
    /// Default-on with the shipped policy; `None` disables it, which a
    /// receiver must do only for documents it already trusts (the gate is
    /// the negotiation layer, not a validation nicety).
    pub envelope_gate: Option<crate::envelope::EnvelopeGate>,
    /// Trusted keys for v2 sender signatures (L-T06 review H4). When a
    /// sender carries a key/signature and this is `None`, evaluation
    /// refuses (E045): an unverifiable signature is never accepted.
    pub envelope_trust: Option<crate::sign::TrustedKeys>,
    /// L-P28: a closure guard set only by `Sandbox`; external code cannot
    /// construct a guarded engine.
    pub(crate) sandbox_guard: Option<crate::sandbox::SandboxGuard>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// Version floors (spec/schema-evolution.md §1): when the host declares
/// its reader version, a document whose `min_reader` is above it is
/// refused with T007. Without a declared version the floors stay
/// advisory (LANG-B-021).
fn check_reader_floor(module: &Module, reader: &str) -> Result<(), EvalError> {
    let Some(root) = module.schema else {
        return Ok(());
    };
    let Some(Node::SchemaTable { meta, .. }) = module.get(root) else {
        return Ok(());
    };
    let Some(floor) = meta.min_reader.as_deref() else {
        return Ok(());
    };
    if !version_at_least(reader, floor) {
        return Err(EvalError::ValidationFailed {
            diagnostics: vec![Diagnostic {
                severity: Severity::Error,
                code: "T007".into(),
                message: format!("document requires reader {floor}; this reader is {reader}"),
                path: None,
                span: None,
                repair: None,
                confidence: 1.0,
                file: None,
            }],
        });
    }
    Ok(())
}

/// Whether `have` satisfies the `need` floor, comparing numeric triplets
/// component-wise. Unparseable input fails closed (the caller has
/// already validated metadata syntax, so this is defense in depth).
pub(crate) fn version_at_least(have: &str, need: &str) -> bool {
    fn parts(v: &str) -> Option<(u64, u64, u64)> {
        let mut it = v.split('.');
        let a = it.next()?.parse().ok()?;
        let b = it.next()?.parse().ok()?;
        let c = it.next()?.parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some((a, b, c))
    }
    match (parts(have), parts(need)) {
        (Some(h), Some(n)) => h >= n,
        _ => false,
    }
}

/// Iterative result-depth check for patch-composed trees
/// (spec/budgets.md §2, C-037). Explicit stack (never recursion):
/// patch paths are input-proportioned, but the composed depth is not
/// statically bounded. Runs only when patches ran (profiles merge
/// key-wise and cannot deepen).
fn check_result_depth(result: &Value, max_depth: u64) -> Result<(), EvalError> {
    let mut stack: Vec<(&Value, u64)> = vec![(result, 0)];
    while let Some((value, depth)) = stack.pop() {
        if depth > max_depth {
            return Err(EvalError::BudgetExceeded {
                kind: BudgetKind::Depth,
                limit: max_depth,
                warnings: Vec::new(),
                span: None,
                file: None,
            });
        }
        match value {
            Value::Table(map) => {
                for v in map.values() {
                    stack.push((v, depth + 1));
                }
            }
            Value::Array(elements) => {
                for v in elements {
                    stack.push((v, depth + 1));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Context for the post-walk transforms (patch sets, profile merges). It
/// groups the arguments those passes share so no pass grows past the
/// argument-count lint: the demand `cone` a patch set must respect, the
/// effective `budget` a profile merge enforces, and the `profile` plus
/// closed `env` a material `String`'s report size needs — so a transform
/// charge can never reveal a secret's length (C2).
#[derive(Clone, Copy)]
struct TransformCtx<'a> {
    cone: Option<&'a str>,
    budget: Budget,
    profile: Option<&'a SecretProfile>,
    env: &'a HashMap<String, String>,
}

#[cfg(test)]
fn test_ctx() -> TransformCtx<'static> {
    // The report measure reads `env` only when a secret profile is present,
    // so the unit tests, which analyse no profile, share one empty snapshot.
    static ENV: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
    TransformCtx {
        cone: None,
        budget: Budget::default(),
        profile: None,
        env: ENV.get_or_init(HashMap::new),
    }
}

/// Cumulative materialization account threaded through post-walk
/// transforms (patch sets, profile merges, default fills). The DP
/// pass seeds it with the arena total; each transform charges what
/// it clones. Trips X006 past the total limit.
pub(crate) struct Accountant {
    spent: u64,
    /// Report-side total: what the walk charged for material nodes sized at
    /// `secret::MARKER_SIZE` instead of their byte length. `spent` stays the
    /// real-byte balance that enforces X006 (clamping it would open a
    /// clone-amplification hole); `reported` is what surfaces print, so a
    /// `ChargeReport` cannot reveal how long a secret is (C2).
    reported: u64,
    limit: u64,
    /// Wall-clock sampling on materialised bytes (LANG-C-006). The walk
    /// samples the deadline every 64 nodes, which few-node/huge-work
    /// documents never reach (an 8-level environment doubling has 27
    /// nodes), so the clock also fires after every `WALL_SAMPLE_BYTES`
    /// charged bytes - whichever comes first.
    deadline: Option<Instant>,
    wall_limit: u64,
    bytes_since_sample: u64,
}

/// Wall budget for canonical (identity) and analysis passes (LANG-C-008).
///
/// These passes exist to produce a reproducible artifact (audit/lock hash,
/// replay steps, minimizer verdicts, language-server steps), so they cannot
/// inherit a caller's wall clock: a document that just passed under a
/// tighter limit would fail the second pass on machine load alone. They
/// must NOT run without a clock either - that was the hole LANG-C-008
/// closed: with `max_wall_ms = 0` the amp fixtures returned multi-megabyte
/// outputs with no time bound at all. Every such surface uses
/// [`canonical_budget`], which pins this single finite value, so the pass
/// is reproducible (same deadline everywhere) and still bounded.
pub const CANONICAL_WALL_MS: u64 = 30_000;

/// Budget for a canonical/analysis pass: every limit from `base` except the
/// wall clock, which becomes [`CANONICAL_WALL_MS`]. A caller's `0`
/// (unlimited) is replaced too, so no surface runs unbounded.
pub fn canonical_budget(base: &Budget) -> Budget {
    // A canonical pass needs a bound in every dimension, not only time: a
    // caller's `UNLIMITED` counts/sizes would let a huge arena exhaust
    // memory before the wall clock fires (review finding on LANG-C-008).
    // Tighter caller limits are kept; only "unlimited" becomes the shipped
    // default.
    let shipped = Budget::default();
    let bound = |v: u64, d: u64| if v == UNLIMITED { d } else { v };
    Budget {
        max_nodes: bound(base.max_nodes, shipped.max_nodes),
        max_depth: bound(base.max_depth, shipped.max_depth),
        max_array_len: bound(base.max_array_len, shipped.max_array_len),
        max_keys: bound(base.max_keys, shipped.max_keys),
        max_wall_ms: CANONICAL_WALL_MS,
        max_value_size: bound(base.max_value_size, shipped.max_value_size),
        max_total_value_size: bound(base.max_total_value_size, shipped.max_total_value_size),
    }
}

/// Charged bytes between wall-clock samples: 1 MiB. One clock read per MiB
/// is far below the cost of materialising it (~10 us against ~250 us of
/// memcpy), and it bounds how far past the deadline a byte-heavy walk can
/// run to a single MiB of work.
pub const WALL_SAMPLE_BYTES: u64 = 1 << 20;

impl Accountant {
    /// Charge a non-material amount: the enforcement and report balances move
    /// together. Material charges go through [`Self::charge_scoped`] with a
    /// marker-sized report.
    pub(crate) fn charge(
        &mut self,
        module: &Module,
        node: Option<NodeId>,
        amount: u64,
    ) -> Result<(), EvalError> {
        self.charge_scoped(module, node, amount, amount)
    }

    /// Charge `amount` to the X006 enforcement balance and `report` to the
    /// printed total. For a material node the caller passes `amount =` its
    /// real byte length and `report =` `secret::MARKER_SIZE`, so the report
    /// never varies with the secret's length while enforcement stays exact.
    pub(crate) fn charge_scoped(
        &mut self,
        module: &Module,
        node: Option<NodeId>,
        amount: u64,
        report: u64,
    ) -> Result<(), EvalError> {
        self.spent = self.spent.saturating_add(amount);
        self.reported = self.reported.saturating_add(report);
        // `>= limit + 1` (not `> limit`): identical ordinarily, and
        // still correct at `u64::MAX`, where the saturating cap below
        // would otherwise go blind.
        if self.limit != 0 && self.spent >= self.limit.saturating_add(1) {
            return Err(EvalError::budget_exceeded(
                BudgetKind::TotalSize,
                self.limit,
                module,
                node,
            ));
        }
        // Byte-driven wall-clock sample (LANG-C-006): the caller charges
        // BEFORE materialising, so a trip here fires before the allocation
        // it would have paid for.
        if let Some(end) = self.deadline {
            self.bytes_since_sample = self.bytes_since_sample.saturating_add(amount);
            if self.bytes_since_sample >= WALL_SAMPLE_BYTES {
                self.bytes_since_sample = 0;
                if Instant::now() >= end {
                    return Err(EvalError::budget_exceeded(
                        BudgetKind::WallClock,
                        self.wall_limit,
                        module,
                        node,
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Data Mode progressive warnings (PROPOSAL §0.2, Phase M): at 80% of any
/// nonzero limit except wall-clock (which has no static bound to warn on -
/// documented, not an omission), warn that the hard ceiling (which never
/// moves) is approaching. Informational only - evaluation continues and the
/// same ceilings bind both modes.
///
/// Split out of the walk so an aborted evaluation can re-derive the same
/// list for the error it returns (LANG-C-009): the walk replaces its
/// diagnostics with a budget error, and an agent watching W013 as its
/// "approaching a ceiling" signal must still see it on a slow host.
fn progressive_warnings(module: &Module, budget: &Budget) -> Vec<Diagnostic> {
    if module.mode != Mode::Data {
        return Vec::new();
    }
    let Ok(order) = toposort(&module.graph, None) else {
        return Vec::new();
    };
    let Ok((dp_total, dp_bounds, _dp_depth, dp_max_depth)) =
        check_dp_bounds(module, &order, budget)
    else {
        return Vec::new();
    };
    let max_node_size = dp_bounds.iter().copied().max().unwrap_or(0);
    let warn_at = |used: u64, limit: u64| limit != 0 && (used as u128) * 5 >= (limit as u128) * 4;
    // Static shape scan (same counting as `check_static_budgets`): longest
    // array and total value-table keys for the progressive warnings.
    let mut keys = 0u64;
    let mut max_arr = 0u64;
    for node in &module.arena {
        match node {
            Node::Table { fields } => keys += fields.len() as u64,
            Node::Array { elements } => max_arr = max_arr.max(elements.len() as u64),
            _ => {}
        }
    }
    let mut diagnostics = Vec::new();
    let mut warn = |kind: BudgetKind, used: u64, limit: u64| {
        if warn_at(used, limit) {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                code: "W013".into(),
                message: format!(
                    "approaching {} ({} of limit {}); hard ceilings apply in Data Mode exactly as in Config",
                    kind, used, limit
                ),
                path: None,
                span: None,
                repair: None,
                confidence: 1.0,
                file: None,
            });
        }
    };
    warn(
        BudgetKind::Nodes,
        module.arena.len() as u64,
        budget.max_nodes,
    );
    warn(BudgetKind::ValueSize, max_node_size, budget.max_value_size);
    warn(BudgetKind::Depth, dp_max_depth, budget.max_depth);
    warn(BudgetKind::TotalSize, dp_total, budget.max_total_value_size);
    warn(BudgetKind::ArrayLength, max_arr, budget.max_array_len);
    warn(BudgetKind::Keys, keys, budget.max_keys);
    diagnostics
}

/// Carry the progressive warnings a trip would otherwise discard
/// (LANG-C-009).
fn attach_progressive_warnings(module: &Module, budget: &Budget, error: EvalError) -> EvalError {
    match error {
        EvalError::BudgetExceeded {
            kind,
            limit,
            warnings,
            span,
            file,
        } if warnings.is_empty() => EvalError::BudgetExceeded {
            kind,
            limit,
            warnings: progressive_warnings(module, budget),
            span,
            file,
        },
        other => other,
    }
}

/// Budget context threaded through validation (spec/budgets.md §2):
/// the shared evaluation deadline, its configured limit for messages,
/// and the cumulative materialization account. Bundled so validation
/// entry points stay under the argument-count lint.
///
/// `material` and `env` are the runtime-aware secret analysis and the
/// engine's environment snapshot. A schema default the validator fills
/// is charged for enforcement at its real size and for the report at its
/// redaction-safe size, so a filled default that copies a secret never
/// varies the printed `charge.total` with the secret's length (L-P40
/// §3.4, C2). Both are `None`/empty only when no analysis ran.
pub(crate) struct BudgetCx<'a> {
    pub deadline: Option<Instant>,
    pub wall_limit: u64,
    pub account: &'a mut Accountant,
    pub material: Option<&'a SecretProfile>,
    pub env: &'a HashMap<String, String>,
}

/// Iterative value-size measure in DP units (scalars 1, strings and
/// secrets plus their bytes, composites 1 plus children). Matches
/// `check_dp_bounds` semantics for transform-site accounting.
/// Cap on E046 diagnostics from deferred merges in one evaluation. The
/// merge itself still runs for every entry; only the report is bounded.
const MAX_MERGE_DIAGNOSTICS: usize = 1_024;

/// Resolve one scope's deferred same-priority merges (L-P09). Equal
/// values agree; a disagreement is E046 and keeps the first source in
/// source order; tables union. Entries sharing a path are folded once in
/// candidate order, so a three-way union never discards a prior pair's
/// keys. Parse-time shape choices stay in place until this runs, so
/// references and provenance see one value.
#[allow(clippy::too_many_arguments)]
fn apply_pending_merges(
    module: &Module,
    scope: NodeId,
    pending: &[crate::unify::PendingMerge],
    map: &mut indexmap::IndexMap<String, Value>,
    values: &HashMap<NodeId, Value>,
    diagnostics: &mut Vec<Diagnostic>,
    account: &mut Accountant,
    deadline: Option<std::time::Instant>,
    budget: &Budget,
    profile: Option<&SecretProfile>,
    env: &HashMap<String, String>,
    reported: &mut usize,
    cone: Option<&str>,
) -> Result<(), EvalError> {
    // Merge all entries for a path into one candidate list (deduplicated
    // by node), preserving source order across the parser's pairs.
    let mut grouped: indexmap::IndexMap<String, (String, Vec<crate::unify::MergeCandidate>)> =
        indexmap::IndexMap::new();
    let mut seen: indexmap::IndexMap<String, std::collections::HashSet<NodeId>> =
        indexmap::IndexMap::new();
    for entry in pending {
        // L-P40 §2.2.3: a merge outside the cone writes to another path
        // and cannot change a demanded value, so a cone never runs it.
        // Membership is scoped by the full-document `path`: `local` is
        // relative to the scope table, so a nested merge (`local = "t.a"`,
        // `path = "s.t.a"`) would never match the cone `s`. A merge into
        // the envelope always runs: the envelope is always part of the cone
        // (it gates E035), so skipping it would leave the gate reading an
        // unmerged envelope (§2.2.4).
        if cone.is_some_and(|cone| {
            !demand::path_in_cone(&entry.path, cone)
                && !crate::envelope::is_envelope_path(&entry.path)
        }) {
            continue;
        }
        if let Some(end) = deadline {
            if std::time::Instant::now() >= end {
                return Err(EvalError::budget_exceeded(
                    BudgetKind::WallClock,
                    budget.max_wall_ms,
                    module,
                    None,
                ));
            }
        }
        let slot = grouped
            .entry(entry.local.clone())
            .or_insert_with(|| (entry.path.clone(), Vec::new()));
        let known = seen.entry(entry.local.clone()).or_default();
        for candidate in &entry.candidates {
            if known.insert(candidate.node) {
                slot.1.push(candidate.clone());
            }
        }
    }
    for (local, (path, candidates)) in grouped {
        // `depends_on` is extracted at parse and never exported. Skip
        // only an actual extracted metadata key; a data key with the same
        // name must still conflict normally.
        let metadata_depends_on = match local.rsplit_once('.') {
            Some((parent, "depends_on")) => scope_node_at(module, scope, parent)
                .is_some_and(|parent_id| module.depends_on.contains_key(&parent_id)),
            None => local == "depends_on" && module.depends_on.contains_key(&scope),
            _ => false,
        };
        if metadata_depends_on {
            continue;
        }
        // A local body replaced an include-derived value here (or at an
        // ancestor): the local value wins and the include pair is not an
        // observable conflict, even when the values happen to agree.
        if module.replaced_paths.iter().any(|replaced| {
            path == *replaced
                || (path.len() > replaced.len()
                    && path.starts_with(replaced.as_str())
                    && path.as_bytes()[replaced.len()] == b'.')
        }) {
            continue;
        }
        // Engine ceilings apply to merges too; the fixed merge defaults
        // are only fallbacks for a host that disabled a limit.
        let merge_limits = crate::unify::MergeLimits {
            max_depth: if budget.max_depth == 0 {
                crate::unify::MAX_MERGE_DEPTH
            } else {
                (budget.max_depth as usize).min(crate::unify::MAX_MERGE_DEPTH)
            },
            max_nodes: if budget.max_nodes == 0 {
                crate::unify::MAX_MERGE_NODES
            } else {
                budget.max_nodes.min(crate::unify::MAX_MERGE_NODES)
            },
            max_bytes: if budget.max_total_value_size == 0 {
                crate::unify::MAX_MERGE_BYTES
            } else {
                budget
                    .max_total_value_size
                    .min(crate::unify::MAX_MERGE_BYTES)
            },
        };
        let mut candidates = candidates.iter();
        let Some(first) = candidates.next() else {
            continue;
        };
        let live = value_at_path(map, &local).cloned();
        let tree_node = scope_node_at(module, scope, &local);
        let base = match (tree_node, live) {
            // The kept candidate is still the tree node: fold onto it.
            (Some(node), Some(value)) if node == first.node => Some(value),
            // A different node occupies the path. If its value equals the
            // first candidate (an earlier body with the same value), the
            // merge is still live and its later candidates must be
            // compared; a genuinely different value means a higher
            // priority or local replacement took the path.
            (Some(_), Some(value)) => match values.get(&first.node) {
                Some(first_value) if crate::unify::merge_agree(&value, first_value) => Some(value),
                _ => None,
            },
            (Some(_), None) => values.get(&first.node).cloned(),
            // No node at the path: extracted metadata or a malformed
            // host-built module. Fail closed.
            (None, _) => None,
        };
        let Some(mut current) = base else {
            if *reported < MAX_MERGE_DIAGNOSTICS {
                let message = if tree_node.is_none() {
                    format!("merge target '{path}' has no evaluated value")
                } else {
                    format!("merge candidate at '{path}' has no evaluated value")
                };
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "E046".into(),
                    message,
                    path: Some(path),
                    span: module.node_spans.get(&first.node).cloned(),
                    file: module.node_files.get(&first.node).cloned(),
                    repair: None,
                    confidence: 1.0,
                });
                *reported += 1;
            }
            continue;
        };
        let group_material =
            profile.is_some_and(|p| p.node_is_secret(first.node) || p.path_is_secret(&path));
        // Material is never compared: redact material leaves first, so
        // unify sees a Secret (never equal) instead of the plaintext.
        if group_material {
            current = redact_material_value(&current);
        }
        for candidate in candidates {
            if let Some(end) = deadline {
                if std::time::Instant::now() >= end {
                    return Err(EvalError::budget_exceeded(
                        BudgetKind::WallClock,
                        budget.max_wall_ms,
                        module,
                        Some(candidate.node),
                    ));
                }
            }
            let Some(next) = values.get(&candidate.node) else {
                if *reported < MAX_MERGE_DIAGNOSTICS {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "E046".into(),
                        message: format!("merge candidate at '{path}' has no evaluated value"),
                        path: Some(path.clone()),
                        span: module.node_spans.get(&candidate.node).cloned(),
                        file: module.node_files.get(&candidate.node).cloned(),
                        repair: None,
                        confidence: 1.0,
                    });
                    *reported += 1;
                }
                continue;
            };
            let candidate_material =
                group_material || profile.is_some_and(|p| p.node_is_secret(candidate.node));
            let redacted;
            let next = if candidate_material {
                redacted = redact_material_value(next);
                &redacted
            } else {
                next
            };
            let mut provenance = MergeProvenance::new();
            match crate::unify::unify_with_limits(
                &current,
                &first.source,
                next,
                &candidate.source,
                &path,
                &mut provenance,
                &merge_limits,
            ) {
                Ok(merged) => {
                    crate::unify::record(&provenance);
                    current = merged;
                }
                Err(error @ MergeError::Conflict(_)) => {
                    // Expose the contested path even though the merge
                    // kept the first source: winner = first, loser = second.
                    let mut contested = MergeProvenance::new();
                    contested.record_conflict(&path, &first.source, &candidate.source);
                    crate::unify::record(&contested);
                    if *reported < MAX_MERGE_DIAGNOSTICS {
                        let mut diagnostic = error.to_diagnostic();
                        diagnostic.span = module.node_spans.get(&candidate.node).cloned();
                        diagnostic.file = module.node_files.get(&candidate.node).cloned();
                        diagnostics.push(diagnostic);
                        *reported += 1;
                    }
                }
                Err(MergeError::Budget(budget)) => {
                    let kind = match budget.kind {
                        MergeBudgetKind::Depth => BudgetKind::Depth,
                        MergeBudgetKind::Nodes => BudgetKind::Nodes,
                        MergeBudgetKind::Bytes => BudgetKind::TotalSize,
                    };
                    return Err(EvalError::budget_exceeded(
                        kind,
                        budget.limit,
                        module,
                        Some(candidate.node),
                    ));
                }
            }
        }
        // Measure the subtree twice: enforcement bytes and the
        // redaction-safe report size. They differ only when the merged
        // value carries material, which is exactly when the report must not
        // reveal the secret's length (C2); without material the two agree and
        // the printed total is unchanged.
        let measure_pair = |v: &Value| -> (u64, u64) {
            let real = measure_merge_value(v);
            let report = profile.map_or(real, |p| measure_report_value(v, p, env));
            (real, report)
        };
        let before = value_at_path(map, &local)
            .map(&measure_pair)
            .unwrap_or((0, 0));
        if set_value_at(map, &local, current) {
            let after = value_at_path(map, &local)
                .map(&measure_pair)
                .unwrap_or((0, 0));
            if budget.max_value_size != 0 && after.0 > budget.max_value_size {
                return Err(EvalError::budget_exceeded(
                    BudgetKind::ValueSize,
                    budget.max_value_size,
                    module,
                    Some(scope),
                ));
            }
            account.charge_scoped(
                module,
                Some(scope),
                after.0.saturating_sub(before.0),
                after.1.saturating_sub(before.1),
            )?;
        } else if *reported < MAX_MERGE_DIAGNOSTICS {
            // A host-built module can name a path the tree does not
            // contain; dropping the merged value would be fail-open.
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "E046".into(),
                message: format!("merge target '{path}' is missing from the composed tree"),
                path: Some(path),
                span: module.node_spans.get(&first.node).cloned(),
                file: module.node_files.get(&first.node).cloned(),
                repair: None,
                confidence: 1.0,
            });
            *reported += 1;
        }
    }
    Ok(())
}

/// Resolve a scope-relative dotted path to the node currently placed in
/// that scope's table fields (AST identity, not values).
fn scope_node_at(module: &Module, scope: NodeId, path: &str) -> Option<NodeId> {
    let Some(Node::Table { fields }) = module.get(scope) else {
        return None;
    };
    let mut segments = path.split('.');
    let first = segments.next()?;
    let mut node = *fields.get(first)?;
    for segment in segments {
        match module.get(node) {
            Some(Node::Table { fields }) => node = *fields.get(segment)?,
            _ => return None,
        }
    }
    Some(node)
}

fn value_at_path<'a>(map: &'a indexmap::IndexMap<String, Value>, path: &str) -> Option<&'a Value> {
    let mut segments = path.split('.');
    let first = segments.next()?;
    let mut value = map.get(first)?;
    for segment in segments {
        let Value::Table(child) = value else {
            return None;
        };
        value = child.get(segment)?;
    }
    Some(value)
}

/// Replace the value at a scope-relative dotted path. The parser keeps
/// the path in the tree, so a missing segment means the path was pruned
/// by an earlier transform; leave the tree alone.
fn set_value_at(map: &mut indexmap::IndexMap<String, Value>, path: &str, value: Value) -> bool {
    let mut segments = path.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    let rest: Vec<&str> = segments.collect();
    if rest.is_empty() {
        // Never insert: a missing key means the path was extracted
        // metadata or already replaced, and inventing it would re-export
        // reserved keys (depends_on) or values the tree dropped.
        return match map.get_mut(first) {
            Some(slot) => {
                *slot = value;
                true
            }
            None => false,
        };
    }
    let Some(Value::Table(child)) = map.get_mut(first) else {
        return false;
    };
    set_value_at(child, &rest.join("."), value)
}

/// Measure a value.
pub(crate) fn measure_value(value: &Value) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        total = total.saturating_add(1);
        match v {
            Value::String(s) => {
                total = total.saturating_add(s.len() as u64);
            }
            Value::Secret(s) => {
                total = total.saturating_add(s.len() as u64);
            }
            // Identity leaf of known size (spec/blob.md §6): the
            // payload is charged at materialisation, not here.
            Value::Blob { size, .. } => {
                total = total.saturating_add(*size);
            }
            Value::Array(items) => stack.extend(items.iter()),
            Value::Table(map) => stack.extend(map.values()),
            _ => {}
        }
    }
    total
}

/// Merge-budget size: material costs a constant, never its byte length,
/// so X006 outcomes cannot reveal a secret's size.
fn measure_merge_value(value: &Value) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        total = total.saturating_add(1);
        match v {
            Value::String(s) => {
                total = total.saturating_add(s.len() as u64);
            }
            // Constant: secret length is material.
            Value::Secret(_) => {}
            Value::Blob { size, .. } => {
                total = total.saturating_add(*size);
            }
            Value::Array(items) => stack.extend(items.iter()),
            Value::Table(map) => stack.extend(map.values()),
            _ => {}
        }
    }
    total
}

/// Whether `text` carries a runtime material env value. The profile knows
/// authored literals and material env *names*, but an env *value* is only
/// known at evaluation; the engine's closed snapshot is the one place it
/// lives (the same rule the blob arm applies to a copied payload).
fn text_carries_material_env(
    text: &str,
    profile: &SecretProfile,
    env: &HashMap<String, String>,
) -> bool {
    profile.material_envs().iter().any(|name| {
        env.get(name)
            .is_some_and(|value| !value.is_empty() && text.contains(value))
    })
}

/// Report-side value size (L-P40 §3.4, C2): like [`measure_merge_value`],
/// material costs one constant so a printed `charge.total` cannot reveal a
/// secret's length, but stricter — it also redacts a `String` that equals a
/// secret literal, a `String` that embeds a material env value, and a
/// tainted `Blob`'s plaintext size, the three places [`measure_merge_value`]
/// still counts material bytes. `profile` is the same analysis the redacting
/// surfaces use, so a byte the report hides is a byte every other surface
/// hides too.
pub(crate) fn measure_report_value(
    value: &Value,
    profile: &SecretProfile,
    env: &HashMap<String, String>,
) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        total = total.saturating_add(1);
        match v {
            // A whole-value copy of a secret redacts (exact match, at any
            // length), and a string that embeds a material env value is a
            // copy of secret bytes; cost the marker, never the length.
            Value::String(s) => {
                if profile.redact_text(s).is_some() || text_carries_material_env(s, profile, env) {
                    total = total.saturating_add(crate::secret::MARKER_SIZE);
                } else {
                    total = total.saturating_add(s.len() as u64);
                }
            }
            Value::Secret(_) => {
                total = total.saturating_add(crate::secret::MARKER_SIZE);
            }
            // A tainted blob never retains a payload; its plaintext size is
            // material, so report the marker instead of the byte count.
            Value::Blob { size, tainted, .. } => {
                if *tainted {
                    total = total.saturating_add(crate::secret::MARKER_SIZE);
                } else {
                    total = total.saturating_add(*size);
                }
            }
            Value::Array(items) => stack.extend(items.iter()),
            Value::Table(map) => stack.extend(map.values()),
            _ => {}
        }
    }
    total
}

/// The report-side charge for a node whose enforcement charge is `amount`:
/// the node's redaction-safe size minus its clamped DP bound, which is the
/// enforcement delta with every material byte replaced by the marker. When
/// the document carries no material the report equals enforcement, so the
/// helper returns `amount` unchanged and the walk's hot path skips it.
fn report_amount(
    has_material: bool,
    reported_sizes: &[u64],
    report_bounds: &[u64],
    idx: usize,
    amount: u64,
) -> u64 {
    if !has_material {
        return amount;
    }
    match (reported_sizes.get(idx), report_bounds.get(idx)) {
        (Some(size), Some(bound)) => size.saturating_sub(*bound),
        _ => amount,
    }
}

/// Whether the walk must report the node at arena index `idx` at the
/// marker: the report flags it (secret kind, secret-equal literal, material
/// path) or it is an environment read of a name the profile marks material
/// — the same env rule the redacting surfaces use, so the report hides
/// every byte they hide.
fn node_reports_material(
    module: &Module,
    idx: usize,
    report_material: &[bool],
    profile: Option<&SecretProfile>,
) -> bool {
    if report_material.get(idx).copied().unwrap_or(false) {
        return true;
    }
    match (module.get(NodeId(idx as u32)), profile) {
        (Some(Node::Env { name, .. }), Some(profile)) => profile.env_is_material(name),
        _ => false,
    }
}

/// Report-side size of a node: the marker when it reports material, else
/// its real (redaction-safe) measure.
fn report_node_size(material: bool, real: u64) -> u64 {
    if material {
        crate::secret::MARKER_SIZE
    } else {
        real
    }
}

/// Replace every scalar leaf with a `Secret`, keeping container shape.
/// Used before comparing or exporting a merge group that is material:
/// a plaintext copy of a material Int/Bool/Quantity has no textual
/// marker, so the only safe representation is the secret marker itself.
fn redact_material_value(value: &Value) -> Value {
    match value {
        Value::Table(map) => Value::Table(
            map.iter()
                .map(|(key, child)| (key.clone(), redact_material_value(child)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_material_value).collect()),
        Value::Secret(_) => value.clone(),
        // A blob at a material path is incomparable, not an identity to
        // compare: comparing its (untainted) identity would answer
        // whether two material payloads are equal.
        Value::Blob { .. } => Value::Secret(zeroize::Zeroizing::new(String::new())),
        _ => Value::Secret(zeroize::Zeroizing::new(String::new())),
    }
}

/// Static budget pre-checks (spec/budgets.md §2, C-037): arena-wide
/// sums, span-attributed where a single node offends. Over-counting
/// unreachable nodes is fail-safe. Runs before the walk: monsters
/// fail fast. Depth and materialized size need reference-aware
/// longest-path bounds — those run as `check_dp_bounds` over the
/// topological order instead (a plain roots walk would both miss
/// reference expansion and report nondeterministic spans).
fn check_static_budgets(module: &Module, budget: &Budget) -> Result<(), EvalError> {
    let over = |limit: u64, used: u64| limit != 0 && used > limit;
    if over(budget.max_nodes, module.arena.len() as u64) {
        return Err(EvalError::BudgetExceeded {
            kind: BudgetKind::Nodes,
            limit: budget.max_nodes,
            warnings: Vec::new(),
            span: None,
            file: None,
        });
    }
    if budget.max_array_len != 0 || budget.max_keys != 0 {
        let mut keys = 0u64;
        for (i, node) in module.arena.iter().enumerate() {
            let id = NodeId(i as u32);
            match node {
                // Value tables only: schema tables are metadata with
                // their own (parser-side) limits.
                Node::Table { fields } => {
                    keys += fields.len() as u64;
                    if over(budget.max_keys, keys) {
                        return Err(EvalError::BudgetExceeded {
                            kind: BudgetKind::Keys,
                            limit: budget.max_keys,
                            warnings: Vec::new(),
                            span: None,
                            file: None,
                        });
                    }
                }
                Node::Array { elements } if over(budget.max_array_len, elements.len() as u64) => {
                    return Err(EvalError::budget_exceeded(
                        BudgetKind::ArrayLength,
                        budget.max_array_len,
                        module,
                        Some(id),
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// One materialization-bound pass (`spec/budgets.md` §2, C-037).
/// `size[i]` bounds the value at node `i` in materialized data units
/// (scalars plus string bytes; each `Ref` clone costs its target's
/// bound — this kills exponential reference expansion before anything
/// materializes), `depth[i]` bounds its nesting through references,
/// `total` is the sum of the `size` bounds this pass counted, and
/// `max_depth` is the deepest bound seen (drives Data Mode progressive
/// warnings).
struct DpBounds {
    total: u64,
    size: Vec<u64>,
    depth: Vec<u64>,
    max_depth: u64,
}

/// Which children one bound pass counts, and whether it enforces the
/// ceilings.
///
/// `Whole` counts every child and trips the budget the moment a bound
/// crosses a limit: the whole-document static phase, which runs for
/// every demand. `Closure(members)` counts a container's children only
/// when they are members of the demanded set and never trips — it runs
/// only for a cone, after the `Whole` pass has already cleared the
/// whole document, and every closure-scoped bound is at most its
/// whole-document bound (a container sums a subset of its children, a
/// `Ref` copies its member target), so its total cannot cross a ceiling
/// the whole pass passed. That total seeds the cone accountant
/// (`spec/budgets.md` §2, L-P40 §4.2), so an undemanded sibling's
/// static weight never counts against the cone.
///
/// `WholeReport` counts every child like `Whole` but enforces nothing:
/// the L-P40 cost report recomputes the per-node bounds of a run that
/// already cleared the whole static phase, so this pass is total and
/// can never trip a ceiling the enforcing pass allowed.
enum DpScope<'a> {
    Whole,
    WholeReport,
    Closure(&'a [bool]),
}

impl DpScope<'_> {
    /// Whether this pass counts the child at arena index `child` toward
    /// its parent's bound, and materialises the node at arena index
    /// `child` at all. `Whole` and `WholeReport` count everything;
    /// `Closure` counts only members (a member's value dependencies are
    /// members too, so its bound reads only computed member bounds).
    fn counts(&self, child: usize) -> bool {
        match self {
            DpScope::Whole | DpScope::WholeReport => true,
            DpScope::Closure(members) => members.get(child).copied().unwrap_or(false),
        }
    }

    /// Whether this pass enforces the size/depth/total ceilings. Only
    /// the enforcing `Whole` pass trips a budget; `WholeReport` and
    /// `Closure` compute their bounds without one.
    fn enforces(&self) -> bool {
        matches!(self, DpScope::Whole)
    }
}

/// Reference-aware materialization bounds (spec/budgets.md §2,
/// C-037): one linear dynamic-programming pass over the topological
/// order (dependencies first, so every bound is final when read).
/// Both bounds saturate at `limit + 1` so the numbers stay tiny;
/// tripping either aborts with the offending node's span. Memoized per
/// node, so shared subtrees cost O(1) — the pass itself is linear. The
/// scope decides which children count and whether the ceilings are
/// enforced (see `DpScope`).
fn materialization_bounds(
    module: &Module,
    order: &[petgraph::graph::NodeIndex],
    budget: &Budget,
    scope: DpScope<'_>,
    clamp: Option<&[bool]>,
) -> Result<DpBounds, EvalError> {
    if budget.max_value_size == 0 && budget.max_depth == 0 && budget.max_total_value_size == 0 {
        // No size bound to compute (every ceiling disabled), but a cost
        // report still clamps material nodes so it carries no length.
        let mut size = vec![1u64; module.arena.len()];
        if let Some(clamp) = clamp {
            for (i, material) in clamp.iter().enumerate() {
                if *material && i < size.len() {
                    size[i] = crate::secret::MARKER_SIZE;
                }
            }
        }
        return Ok(DpBounds {
            total: 0,
            size,
            depth: vec![0u64; module.arena.len()],
            max_depth: 0,
        });
    }
    // Separate saturation caps: comparing against `limit + 1` keeps
    // numbers tiny, and `>= cap` (instead of `> limit`) stays correct
    // even at `u64::MAX`, where `limit + 1` saturates. A disabled
    // limit (0) caps at `u64::MAX` so accounting stays exact while
    // its trip check is skipped.
    let size_cap = if budget.max_value_size == 0 {
        u64::MAX
    } else {
        budget.max_value_size.saturating_add(1)
    };
    let depth_cap = if budget.max_depth == 0 {
        u64::MAX
    } else {
        budget.max_depth.saturating_add(1)
    };
    let mut size = vec![1u64; module.arena.len()];
    let mut depth = vec![0u64; module.arena.len()];
    let sat_size = |a: u64, b: u64| a.saturating_add(b).min(size_cap);
    // Cumulative materialization across the whole pass: per-node
    // bounds cap each clone, this caps their sum (many aliases of one
    // large value trip here). Over-approximates (unreachable nodes
    // count) in the fail-safe direction.
    let mut total = 0u64;
    let mut max_depth_seen = 0u64;
    for node_idx in order {
        let id = module.graph[*node_idx];
        let i = id.0 as usize;
        if i >= size.len() || !scope.counts(i) {
            continue;
        }
        // Child bounds: table/array fields and elements, reference
        // targets (the clone!), interpolation parts, and composite
        // operands. Expression-only children are parser-capped, but
        // counting them is harmless over-approximation in the safe
        // direction.
        let mut child_size = 0u64;
        let mut child_depth = 0u64;
        match module.get(id) {
            Some(Node::Table { fields }) => {
                for child in fields.values() {
                    let c = child.0 as usize;
                    if c < size.len() && scope.counts(c) {
                        child_size = sat_size(child_size, size[c]);
                        child_depth = child_depth.max(depth[c]);
                    }
                }
                // Deferred merges are charged when they run (true union
                // growth), not here: the first candidate is already a
                // field, and summing candidates would double-count and
                // refuse valid equal-value merges.
                size[i] = sat_size(1, child_size);
                depth[i] = child_depth.saturating_add(1).min(depth_cap);
            }
            Some(Node::Array { elements }) => {
                for child in elements {
                    let c = child.0 as usize;
                    if c < size.len() && scope.counts(c) {
                        child_size = sat_size(child_size, size[c]);
                        child_depth = child_depth.max(depth[c]);
                    }
                }
                size[i] = sat_size(1, child_size);
                depth[i] = child_depth.saturating_add(1).min(depth_cap);
            }
            Some(Node::Ref(target)) => {
                let c = target.0 as usize;
                if c < size.len() && scope.counts(c) {
                    size[i] = size[c];
                    depth[i] = depth[c];
                }
            }
            Some(Node::Literal(Value::String(s))) => {
                size[i] = sat_size(1, s.len() as u64);
            }
            Some(Node::Interpolate { parts }) => {
                for part in parts {
                    match part {
                        InterpPart::Expr(e) => {
                            let c = e.0 as usize;
                            if c < size.len() && scope.counts(c) {
                                child_size = sat_size(child_size, size[c]);
                                child_depth = child_depth.max(depth[c]);
                            }
                        }
                        InterpPart::Literal(s) => {
                            child_size = sat_size(child_size, s.len() as u64);
                        }
                    }
                }
                size[i] = sat_size(1, child_size);
                // A nested interpolation is a real value-nesting step:
                // its rendered text embeds the child's value. Omitting
                // the depth let a long `${…}` chain recurse past the
                // budget and overflow the stack.
                depth[i] = child_depth.saturating_add(1).min(depth_cap);
            }
            Some(Node::Binary { lhs, rhs, .. }) => {
                for child in [*lhs, *rhs] {
                    let c = child.0 as usize;
                    if c < size.len() && scope.counts(c) {
                        child_size = sat_size(child_size, size[c]);
                    }
                }
                size[i] = sat_size(1, child_size);
            }
            Some(Node::Unary { expr, .. }) => {
                let c = expr.0 as usize;
                if c < size.len() && scope.counts(c) {
                    size[i] = sat_size(1, size[c]);
                }
            }
            // Transparent wrapper: the tag is tool treatment only, so
            // the bound is the inner value's bound exactly.
            Some(Node::Semantic { inner, .. }) => {
                let c = inner.0 as usize;
                if c < size.len() && scope.counts(c) {
                    size[i] = size[c];
                    depth[i] = depth[c];
                }
            }
            // Blob construction: identity metadata plus the full
            // payload bytes (children bounds are already computed:
            // graph edges guarantee topological order). Aliases
            // inherit this bound through the Ref arm, so N aliases
            // count N times toward the cumulative total.
            Some(Node::ContentAddressed { media_type, data }) => {
                let mut bound = 1u64;
                for child in [*media_type, *data] {
                    let c = child.0 as usize;
                    if c < size.len() && scope.counts(c) {
                        bound = sat_size(bound, size[c]);
                        child_depth = child_depth.max(depth[c]);
                    }
                }
                size[i] = bound;
                depth[i] = child_depth.saturating_add(1).min(depth_cap);
            }
            _ => {}
        }
        // L-P40 §3.2: the cost report must carry no material, so a node
        // whose own content is material is sized at the redacted marker
        // and every later parent aggregates that clamped child (children
        // precede parents in the topological order). Only the cost pass
        // passes `clamp`; the enforcing passes pass `None` and are
        // byte-identical to before.
        if let Some(clamp) = clamp {
            if clamp.get(i).copied().unwrap_or(false) {
                size[i] = crate::secret::MARKER_SIZE;
            }
        }
        // `>= cap` (not `> limit`): identical for ordinary limits,
        // and still correct at `u64::MAX`, where `limit + 1`
        // saturates and only a truly astronomical bound trips.
        if scope.enforces() && budget.max_value_size != 0 && size[i] >= size_cap {
            return Err(EvalError::budget_exceeded(
                BudgetKind::ValueSize,
                budget.max_value_size,
                module,
                Some(id),
            ));
        }
        if scope.enforces() && budget.max_depth != 0 && depth[i] >= depth_cap {
            return Err(EvalError::budget_exceeded(
                BudgetKind::Depth,
                budget.max_depth,
                module,
                Some(id),
            ));
        }
        total = total.saturating_add(size[i]);
        if scope.enforces()
            && budget.max_total_value_size != 0
            && total >= budget.max_total_value_size.saturating_add(1)
        {
            return Err(EvalError::budget_exceeded(
                BudgetKind::TotalSize,
                budget.max_total_value_size,
                module,
                Some(id),
            ));
        }
        max_depth_seen = max_depth_seen.max(depth[i]);
    }
    Ok(DpBounds {
        total,
        size,
        depth,
        max_depth: max_depth_seen,
    })
}

/// Reference-aware materialization bounds for the whole document
/// (`spec/budgets.md` §2, C-037): the `Whole` pass, which trips the
/// budget on the first bound that crosses a ceiling. Returns the
/// cumulative bound total, the per-node size bounds table (drives delta
/// accounting for dynamic values during the walk, and the L-P40 cost
/// model), the per-node depth bounds table, and the maximum value depth
/// bound (drives Data Mode progressive warnings).
fn check_dp_bounds(
    module: &Module,
    order: &[petgraph::graph::NodeIndex],
    budget: &Budget,
) -> Result<(u64, Vec<u64>, Vec<u64>, u64), EvalError> {
    let DpBounds {
        total,
        size,
        depth,
        max_depth,
    } = materialization_bounds(module, order, budget, DpScope::Whole, None)?;
    Ok((total, size, depth, max_depth))
}

type MemoConfig = (
    bool,
    Option<String>,
    Budget,
    bool,
    bool,
    Mode,
    bool,
    bool,
    crate::authority::CapabilitySet,
);

/// Memo store for incremental evaluation (C-055, spec/performance.md
/// §2): fingerprint → evaluated value plus the diagnostics produced
/// while evaluating it. Reused across `evaluate_incremental` calls.
/// Telemetry (`last_*`) refreshes every call. The store is bounded
/// (see the cap in `evaluate_incremental`): clearing it only loses
/// hits, never correctness.
///
/// Deliberately NOT `derive(Debug)`: the store holds evaluated
/// values (including plaintext intermediates), and a derived dump
/// would print document content into any log that formats the
/// memo. The manual impl below reports shape only — entry count
/// and telemetry. Secret-kind values additionally carry `Value`'s
/// own redacting `Debug` wherever else they are formatted.
pub struct IncrementalMemo {
    map: HashMap<u64, (Value, Vec<Diagnostic>)>,
    // The configuration token covers
    // (resilient, profile override, budget, materialise flag, taint
    // probe, mode, env-fallback presence): any configuration change
    // clears the store before the walk, so a flag flip or mode change
    // can never serve a stale tree (or a warning-less diagnostic set)
    // across modes.
    config: Option<MemoConfig>,
    salt: u64,
    pub last_eval_count: u64,
    pub last_memo_hits: u64,
}

impl IncrementalMemo {
    /// Number of memoized fingerprints. Telemetry only: the store is a
    /// performance cache, never an input to results.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// True when the store holds no fingerprints.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl std::fmt::Debug for IncrementalMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncrementalMemo")
            .field("entries", &self.map.len())
            .field("last_eval_count", &self.last_eval_count)
            .field("last_memo_hits", &self.last_memo_hits)
            .finish()
    }
}

impl Default for IncrementalMemo {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            config: None,
            salt: memo_salt(),
            last_eval_count: 0,
            last_memo_hits: 0,
        }
    }
}

/// One salt per memo (C-055 review): keeps fingerprint keys local to
/// the memo that minted them. `getrandom` failure falls back to a
/// fixed constant — uniqueness lost, correctness kept (a constant
/// salt is exactly the old unsalted behavior).
fn memo_salt() -> u64 {
    static MEMO_SEQ: AtomicU64 = AtomicU64::new(0);
    let mut buf = [0u8; 8];
    if getrandom::getrandom(&mut buf).is_ok() {
        u64::from_le_bytes(buf)
    } else {
        // No host entropy (wasm32-unknown-unknown): a per-memo sequence
        // keeps keys distinct within the process instead of one public
        // constant shared by every memo (LANG-I-013). Fingerprints are
        // in-memory keys, never persisted, and output determinism does
        // not depend on the salt.
        MEMO_SEQ.fetch_add(1, Ordering::Relaxed) ^ 0x9E37_79B9_7F4A_7C15
    }
}

/// FNV-1a 64-bit mixer for fingerprints (C-055 §2). Chosen over
/// SipHash (`DefaultHasher`) deliberately: per-node hasher
/// construction dominates a 10k walk, and FNV-1a folds bytes ~10×
/// cheaper with no allocation and no per-hasher setup. Implements
/// `std::hash::Hasher` so every structural piece hashes through one
/// path (`Hash` impls, discriminant bytes, float bit patterns) with
/// zero formatting. The 64-bit width is load-bearing — a collision
/// would serve a stale value — so fingerprints must never narrow.
/// `usize` lengths hash platform-wide; harmless because memos never
/// cross processes (fingerprints are in-memory keys, never
/// persisted), but they must never be persisted without review.
struct FpHasher(u64);

impl FpHasher {
    fn new(salt: u64) -> Self {
        // Per-memo salt: keys stay unique to the memo that minted
        // them, so a logged or leaked key is not a stable
        // cross-process oracle for short secret-derived values, and
        // any collision search starts over per memo. Output
        // determinism is unaffected — the salt only names keys.
        Self(14_695_981_039_346_656_037 ^ salt)
    }
}

impl std::hash::Hasher for FpHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(1_099_511_628_211);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Structural hash of a literal value (no formatting: discriminants,
/// integer/float bit patterns, and raw string bytes feed the mixer
/// directly). Floats hash by bit pattern — `-0.0` vs `0.0` or
/// differing NaN payloads fingerprint apart and simply miss the
/// memo, which is the safe direction. Table order is the binding
/// order, which evaluation preserves deterministically.
fn fp_value(hasher: &mut FpHasher, value: &Value) {
    use std::hash::Hash;
    match value {
        Value::Null => 0u8.hash(hasher),
        Value::Bool(flag) => {
            1u8.hash(hasher);
            flag.hash(hasher);
        }
        Value::Int(num) => {
            2u8.hash(hasher);
            num.hash(hasher);
        }
        Value::Float(num) => {
            3u8.hash(hasher);
            num.to_bits().hash(hasher);
        }
        Value::String(text) => {
            4u8.hash(hasher);
            text.hash(hasher);
        }
        Value::DurationMs(millis) => {
            5u8.hash(hasher);
            millis.hash(hasher);
        }
        Value::Quantity(amount, unit) => {
            6u8.hash(hasher);
            amount.to_bits().hash(hasher);
            unit.hash(hasher);
        }
        Value::Array(items) => {
            7u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                fp_value(hasher, item);
            }
        }
        Value::Table(fields) => {
            8u8.hash(hasher);
            fields.len().hash(hasher);
            for (key, item) in fields {
                key.hash(hasher);
                fp_value(hasher, item);
            }
        }
        // Same-process memo key only: secrecy rests on the redacting
        // `Debug` impl and export refusal, never on hiding this hash.
        // Blobs fingerprint by identity (hash + size + taint), never by
        // payload bytes: same identity ⇒ same value.
        Value::Secret(text) => {
            9u8.hash(hasher);
            text.hash(hasher);
        }
        Value::Blob {
            hash,
            size,
            tainted,
            media_type,
            ..
        } => {
            10u8.hash(hasher);
            hash.hash(hasher);
            size.hash(hasher);
            tainted.hash(hasher);
            media_type.hash(hasher);
        }
    }
}

/// Canonical spelling of an environment variable name (LANG-H-003):
/// `env.NAME` is case-insensitive on every platform, so names fold to
/// uppercase at snapshot, lookup, fingerprint, and provenance time.
pub fn canonical_env_name(name: &str) -> String {
    name.to_uppercase()
}

/// Fold a name/value stream into the canonical environment map
/// (LANG-H-003). Names uppercase; if two spellings collide (possible on
/// Unix, e.g. `PATH` and `Path`), the lexicographically last value wins,
/// so every run and platform resolves the same variable the same way.
fn canonical_env_map<I: Iterator<Item = (String, String)>>(
    entries: I,
) -> std::collections::HashMap<String, String> {
    let mut pairs: Vec<(String, String)> = entries
        .map(|(name, value)| (canonical_env_name(&name), value))
        .collect();
    pairs.sort();
    pairs.into_iter().collect()
}

/// Structural fingerprint of one node for incremental memoization:
/// expression shape, literal data, environment values read, and the
/// already-computed fingerprints of structural children. `cache` is
/// arena-indexed (`NodeId.0` is a dense arena index): the caller
/// walks in topological order, so every dependency's fingerprint is
/// already stored — no recursion, no stack risk on deep chains, no
/// hash-map lookup per edge. Returns None for shapes with no stable
/// form (schema/patch/profile nodes evaluate to Null or outside the
/// walk anyway); unhashable nodes simply re-evaluate every call.
/// Same fingerprint ⇒ same value: every input the evaluator can
/// observe (literals, env snapshot, dependency values) feeds the
/// hash.
fn fingerprint_one(
    engine: &Engine,
    module: &Module,
    id: NodeId,
    cache: &[Option<u64>],
    salt: u64,
    materials: Option<&SecretProfile>,
) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let child = |dep: &NodeId| cache.get(dep.0 as usize).copied().flatten();
    let mut hasher = FpHasher::new(salt);
    match module.get(id)? {
        Node::Literal(v) => {
            0u8.hash(&mut hasher);
            fp_value(&mut hasher, v);
        }
        Node::Env { name, default } => {
            // LANG-H-003: one spelling on every platform.
            let key = canonical_env_name(name);
            // A lazy host fallback can answer for an absent key with a
            // value this engine cannot hash. Never memoize that node
            // (or anything depending on it) across engines.
            if engine.env_fallback.is_some() && !engine.env.contains_key(&key) {
                return None;
            }
            1u8.hash(&mut hasher);
            key.hash(&mut hasher);
            engine.env.get(&key).hash(&mut hasher);
            match default {
                Some(d) => child(d)?.hash(&mut hasher),
                None => 0u8.hash(&mut hasher),
            }
        }
        Node::Secret { locator } => {
            2u8.hash(&mut hasher);
            child(locator)?.hash(&mut hasher);
        }
        Node::Ref(target) => {
            3u8.hash(&mut hasher);
            child(target)?.hash(&mut hasher);
        }
        Node::Ident(name) => {
            4u8.hash(&mut hasher);
            name.hash(&mut hasher);
        }
        Node::Interpolate { parts } => {
            5u8.hash(&mut hasher);
            parts.len().hash(&mut hasher);
            for part in parts {
                match part {
                    crate::ir::InterpPart::Literal(s) => {
                        0u8.hash(&mut hasher);
                        s.hash(&mut hasher);
                    }
                    crate::ir::InterpPart::Expr(e) => {
                        1u8.hash(&mut hasher);
                        child(e)?.hash(&mut hasher);
                    }
                }
            }
        }
        Node::Binary { op, lhs, rhs } => {
            6u8.hash(&mut hasher);
            op.hash(&mut hasher);
            child(lhs)?.hash(&mut hasher);
            child(rhs)?.hash(&mut hasher);
        }
        Node::Unary { op, expr } => {
            7u8.hash(&mut hasher);
            op.hash(&mut hasher);
            child(expr)?.hash(&mut hasher);
        }
        Node::Array { elements } => {
            8u8.hash(&mut hasher);
            elements.len().hash(&mut hasher);
            for element in elements {
                child(element)?.hash(&mut hasher);
            }
        }
        Node::Table { fields } => {
            9u8.hash(&mut hasher);
            fields.len().hash(&mut hasher);
            for (key, child_id) in fields {
                key.hash(&mut hasher);
                child(child_id)?.hash(&mut hasher);
            }
            // Deferred merges change this table's value, so every
            // candidate is an input to the fingerprint.
            if let Some(pending) = module.merge_pending.get(&id) {
                pending.len().hash(&mut hasher);
                for entry in pending {
                    entry.path.hash(&mut hasher);
                    entry.local.hash(&mut hasher);
                    for candidate in &entry.candidates {
                        // Source identity and the candidate's own position
                        // are part of the diagnostic, so they are part of
                        // the fingerprint too.
                        candidate.source.hash(&mut hasher);
                        module.node_files.get(&candidate.node).hash(&mut hasher);
                        module
                            .node_spans
                            .get(&candidate.node)
                            .map(|s| (s.line, s.column, s.end_line, s.end_column))
                            .hash(&mut hasher);
                        child(&candidate.node)?.hash(&mut hasher);
                    }
                }
            }
        }
        Node::Quantity { value, unit } => {
            10u8.hash(&mut hasher);
            unit.hash(&mut hasher);
            child(value)?.hash(&mut hasher);
        }
        Node::Semantic { tag, inner } => {
            11u8.hash(&mut hasher);
            tag.as_str().hash(&mut hasher);
            child(inner)?.hash(&mut hasher);
        }
        Node::ContentAddressed { media_type, data } => {
            12u8.hash(&mut hasher);
            child(media_type)?.hash(&mut hasher);
            child(data)?.hash(&mut hasher);
            // Structural taint is an input to the value: adding a
            // `secret(data)` or `secret(media)` elsewhere in the
            // document must not serve a memoized untainted blob.
            materials
                .is_some_and(|profile| profile.node_is_secret(*data))
                .hash(&mut hasher);
            materials
                .is_some_and(|profile| profile.node_is_secret(*media_type))
                .hash(&mut hasher);
        }
        _ => return None,
    }
    Some(hasher.finish())
}

/// One secret-safe evaluation step (TOOLING DEBUG.001): node identity,
/// value path when one exists, kind, redacted JSON value, taint, and
/// the diagnostic codes emitted while the node was evaluated. Values
/// render through the canonical redactor, so secret material never
/// enters a step.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StepEvent {
    pub node: u32,
    pub path: Option<String>,
    pub kind: String,
    pub tainted: bool,
    pub value: serde_json::Value,
    pub codes: Vec<String>,
}

/// A step collected during evaluation and rendered once the whole walk
/// has finished, so the sensitive set is complete regardless of node
/// order (a copy can precede the secret it contains).
struct PendingStep {
    node_id: crate::ir::NodeId,
    path: Option<String>,
    kind: String,
    sensitive: bool,
    codes: Vec<String>,
}

/// Renders step values with path-aware material marking: containers
/// keep their shape, and every material leaf (string, number, whatever
/// the kind) becomes the marker. Without a path an internal container
/// fails closed to the marker when it is material at all.
///
/// The sensitive set is the complete redactor (authored literals plus
/// every runtime secret evaluated so far), so a blob payload embedding
/// either kind of secret is dropped before its bytes are encoded. The
/// bool reports whether the render redacted anything, so the step's
/// `tainted` flag always agrees with its value.
struct StepRenderer<'a> {
    profile: &'a SecretProfile,
    redactor: &'a SecretRedactor,
    blob_material: std::cell::RefCell<std::collections::HashMap<[u8; 32], bool>>,
}

impl<'a> StepRenderer<'a> {
    fn new(profile: &'a SecretProfile, redactor: &'a SecretRedactor) -> Self {
        Self {
            profile,
            redactor,
            blob_material: std::cell::RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// Payload materiality, memoized by content hash. An identity-only
    /// blob (no payload in the tree) is never material here.
    /// Retained-payload materiality, cached by the bytes actually scanned.
    /// The blob's hash is not trusted as a cache key: a host-built value
    /// can assign the same hash to two different payloads.
    fn blob_material(&self, payload: Option<&Vec<u8>>) -> bool {
        let Some(bytes) = payload else {
            return false;
        };
        let key = crate::convert::payload_scan_key(bytes);
        if let Some(known) = self.blob_material.borrow().get(&key) {
            return *known;
        }
        let material =
            crate::convert::blob_payload_is_material(self.redactor, self.profile, Some(bytes));
        self.blob_material.borrow_mut().insert(key, material);
        material
    }

    fn render(
        &self,
        path: Option<&str>,
        value: &Value,
        sensitive: bool,
    ) -> (serde_json::Value, bool) {
        let marker = || serde_json::Value::String(crate::secret::MARKER.to_string());
        match value {
            Value::Array(items) => {
                let Some(base) = path else {
                    if sensitive {
                        return (marker(), true);
                    }
                    // Without a path the shape is still rendered child
                    // by child, so a blob payload is swept at depth.
                    let mut marked = false;
                    let rendered = items
                        .iter()
                        .map(|item| {
                            let (child, hit) = self.render(None, item, false);
                            marked |= hit;
                            child
                        })
                        .collect();
                    return (serde_json::Value::Array(rendered), marked);
                };
                let mut marked = false;
                let rendered = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let child = format!("{base}[{index}]");
                        let (rendered, hit) =
                            self.render(Some(&child), item, self.profile.path_is_secret(&child));
                        marked |= hit;
                        rendered
                    })
                    .collect();
                (serde_json::Value::Array(rendered), marked)
            }
            Value::Table(map) => {
                let Some(base) = path else {
                    if sensitive {
                        return (marker(), true);
                    }
                    let mut marked = false;
                    let mut out = serde_json::Map::new();
                    for (key, item) in map {
                        let (rendered, hit) = self.render(None, item, false);
                        marked |= hit;
                        let export_key = if self.profile.redact_text(key).is_some() {
                            marked = true;
                            crate::secret::MARKER.to_string()
                        } else {
                            key.clone()
                        };
                        out.insert(export_key, rendered);
                    }
                    return (serde_json::Value::Object(out), marked);
                };
                let mut marked = false;
                let mut out = serde_json::Map::new();
                for (key, item) in map {
                    let child = if base.is_empty() {
                        key.clone()
                    } else {
                        format!("{base}.{key}")
                    };
                    let (rendered, hit) =
                        self.render(Some(&child), item, self.profile.path_is_secret(&child));
                    marked |= hit;
                    // A key is data: material key text renders the marker.
                    let export_key = if self.profile.redact_text(key).is_some() {
                        marked = true;
                        crate::secret::MARKER.to_string()
                    } else {
                        key.clone()
                    };
                    out.insert(export_key, rendered);
                }
                (serde_json::Value::Object(out), marked)
            }
            Value::Blob {
                hash,
                size,
                tainted,
                media_type,
                payload,
            } => {
                // A materialised payload can carry a secret literal (or
                // a runtime secret) whose base64 form no literal sweep
                // can match. The identity is withheld when its media type
                // or retained payload is material, or when an untainted
                // value is promoted only by structural/path materiality;
                // an evaluated tainted blob derived from a secret keeps
                // its real salted identity.
                let path_material =
                    sensitive || path.is_some_and(|path| self.profile.path_is_secret(path));
                let identity_material = self.profile.blob_identity_is_material(media_type)
                    || self.blob_material(payload.as_ref())
                    || (!*tainted && path_material);
                if *tainted || identity_material {
                    let identity = if identity_material {
                        crate::blob::redacted_identity()
                    } else {
                        crate::blob::verified_identity(hash, *tainted, media_type, payload.as_ref())
                    };
                    (crate::convert::redacted_blob_json(&identity, *size), true)
                } else {
                    // Steps are identity-only: payload bytes are never
                    // rendered (they can be megabytes per ancestor).
                    (
                        crate::convert::value_to_json(&crate::convert::strip_blob_payloads(value)),
                        false,
                    )
                }
            }
            _ if sensitive || path.is_some_and(|path| self.profile.path_is_secret(path)) => {
                (marker(), true)
            }
            _ => (crate::convert::value_to_json(value), false),
        }
    }
}

pub(crate) fn value_has_secret(value: &Value) -> bool {
    match value {
        Value::Secret(_) => true,
        Value::Blob { tainted, .. } => *tainted,
        Value::Array(items) => items.iter().any(value_has_secret),
        Value::Table(map) => map.values().any(value_has_secret),
        _ => false,
    }
}

/// True when a rendered value carries structural material that is not
/// Secret-kinded: a copy of a plain `secret(...)` locator, an environment
/// value, a runtime seed, or a blob whose media type or retained payload
/// is material. The predicate is exactly "the export would redact this",
/// so the claim check and the emitted tree agree.
fn payload_has_material_content(value: &Value, profile: &SecretProfile) -> bool {
    match value {
        Value::String(text) => profile.redact_text(text).is_some(),
        Value::Blob {
            tainted,
            media_type,
            payload,
            ..
        } => {
            *tainted
                || profile.blob_identity_is_material(media_type)
                || crate::convert::blob_payload_is_material(
                    profile.literals(),
                    profile,
                    payload.as_ref(),
                )
        }
        Value::Array(items) => items
            .iter()
            .any(|item| payload_has_material_content(item, profile)),
        Value::Table(map) => map.iter().any(|(key, item)| {
            // A key is data too: material key text is a copy even when
            // the value beside it is benign.
            profile.redact_text(key).is_some() || payload_has_material_content(item, profile)
        }),
        _ => false,
    }
}

/// True when the payload sits at a path the export renders as material.
/// A profile override inherits the materiality of the value it replaces,
/// and the export redacts whatever kind now occupies that path - including
/// a number or boolean, which carries no material text to match.
fn payload_has_material_path(value: &Value, profile: &SecretProfile, path: &str) -> bool {
    if profile.path_is_secret(path) {
        return true;
    }
    match value {
        Value::Array(items) => items.iter().enumerate().any(|(index, item)| {
            payload_has_material_path(item, profile, &format!("{path}[{index}]"))
        }),
        Value::Table(map) => map.iter().any(|(key, item)| {
            let child = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            payload_has_material_path(item, profile, &child)
        }),
        _ => false,
    }
}

/// The optional side channels of a walk — the incremental memo, the step
/// trace, the runtime-seed collector, the demand charge report — plus the
/// validation switch, bundled so `evaluate_walk` stays within clippy's
/// argument bound. Fields are exactly the walk's previous parameters.
struct WalkChannels<'a> {
    memo: Option<&'a mut IncrementalMemo>,
    steps: Option<&'a mut Vec<StepEvent>>,
    seeds: Option<&'a mut SecretRedactor>,
    validate: bool,
    charge: Option<&'a mut ChargeReport>,
}

impl Engine {
    /// Engine with the process environment snapshotted.
    ///
    /// Resilient by default: `evaluate` returns `Ok` with the value AND the
    /// diagnostics; hosts MUST inspect the diagnostics vector (or set
    /// `resilient = false`, or use `Engine::strict()`) for fail-closed
    /// validation. `Ok` alone does not mean the document is valid.
    pub fn new() -> Self {
        let mut engine = Self::hermetic();
        // Undecodable entries cannot enter the snapshot (keys
        // and values must be Unicode to be referenceable);
        // skipping them is deterministic per machine, unlike a
        // `vars()` panic on exotic ambients. Names fold to uppercase
        // so `env.Path` and `env.PATH` are one variable everywhere
        // (LANG-H-003).
        engine.env =
            canonical_env_map(std::env::vars_os().filter_map(|(key, value)| {
                Some((key.into_string().ok()?, value.into_string().ok()?))
            }));
        engine
    }

    /// Strict engine with the process environment snapshotted: `evaluate`
    /// returns `Err(ValidationFailed)` when any Error-severity diagnostic is
    /// recorded (schema violations, blob bounds, closed tables), which is
    /// the fail-closed behaviour a service host wants. `Engine::new()` is
    /// resilient by design; this constructor makes the strict choice
    /// discoverable without changing the library default.
    pub fn strict() -> Self {
        let mut engine = Self::new();
        engine.resilient = false;
        engine
    }

    /// Hermetic engine: identical to `new()` except the environment starts
    /// empty instead of snapshotting the process ambient. Embeddings (FFI,
    /// WASM) use this so host ambient can never leak across the boundary
    /// (this constructor is resilient too: see `new()`);
    /// host values arrive only through explicit maps or lookup callbacks.
    /// Required on platforms without a process environment
    /// (wasm32-unknown-unknown aborts on ambient access): construct here,
    /// never snapshot-then-discard.
    pub fn hermetic() -> Self {
        Self {
            resilient: true,
            profile_override: None,
            budget: Budget::default(),
            env: std::collections::HashMap::new(),
            env_fallback: None,
            pure: false,
            materialise_blobs: false,
            reader_version: None,
            taint_probe: false,
            blob_salt: None,
            envelope_gate: Some(crate::envelope::EnvelopeGate::shipped()),
            envelope_trust: None,
            capabilities: crate::authority::CapabilitySet::all(),
            sandbox_guard: None,
        }
    }

    /// Fixed environment snapshot instead of the process ambient. Names
    /// fold to uppercase, exactly like the process snapshot (LANG-H-003).
    pub fn with_env(mut self, env: std::collections::HashMap<String, String>) -> Self {
        self.env = canonical_env_map(env.into_iter());
        self
    }

    /// Refuse a surface operation whose authority this host does not hold.
    /// Embedded hosts call this before a tool operation; the evaluator's
    /// own `evaluate`/`read-secrets` gates do not depend on it.
    pub fn require_capability(
        &self,
        capability: crate::authority::Capability,
    ) -> Result<(), EvalError> {
        if self.capabilities.contains(capability) {
            Ok(())
        } else {
            Err(EvalError::CapabilityDenied {
                capability: capability.as_str(),
                span: None,
            })
        }
    }

    /// Lazy host lookup consulted on snapshot miss (F-016).
    pub fn with_env_fallback(mut self, lookup: EnvLookup) -> Self {
        self.env_fallback = Some(lookup);
        self
    }

    /// L-P05: refuse ambient input (env reads, unpinned includes, profile
    /// overrides) before evaluation. A document that is pure evaluates
    /// identically with and without this mode.
    pub fn with_pure(mut self, pure: bool) -> Self {
        self.pure = pure;
        self
    }

    pub fn evaluate(&self, module: &Module) -> Result<(Value, Vec<Diagnostic>), EvalError> {
        // LANG-F-004: verifying an envelope's taint claim needs the same
        // material set every export uses, including copies of plain locator
        // values and computed runtime secrets. Collect seeds only when the
        // document makes a claim, so ordinary evaluation pays nothing.
        if matches!(
            crate::envelope::sender_taint(module),
            crate::envelope::TaintClaim::Claim(_)
        ) || module.mode == Mode::Data
            || !module.patches.is_empty()
        {
            let mut seeds = SecretRedactor::new();
            return self.evaluate_inner(module, None, None, Some(&mut seeds));
        }
        self.evaluate_inner(module, None, None, None)
    }

    /// L-P40: evaluate under an explicit demand and report what it cost.
    ///
    /// `Demand::All` is the default for every existing entry point and
    /// stays byte-identical, diagnostics included. `Demand::Cone(P)`
    /// materialises only the closure of the document path `P` — the node
    /// at `P`, its value dependencies, the ancestor containers that shape
    /// it, and the transform inputs that write into it — so an undemanded
    /// pathological subtree cannot deny service to the cone (LANG-C-013).
    /// The whole-document static phase and every safety gate still run
    /// first, so a cone never hides a budget trip (spec/budgets.md §2).
    ///
    /// A `Cone(P)` whose `P` is not a document subtree — a top-level
    /// binding name, an indexed path, or a missing key — cannot be shaped
    /// into a cone, so the walk materialises the whole document and the
    /// caller projects the path it asked for; the projected value is
    /// identical either way (§6.4). The shipped `eval --path` and
    /// `inspect --path` surfaces evaluate first and project (or report)
    /// the path afterwards — they do not pre-resolve it — so for such a
    /// spelling the printed value is still correct and only the charge is
    /// whole-document, never the cone's.
    pub fn evaluate_demanded(
        &self,
        module: &Module,
        demand: &Demand,
    ) -> Result<DemandOutcome, EvalError> {
        let closure = self.demand_closure(module, demand);
        // Seed collection mirrors `evaluate`: a plain Config document
        // pays nothing, while a taint claim, Data mode, or a patch set
        // needs the runtime seeds to redact its own diagnostics.
        let needs_seeds = matches!(
            crate::envelope::sender_taint(module),
            crate::envelope::TaintClaim::Claim(_)
        ) || module.mode == Mode::Data
            || !module.patches.is_empty();
        let mut seeds = SecretRedactor::new();
        let mut charge = ChargeReport { total: 0, limit: 0 };
        let seed_opt = needs_seeds.then_some(&mut seeds);
        let (value, diagnostics) = self.evaluate_inner_demanded(
            module,
            &closure,
            None,
            None,
            seed_opt,
            Some(&mut charge),
        )?;
        Ok(DemandOutcome {
            value,
            diagnostics,
            seeds,
            charge,
        })
    }

    /// L-P40: the demand-driven analogue of
    /// [`Engine::evaluate_with_seeds`] — the runtime secret seeds are
    /// always collected, whatever the document declares.
    pub fn evaluate_with_seeds_demanded(
        &self,
        module: &Module,
        demand: &Demand,
    ) -> Result<DemandOutcome, EvalError> {
        let closure = self.demand_closure(module, demand);
        let mut seeds = SecretRedactor::new();
        let mut charge = ChargeReport { total: 0, limit: 0 };
        let (value, diagnostics) = self.evaluate_inner_demanded(
            module,
            &closure,
            None,
            None,
            Some(&mut seeds),
            Some(&mut charge),
        )?;
        Ok(DemandOutcome {
            value,
            diagnostics,
            seeds,
            charge,
        })
    }

    /// Resolve a `Demand` to a concrete `Closure`. A `Cone` that is not a
    /// document subtree path falls back to `All`; see
    /// [`Engine::evaluate_demanded`].
    fn demand_closure(&self, module: &Module, demand: &Demand) -> Closure {
        match demand {
            Demand::All => Closure::all(),
            Demand::Cone(path) => demand::cone_closure(module, path).unwrap_or(Closure::All),
        }
    }

    /// Evaluate the tree without schema, annotation, or check validation.
    ///
    /// The repair planner (L-P53) needs the evaluated value while the
    /// validation diagnostics are its own input, and strict validation
    /// never mutates the tree, so this is the same value validation
    /// would see. Crate-internal on purpose: an unvalidated tree is
    /// never an export.
    pub(crate) fn evaluate_tree(
        &self,
        module: &Module,
    ) -> Result<(Value, Vec<Diagnostic>), EvalError> {
        self.evaluate_walk(
            module,
            &Closure::all(),
            WalkChannels {
                memo: None,
                steps: None,
                seeds: None,
                validate: false,
                charge: None,
            },
        )
        .map_err(|e| attach_progressive_warnings(module, &self.budget.resolve(), e))
    }

    /// Evaluate and also return the runtime secret seeds observed by the
    /// walk: every material node's evaluated text. Export surfaces seed
    /// their redactor with this so a secret copied through a `let`
    /// binding, a non-string interpolation part, or a lazy host env
    /// lookup still redacts even though it never enters the tree.
    pub fn evaluate_with_seeds(
        &self,
        module: &Module,
    ) -> Result<(Value, Vec<Diagnostic>, SecretRedactor), EvalError> {
        let (outcome, seeds) = self.evaluate_collecting_seeds(module);
        outcome.map(|(value, diagnostics)| (value, diagnostics, seeds))
    }

    /// Evaluate while collecting runtime secret seeds, returning the
    /// seeds even when the walk fails. Seeds are written as each node
    /// evaluates, so a type, blob, or budget error still leaves the
    /// material text discovered so far available for redaction.
    pub fn evaluate_collecting_seeds(
        &self,
        module: &Module,
    ) -> (Result<(Value, Vec<Diagnostic>), EvalError>, SecretRedactor) {
        let mut seeds = SecretRedactor::new();
        let outcome = self.evaluate_inner(module, None, None, Some(&mut seeds));
        (outcome, seeds)
    }

    /// Evaluate and return one secret-safe step per arena node in
    /// topological order (TOOLING DEBUG.001/002). Each step carries the
    /// node id, its value path when one exists, the value kind, the
    /// REDACTED JSON value, taint, and the diagnostic codes emitted
    /// while evaluating that node. Deterministic: same input, same
    /// steps. The stepper is a post-hoc trace of the real walk, not a
    /// second evaluator.
    pub fn evaluate_with_steps(
        &self,
        module: &Module,
    ) -> Result<(Value, Vec<Diagnostic>, Vec<StepEvent>), EvalError> {
        self.evaluate_with_steps_and_seeds(module)
            .map(|(value, diagnostics, steps, _)| (value, diagnostics, steps))
    }

    /// Step trace plus runtime secret seeds: replay needs both so its
    /// content hash agrees with an audit written by an exporter that
    /// absorbed the seeds.
    pub fn evaluate_with_steps_and_seeds(
        &self,
        module: &Module,
    ) -> Result<(Value, Vec<Diagnostic>, Vec<StepEvent>, SecretRedactor), EvalError> {
        let mut steps = Vec::new();
        let mut seeds = SecretRedactor::new();
        let (value, diagnostics) =
            self.evaluate_inner(module, None, Some(&mut steps), Some(&mut seeds))?;
        Ok((value, diagnostics, steps, seeds))
    }

    /// Incremental evaluation over a reused memo store (C-055,
    /// spec/performance.md §2): nodes whose structural fingerprint
    /// (expression shape, literal data, env values read, and
    /// dependency fingerprints) matches a previous call serve the
    /// memoized value and diagnostics; only the affected sub-DAG
    /// re-evaluates. Configuration changes (resilient flag, profile
    /// override, budgets) clear the store first. Post-walk
    /// transforms (profiles, patches, validation) re-run every
    /// call — they compose the final tree and stay cheap relative
    /// to the walk they follow.
    pub fn evaluate_incremental(
        &self,
        module: &Module,
        memo: &mut IncrementalMemo,
    ) -> Result<(Value, Vec<Diagnostic>), EvalError> {
        self.evaluate_incremental_with_seeds(module, memo)
            .map(|(value, diagnostics, _)| (value, diagnostics))
    }

    /// Incremental evaluation that also returns the runtime secret
    /// seeds (see [`Engine::evaluate_with_seeds`]). Memo hits still
    /// insert their served values into the walk map, so the seeds cover
    /// every material node on a partially cached document.
    pub fn evaluate_incremental_with_seeds(
        &self,
        module: &Module,
        memo: &mut IncrementalMemo,
    ) -> Result<(Value, Vec<Diagnostic>, SecretRedactor), EvalError> {
        // `Budget` compares by value (all fields are `u64`): any
        // configuration change clears the store before the walk.
        let token = (
            self.resilient,
            self.profile_override.clone(),
            self.budget,
            self.materialise_blobs,
            self.taint_probe,
            module.mode,
            self.env_fallback.is_some(),
            // L-P05: a pure run refuses before the walk; a cached tree
            // from a default run must not satisfy it.
            self.pure,
            // L-P35: a different authority set may refuse the document.
            self.capabilities,
        );
        if memo.config.as_ref() != Some(&token) {
            memo.map.clear();
            memo.config = Some(token);
        }
        memo.last_eval_count = 0;
        memo.last_memo_hits = 0;
        let mut seeds = SecretRedactor::new();
        let outcome = self.evaluate_inner(module, Some(memo), None, Some(&mut seeds));
        // Bound the store: every document version inserts novel
        // fingerprints, so without eviction a long-lived memo (the
        // language server reuses one per document) grows with edit
        // history. Clearing only loses hits — correctness is
        // unaffected — and the bound is a pure function of counts,
        // so behavior stays deterministic: same edit history, same
        // clears, same outputs.
        let cap = module.arena.len().saturating_mul(4).max(1024);
        if memo.map.len() > cap {
            // LANG-K-006: drop roughly half instead of the whole store. A
            // long-lived memo (the language server carries one per
            // document across edits) kept oscillating between full hits
            // and zero hits at the boundary. Fingerprints are opaque
            // keys, so which half survives is arbitrary; correctness does
            // not depend on the choice (each entry stores the value its
            // own fingerprint produced).
            let drop = memo.map.len() / 2;
            let doomed: Vec<u64> = memo.map.keys().take(drop).copied().collect();
            for fingerprint in doomed {
                memo.map.remove(&fingerprint);
            }
        }
        outcome.map(|(value, diagnostics)| (value, diagnostics, seeds))
    }

    fn evaluate_inner(
        &self,
        module: &Module,
        memo: Option<&mut IncrementalMemo>,
        steps: Option<&mut Vec<StepEvent>>,
        seeds: Option<&mut SecretRedactor>,
    ) -> Result<(Value, Vec<Diagnostic>), EvalError> {
        self.evaluate_inner_demanded(module, &Closure::all(), memo, steps, seeds, None)
    }

    /// The walk plus the abort-path warning re-derivation, parameterised
    /// by demand (L-P40 §7): [`Engine::evaluate_inner`] is the `All` case
    /// and the demand entry points pass their closure here.
    fn evaluate_inner_demanded(
        &self,
        module: &Module,
        demand: &Closure,
        memo: Option<&mut IncrementalMemo>,
        steps: Option<&mut Vec<StepEvent>>,
        seeds: Option<&mut SecretRedactor>,
        charge_out: Option<&mut ChargeReport>,
    ) -> Result<(Value, Vec<Diagnostic>), EvalError> {
        // LANG-C-009: a budget trip replaces the diagnostic list with the
        // error, so re-derive the progressive warnings the walk had already
        // collected and carry them on the error. Only the abort path pays
        // for this (one extra static scan).
        self.evaluate_walk(
            module,
            demand,
            WalkChannels {
                memo,
                steps,
                seeds,
                validate: true,
                charge: charge_out,
            },
        )
        .map_err(|e| {
            // Resolve before deriving W013: an unresolved zero field would
            // read as "limit 0" and skip the warning the success path
            // emits (review finding on LANG-C-009).
            attach_progressive_warnings(module, &self.budget.resolve(), e)
        })
    }

    /// L-P05: the ambient inputs a pure evaluation refuses, in a fixed
    /// order (profile override, unpinned include, env read) so the
    /// diagnostic is deterministic when a document has several.
    fn pure_refusal(&self, module: &Module) -> Option<EvalError> {
        if let Some(name) = &self.profile_override {
            return Some(EvalError::PureRefused {
                message: format!("pure evaluation refused a profile override: '{name}'"),
                span: None,
                file: None,
            });
        }
        if let Some((pin, _)) = module
            .include_resolutions
            .iter()
            .find(|(_, kind)| *kind == crate::ir::IncludeResolutionKind::Unpinned)
        {
            // The include statement has a line but no node span; the file
            // is the document that wrote it.
            let line = module
                .includes
                .iter()
                .find(|stmt| stmt.path == pin.as_written)
                .map(|stmt| stmt.line);
            let message = match line {
                Some(line) => format!(
                    "pure evaluation refused an unpinned include: '{}' (line {line})",
                    pin.as_written
                ),
                None => format!(
                    "pure evaluation refused an unpinned include: '{}'",
                    pin.as_written
                ),
            };
            return Some(EvalError::PureRefused {
                message,
                span: None,
                file: Some(module.source_name.clone()),
            });
        }
        for (index, node) in module.arena.iter().enumerate() {
            if let Node::Env { name, .. } = node {
                let id = NodeId(index as u32);
                return Some(EvalError::PureRefused {
                    message: format!("pure evaluation refused an env read: '{name}'"),
                    span: module.node_spans.get(&id).cloned(),
                    file: module.node_files.get(&id).cloned(),
                });
            }
        }
        None
    }

    fn evaluate_walk(
        &self,
        module: &Module,
        demand: &Closure,
        channels: WalkChannels<'_>,
    ) -> Result<(Value, Vec<Diagnostic>), EvalError> {
        let WalkChannels {
            mut memo,
            steps,
            mut seeds,
            validate,
            charge: charge_out,
        } = channels;
        // L-P05: purity is a structural pre-scan, so every entry point
        // (value, seeds, tree-only) refuses the same ambient inputs and
        // dead code in an unselected profile cannot hide one.
        if self.pure {
            if let Some(refusal) = self.pure_refusal(module) {
                return Err(refusal);
            }
        }
        // L-P35: content authority. The capability-subset check runs before
        // the sandbox guard, so E049 precedes E048 when both apply; the
        // documented gate order is E047 > E049 > E048 > E033/E042.
        let granted = if module.mode == crate::ir::Mode::Data {
            match crate::envelope::declared_capabilities(module)? {
                Some(declared) => {
                    if let Some(missing) = declared.first_missing(self.capabilities) {
                        return Err(EvalError::CapabilityDenied {
                            capability: missing.as_str(),
                            span: None,
                        });
                    }
                    declared
                }
                None => self.capabilities,
            }
        } else {
            self.capabilities
        };
        if !granted.contains(crate::authority::Capability::Evaluate) {
            return Err(EvalError::CapabilityDenied {
                capability: crate::authority::Capability::Evaluate.as_str(),
                span: None,
            });
        }
        if !granted.contains(crate::authority::Capability::ReadSecrets) {
            for (index, node) in module.arena.iter().enumerate() {
                if matches!(node, Node::Env { .. } | Node::Secret { .. }) {
                    return Err(EvalError::CapabilityDenied {
                        capability: crate::authority::Capability::ReadSecrets.as_str(),
                        span: module
                            .node_spans
                            .get(&crate::ir::NodeId(index as u32))
                            .cloned(),
                    });
                }
            }
        }
        if let Some(guard) = &self.sandbox_guard {
            if self.env_fallback.is_some() {
                return Err(EvalError::SandboxRefused {
                    message: "sandbox refused an environment fallback".into(),
                    span: None,
                    file: None,
                });
            }
            if self.env.len() != guard.env_values.len()
                || self
                    .env
                    .iter()
                    .any(|(name, value)| guard.env_values.get(name) != Some(value))
            {
                return Err(EvalError::SandboxRefused {
                    message: "sandbox refused an env value outside the closure".into(),
                    span: None,
                    file: None,
                });
            }
            if self.profile_override != guard.profile {
                return Err(EvalError::SandboxRefused {
                    message: "sandbox refused a profile outside the closure".into(),
                    span: None,
                    file: None,
                });
            }
            for (index, node) in module.arena.iter().enumerate() {
                if let Node::Env { name, .. } = node {
                    if !guard.env_names.contains(&canonical_env_name(name)) {
                        let id = crate::ir::NodeId(index as u32);
                        let escaped: String = name.chars().flat_map(char::escape_default).collect();
                        return Err(EvalError::SandboxRefused {
                            message: format!(
                                "sandbox refused an undeclared environment read `{escaped}`"
                            ),
                            span: module.node_spans.get(&id).cloned(),
                            file: module.node_files.get(&id).cloned(),
                        });
                    }
                }
            }
        }
        // One meaning for a zero field on every surface (LANG-C-015): zero
        // resolves to the shipped default; disabling a limit needs
        // `Budget::unlimited()` (the UNLIMITED sentinel).
        let budget = self.budget.resolve();
        // The deadline covers everything from here on (spec/budgets.md
        // §2): static scans, cycle check, sort, walk, transforms, and
        // validation share one clock.
        let deadline = budget.wall_deadline();
        crate::trace::record("eval_start", module.arena.len() as u64, &module.diagnostics);
        // Version floors (LANG-B-021): refuse before any work when the
        // host declared its reader version and the document needs a
        // newer one. Without a declared version the running library is the
        // reader, so the same refusal applies (the earlier advisory default
        // was superseded; spec/schema-evolution.md states the shipped rule).
        // LANG-F-006: `min_reader` is enforced at read time. A host that
        // declares its reader version gets that version; otherwise the
        // running library is the reader, so a document demanding a newer
        // reader is refused instead of silently mis-parsed.
        let reader = self
            .reader_version
            .clone()
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        check_reader_floor(module, &reader)?;
        // Receiver gate (LANG-C-004): a peer document must not negotiate
        // this host into evaluating semantics it does not implement.
        // Refusals are fatal in strict and resilient modes alike.
        if let Some(gate) = &self.envelope_gate {
            gate.check(module)?;
        }
        // G16 (spec/specification.md §3.4): a cycle is detected FIRST,
        // before any value is produced and before the static shape
        // budgets. The spec is normative; the pre-G16 order ran budgets
        // first and reported X001 for a cyclic over-budget document.
        // `petgraph::is_cyclic_directed` walks the graph recursively and
        // overflows the stack on a long chain (a crafted document with
        // ~950 aliases killed the process). `toposort` is iterative and
        // reports the same cycle, so use it directly.
        let order = toposort(&module.graph, None).map_err(|_| EvalError::CycleDetected)?;
        // Budgets next: fail fast on monsters before any real work.
        check_static_budgets(module, &budget)?;
        // Structural secret material: locator subtrees plus every node
        // derived from one (containers, interpolations, references).
        // Computed once when a stepper asked for a trace, the module builds
        // blobs (their structural taint must match the tooling model), a
        // caller asked for runtime seeds, a report is due (its total must be
        // length-invariant, C2), or the module carries deferred merges. The
        // same analysis backs `inspect` and hover, so all surfaces agree on
        // what is material.
        let has_blobs = module
            .arena
            .iter()
            .any(|node| matches!(node, Node::ContentAddressed { .. }));
        let secret_profile: Option<SecretProfile> = (steps.is_some()
            || has_blobs
            || seeds.is_some()
            || charge_out.is_some()
            || !module.merge_pending.is_empty())
        // Absorb the engine's environment: a material env value copied
        // into a plain literal must be flagged wherever it surfaces, so
        // the report seed and the trace redactor clamp it (L-P40 §3.4,
        // C2). Runtime seeds are collected during the walk, so only the
        // engine's closed environment is available here; the walk adds
        // the seeds afterwards.
        .then(|| {
            SecretProfile::with_context(
                module,
                self.profile_override.as_deref(),
                Some(&self.env),
                None,
            )
        });
        // Reference-aware materialization bounds (size and depth): the
        // billion-laughs killer. Runs on the topological order so every
        // bound is final when read; saturating arithmetic keeps it
        // linear even for exponential shapes. Returns the cumulative
        // bound plus the per-node bounds and the maximum depth bound,
        // seeding the transform accountant below.
        // The walk needs the cumulative bound and per-node bounds; the
        // depth bound and max node size only feed the progressive warnings
        // (recomputed by `progressive_warnings` on the abort path).
        let (dp_total, dp_bounds, _dp_depth, _dp_max_depth) =
            check_dp_bounds(module, &order, &budget)?;
        // The printed total must not vary with a secret's length (C2). The
        // unclamped DP seed counts a material `Literal(String)` as `1 + len`,
        // and the runtime arms add `len` again, so the report would leak the
        // length twice over. Size every material node at the redacted marker
        // in the report seed and in each report charge, exactly as the cost
        // model already does; enforcement keeps the real bytes.
        let report_material: Vec<bool> = match &secret_profile {
            Some(profile) => cost::material_flags(module, profile),
            None => Vec::new(),
        };
        let has_material = report_material.iter().any(|flag| *flag)
            || secret_profile
                .as_ref()
                .is_some_and(|profile| !profile.material_envs().is_empty());
        // Demand seed (L-P40 §4.2): `All` inherits the whole-graph DP
        // total, byte-identical to the pre-demand walk. A cone inherits
        // the closure-scoped DP total — its members' bounds computed
        // over the closure alone — so an undemanded sibling's static
        // weight never seeds the accountant and cannot deny service to
        // the demanded cone (LANG-C-013), while the whole-document
        // static phase above still failed closed. The cone pass never
        // trips (every closure bound is at most its whole-document
        // bound, and the whole document already cleared the ceilings),
        // and dynamic growth inside the closure still charges through
        // `charge`, so the seed cannot hide a trip (§5).
        let seed_total = match demand {
            Closure::All => dp_total,
            Closure::Cone { members, .. } => {
                materialization_bounds(
                    module,
                    &order,
                    &budget,
                    DpScope::Closure(members.as_slice()),
                    None,
                )?
                .total
            }
        };
        // Report-side seed and per-node bounds: the same scope with material
        // nodes clamped to the marker. Only a document that carries material
        // pays the second pass; without material the two seeds are
        // byte-identical and `report_bounds` stays empty.
        let (seed_report, report_bounds) = if has_material {
            let clamp = Some(report_material.as_slice());
            let bounds = match demand {
                Closure::All => {
                    materialization_bounds(module, &order, &budget, DpScope::WholeReport, clamp)?
                }
                Closure::Cone { members, .. } => materialization_bounds(
                    module,
                    &order,
                    &budget,
                    DpScope::Closure(members.as_slice()),
                    clamp,
                )?,
            };
            (bounds.total, bounds.size)
        } else {
            (seed_total, Vec::new())
        };
        let mut account = Accountant {
            spent: seed_total,
            reported: seed_report,
            limit: budget.max_total_value_size,
            deadline,
            wall_limit: budget.max_wall_ms,
            bytes_since_sample: 0,
        };
        // Report-side per-node sizes, biased at `secret::MARKER_SIZE` for a
        // material node. Mirrors `actuals`/`actual_sizes` and stays empty
        // unless the document carries material, so the common path pays
        // nothing. See [`report_amount`].
        let mut reported_sizes: Vec<u64> = if has_material {
            vec![0u64; module.arena.len()]
        } else {
            Vec::new()
        };
        // Actual materialized sizes for dynamic values (env- and
        // secret-backed strings): the DP counts 1 per node, so each
        // clone charges the delta between actual and bound. Static
        // targets need no entry — DP soundness guarantees actuals stay
        // within bounds, hence zero delta without measuring.
        let mut actuals: HashMap<NodeId, u64> = HashMap::new();
        // Actual (measured) size of every evaluated node, in DP units.
        // The DP pass is sound for static shapes; a container holding
        // dynamic content is larger than its bound, and cloning it
        // duplicates that content, so the accountant needs the real
        // number (LANG-C-001). Computed bottom-up as nodes evaluate
        // (children precede parents in the walk order), so this stays
        // O(nodes), never a subtree re-measure per clone.
        let mut actual_sizes: Vec<u64> = vec![0u64; module.arena.len()];
        let mut values: HashMap<NodeId, Value> = HashMap::new();
        // One salt per evaluation for tainted blob identities (LANG-C-010):
        // derived from the document so it is reproducible, or the host's
        // explicit value.
        let tainted_blob_salt = self
            .blob_salt
            .unwrap_or_else(|| crate::blob::document_salt(module));
        let mut diagnostics = module.diagnostics.clone();
        diagnostics.extend(module.healing.iter().cloned());
        diagnostics.extend(progressive_warnings(module, &budget));
        // Wall-clock backstop (spec/budgets.md §2): sampled every 1024
        // nodes so the overhead is unmeasurable. Counts already passed
        // their static and DP checks; only time needs watching mid-walk.
        // `deadline` was computed at entry and shared with validation.
        let mut visited = 0u64;
        // Global E046 report cap for this evaluation, shared by every
        // scope (no per-scope 1024 multiplication).
        let mut merge_reported = 0usize;
        // Deferred merges apply only to scopes still present in the
        // composed tree (a shadowed include root is evaluated for its own
        // diagnostics but its conflicts are not the document's).
        let merge_reachable: Option<std::collections::HashSet<NodeId>> =
            (!module.merge_pending.is_empty()).then(|| {
                let mut set = std::collections::HashSet::new();
                let mut stack: Vec<NodeId> = Vec::new();
                stack.extend(module.root);
                stack.extend(module.bindings.values().copied());
                stack.extend(module.profiles.values().copied());
                stack.extend(module.patches.iter().copied());
                stack.extend(module.schema);
                for pending in module.merge_pending.values() {
                    for entry in pending {
                        stack.extend(entry.candidates.iter().map(|candidate| candidate.node));
                    }
                }
                while let Some(id) = stack.pop() {
                    if !set.insert(id) {
                        continue;
                    }
                    if let Some(node) = module.get(id) {
                        stack.extend(crate::secret::node_children(node));
                        if let Node::PatchDirective {
                            op: PatchOp::Set { value, .. },
                        } = node
                        {
                            stack.push(*value);
                        }
                    }
                }
                set
            });
        // Step paths are built once, only when the caller asked for a
        // stepper; plain evaluation pays nothing.
        let step_paths: Option<HashMap<NodeId, String>> = steps.as_ref().map(|_| {
            let mut paths: HashMap<NodeId, String> = module.value_paths();
            // `value_paths` names document values but not the root
            // itself; the root step still needs a path so its container
            // keeps its shape.
            if let Some(root) = module.root {
                paths.insert(root, String::new());
            }
            paths
        });
        // `has_blobs` and `secret_profile` were computed above the seed,
        // which needs the profile for its report-side bounds.
        // Whether a node still renders through a benign export path is
        // a property of the post-transform profile; no separate walk is
        // needed (and a pre-transform walk would miss overridden slots).
        // Evaluated values of sensitive nodes, so a container step can
        // redact the same bytes even though the container itself is
        // not Secret-kinded (topological order means children are
        // registered before their parents). Seeded with authored
        // literals so a value assembled elsewhere still redacts.
        let mut sensitive_values: SecretRedactor = secret_profile
            .as_ref()
            .map(|profile| profile.literals().clone())
            .unwrap_or_default();
        // Steps render after the walk, when the sensitive set is
        // complete; only the cheap per-node facts are collected here.
        let mut pending_steps: Vec<PendingStep> = Vec::new();

        // Fingerprints for this call (arena-indexed cache, sized
        // once): computed inline during the walk below, which runs
        // in topological order — every dependency's fingerprint is
        // already stored when its dependents hash. No recursion, no
        // stack risk on deep chains, no second pass over the graph.
        // Allocated only for incremental calls; plain `evaluate`
        // pays nothing.
        let mut fp_cache: Vec<Option<u64>> = if memo.is_some() {
            vec![None; module.arena.len()]
        } else {
            Vec::new()
        };
        // The demanded path, or `None` for `All`; a cone restricts the
        // post-walk transforms to the paths it materialised (§2.3).
        let cone = demand.path();
        for node_idx in order {
            visited += 1;
            if visited.is_multiple_of(1024) {
                if let Some(end) = deadline {
                    if Instant::now() >= end {
                        let node_id = module.graph[node_idx];
                        return Err(EvalError::budget_exceeded(
                            BudgetKind::WallClock,
                            budget.max_wall_ms,
                            module,
                            Some(node_id),
                        ));
                    }
                }
            }
            let node_id = module.graph[node_idx];
            // L-P19: check predicates are evaluated after the transform
            // pass against the final tree. They are not part of the value
            // graph, so their expression errors become T009, never fatal
            // evaluation errors.
            if module.check_nodes.contains(&node_id) {
                continue;
            }
            // L-P40 §2.3: a cone materialises its closure only; any other
            // node is skipped as check nodes already are. The whole-
            // document static phase and every safety gate ran above, so
            // skipping a value node here cannot skip a static trip.
            if !demand.contains(node_id) {
                continue;
            }
            let diag_start = diagnostics.len();
            let node = module.get(node_id).ok_or(EvalError::MissingNode(node_id))?;
            // Incremental fast path: the fingerprint is computed
            // inline (dependencies already walked, so their prints
            // are cached) and an identical print serves the memoized
            // value and its diagnostics verbatim — except positions.
            // Served diagnostics are re-anchored to the CURRENT
            // module's span/file for this node: the memo may hold
            // diagnostics from an earlier parse where the node sat
            // on different lines (the W001 stale-span bug). This is
            // exact, not a guess: every diagnostic the walk emits
            // is node-local, and post-walk transforms (profiles,
            // patches, validation) re-run every call rather than
            // serving memoized diagnostics.
            let mut memo_fp = None;
            if let Some(memo) = memo.as_mut() {
                let salt = memo.salt;
                if let Some(fp) = fingerprint_one(
                    self,
                    module,
                    node_id,
                    &fp_cache,
                    salt,
                    secret_profile.as_ref(),
                ) {
                    if let Ok(index) = usize::try_from(node_id.0) {
                        if let Some(slot) = fp_cache.get_mut(index) {
                            *slot = Some(fp);
                        }
                    }
                    memo_fp = Some(fp);
                    if let Some((value, diags)) = memo.map.get(&fp) {
                        values.insert(node_id, value.clone());
                        let span = module.node_spans.get(&node_id).cloned();
                        let file = module.node_files.get(&node_id).cloned();
                        for d in diags {
                            // The global E046 cap covers memo-served
                            // diagnostics too: a warm run must not report
                            // more conflicts than a cold one.
                            if d.code == "E046" {
                                if merge_reported >= MAX_MERGE_DIAGNOSTICS {
                                    continue;
                                }
                                merge_reported += 1;
                            }
                            let mut fresh = d.clone();
                            // Most memoized diagnostics carry a position
                            // from the cached parse and must be re-anchored
                            // to the current node. A merge conflict is the
                            // exception: its span/file name the *candidate*
                            // source, which the fingerprint pins, so it is
                            // already current and must not be erased.
                            if fresh.code != "E046" {
                                fresh.span = span.clone();
                                fresh.file = file.clone();
                            }
                            diagnostics.push(fresh);
                        }
                        memo.last_memo_hits += 1;
                        if let Some(out) = seeds.as_deref_mut() {
                            if let Some(profile) = secret_profile.as_ref() {
                                let structural = profile.node_is_secret(node_id);
                                // A node served at a material path is
                                // material for this evaluation unless it
                                // is still exported through a benign
                                // path (the renderer redacts the material
                                // occurrence; spreading the seed would
                                // corrupt the benign one).
                                let path_material = profile.node_is_path_material(node_id)
                                    && !profile.node_is_export_visible(node_id);
                                if structural || path_material {
                                    let text = match value {
                                        Value::Secret(text) => Some(text.to_string()),
                                        Value::String(text) => Some(text.clone()),
                                        _ => None,
                                    };
                                    if let Some(text) = text {
                                        if structural || text.len() >= 4 {
                                            out.insert(text);
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }
                }
            }

            let diag_mark = diagnostics.len();
            let value = match node {
                Node::Literal(v) => v.clone(),

                Node::Env { name, default } => {
                    // LANG-H-003: names fold to uppercase everywhere, so
                    // `env.Path` and `env.PATH` are the same variable on
                    // every platform.
                    let key = canonical_env_name(name);
                    match self.env.get(&key) {
                        Some(s) => {
                            // Operator-controlled dynamic content (like
                            // secrets below): charge the bytes as delta and
                            // record the actual for reference deltas.
                            let real = 1u64.saturating_add(s.len() as u64);
                            actuals.insert(node_id, real);
                            // Report the marker for a material env read, so
                            // the printed charge never varies with the
                            // secret's length (C2).
                            if has_material {
                                reported_sizes[node_id.0 as usize] = report_node_size(
                                    node_reports_material(
                                        module,
                                        node_id.0 as usize,
                                        &report_material,
                                        secret_profile.as_ref(),
                                    ),
                                    real,
                                );
                            }
                            account.charge_scoped(
                                module,
                                Some(node_id),
                                s.len() as u64,
                                report_amount(
                                    has_material,
                                    &reported_sizes,
                                    &report_bounds,
                                    node_id.0 as usize,
                                    s.len() as u64,
                                ),
                            )?;
                            Value::String(s.clone())
                        }
                        None => {
                            // Lazy host lookup (F-016): same charging as a
                            // snapshot hit, so lazy values cost eager values.
                            // A consulted miss falls through to defaults, so a
                            // callback that knows nothing changes nothing.
                            if let Some(lookup) = self.env_fallback.clone() {
                                if let Some(s) = lookup(&key) {
                                    let real = 1u64.saturating_add(s.len() as u64);
                                    actuals.insert(node_id, real);
                                    if has_material {
                                        reported_sizes[node_id.0 as usize] = report_node_size(
                                            node_reports_material(
                                                module,
                                                node_id.0 as usize,
                                                &report_material,
                                                secret_profile.as_ref(),
                                            ),
                                            real,
                                        );
                                    }
                                    account.charge_scoped(
                                        module,
                                        Some(node_id),
                                        s.len() as u64,
                                        report_amount(
                                            has_material,
                                            &reported_sizes,
                                            &report_bounds,
                                            node_id.0 as usize,
                                            s.len() as u64,
                                        ),
                                    )?;
                                    Value::String(s)
                                } else if let Some(def) = default {
                                    values.get(def).cloned().unwrap_or(Value::Null)
                                } else {
                                    Value::Null
                                }
                            } else if let Some(def) = default {
                                values.get(def).cloned().unwrap_or(Value::Null)
                            } else {
                                Value::Null
                            }
                        }
                    }
                }

                Node::Secret { locator } => {
                    let loc = values.get(locator).cloned().unwrap_or(Value::Null);
                    let secret = match loc {
                        Value::String(s) => Value::Secret(zeroize::Zeroizing::new(s)),
                        _ => Value::Secret(zeroize::Zeroizing::new(String::new())),
                    };
                    // Dynamic content: the DP counted 1, so charge the
                    // string bytes as delta and record the actual for
                    // reference deltas below.
                    if let Value::Secret(s) = &secret {
                        let actual = 1u64.saturating_add(s.len() as u64);
                        actuals.insert(node_id, actual);
                        // A secret is material by kind: report the marker.
                        if has_material {
                            reported_sizes[node_id.0 as usize] = report_node_size(true, actual);
                        }
                        account.charge_scoped(
                            module,
                            Some(node_id),
                            s.len() as u64,
                            report_amount(
                                has_material,
                                &reported_sizes,
                                &report_bounds,
                                node_id.0 as usize,
                                s.len() as u64,
                            ),
                        )?;
                    }
                    secret
                }

                // Content-addressed blob (PROPOSAL §1.1, spec/blob.md):
                // hash the resolved bytes, record identity. Bytes are
                // charged as delta (the DP pass counted the node as a
                // leaf of known size only after evaluation knows it).
                // String data is untainted; secret-derived data is
                // structurally tainted and never retains a payload.
                Node::ContentAddressed { media_type, data } => {
                    let media = match values.get(media_type).cloned().unwrap_or(Value::Null) {
                        Value::String(s) => s,
                        _ => {
                            return Err(EvalError::type_error(
                                "blob() media type must be a string",
                                module,
                                node_id,
                            ));
                        }
                    };
                    if media.is_empty() {
                        return Err(EvalError::BlobError {
                            code: crate::blob::code::BAD_FORM,
                            message: "blob() media type must be non-empty".to_string(),
                            span: module.node_spans.get(&node_id).cloned(),
                            file: module.node_files.get(&node_id).cloned(),
                        });
                    }
                    let structurally_material = secret_profile.as_ref().is_some_and(|profile| {
                        profile.node_is_secret(*data) || profile.node_is_secret(*media_type)
                    });
                    let (bytes, mut tainted) =
                        match values.get(data).cloned().unwrap_or(Value::Null) {
                            Value::String(s) => (s.into_bytes(), false),
                            // Copy out, then the Zeroizing buffer wipes
                            // itself on drop (LANG-I-005).
                            Value::Secret(s) => (s.as_bytes().to_vec(), true),
                            _ => {
                                return Err(EvalError::type_error(
                                    "blob() data must be a string or secret locator",
                                    module,
                                    node_id,
                                ));
                            }
                        };
                    // A payload that copies material is tainted even when
                    // the call site is not structurally material: the
                    // identity-only export would otherwise publish an
                    // untainted media type/hash slot for secret bytes.
                    // Authored literals come from the analyzed profile;
                    // env values from the engine's closed snapshot.
                    if !tainted {
                        if let Some(profile) = secret_profile.as_ref() {
                            let text = String::from_utf8_lossy(&bytes);
                            let authored = profile.redact_text(&text).is_some();
                            let from_env = profile.material_envs().iter().any(|name| {
                                self.env
                                    .get(name)
                                    .is_some_and(|value| !value.is_empty() && text.contains(value))
                            });
                            tainted = authored || from_env;
                        }
                    }
                    // A plain value used as a secret locator is material
                    // even without a `Value::Secret` marker: the blob is
                    // structurally tainted and can never materialise.
                    tainted |= structurally_material;
                    // A material MEDIA TYPE taints the identity too
                    // (spec/blob.md §3): otherwise the v1 hash commits to
                    // the plaintext media string, and an observer can
                    // confirm a low-entropy guess offline. The export
                    // redacts the media type and flips the flag in its
                    // copy; deciding it here keeps the evaluated value,
                    // the claim check, and the salted v2 identity in
                    // agreement.
                    if !tainted {
                        if let Some(profile) = secret_profile.as_ref() {
                            tainted = profile.blob_identity_is_material(&media);
                        }
                    }
                    let size = bytes.len() as u64;
                    // A tainted blob's payload is material, so the report
                    // sizes it at the marker; the enforced charge keeps the
                    // real bytes.
                    if has_material {
                        reported_sizes[node_id.0 as usize] = report_node_size(
                            tainted
                                || node_reports_material(
                                    module,
                                    node_id.0 as usize,
                                    &report_material,
                                    secret_profile.as_ref(),
                                ),
                            1u64.saturating_add(size),
                        );
                    }
                    account.charge_scoped(
                        module,
                        Some(node_id),
                        size,
                        report_amount(
                            has_material,
                            &reported_sizes,
                            &report_bounds,
                            node_id.0 as usize,
                            size,
                        ),
                    )?;
                    // Record the actual for reference deltas below:
                    // aliases re-charge through the Ref arm.
                    actuals.insert(node_id, 1u64.saturating_add(size));
                    // Tainted payloads hash under the salted v2 domain so
                    // their identity is not a public function of the secret
                    // bytes (LANG-C-010); untainted identity stays v1.
                    let hash = if tainted {
                        crate::blob::hash_salted(&tainted_blob_salt, &media, &bytes)
                    } else {
                        crate::blob::hash(&media, &bytes)
                    };
                    let payload = if tainted {
                        None
                    } else if self.materialise_blobs {
                        Some(bytes)
                    } else {
                        None
                    };
                    Value::Blob {
                        hash,
                        size,
                        tainted,
                        media_type: media,
                        payload,
                    }
                }

                Node::Ref(id) => {
                    // Dynamic targets (env/secret actuals recorded at
                    // production) charge actual-minus-bound per clone;
                    // static targets stay within DP bounds (delta zero,
                    // no measuring needed). Retained blob payloads are
                    // the exception: cloning duplicates the bytes in
                    // memory, so every alias charges the payload again
                    // (fail-safe over-counting, same as the DP total).
                    // Charge the clone's real size above its DP bound for
                    // ANY target, not only dynamic leaves: a container of
                    // dynamic values is far larger than its static bound,
                    // and cloning it duplicates every byte (LANG-C-001).
                    //
                    // Charge BEFORE the clone (LANG-C-006): clone-then-
                    // charge lets one huge clone reach the allocator before
                    // any ceiling fires, and the byte-driven wall sample
                    // inside `charge` can then stop the walk first.
                    let actual = actuals
                        .get(id)
                        .copied()
                        .unwrap_or_else(|| actual_sizes.get(id.0 as usize).copied().unwrap_or(0));
                    if actual > 0 {
                        let bound = dp_bounds.get(id.0 as usize).copied().unwrap_or(1);
                        let clone = actual.saturating_sub(bound);
                        account.charge_scoped(
                            module,
                            Some(node_id),
                            clone,
                            report_amount(
                                has_material,
                                &reported_sizes,
                                &report_bounds,
                                id.0 as usize,
                                clone,
                            ),
                        )?;
                    }
                    if !self.taint_probe {
                        if let Some(Value::Blob {
                            payload: Some(bytes),
                            ..
                        }) = values.get(id)
                        {
                            account.charge(module, Some(node_id), bytes.len() as u64)?;
                        }
                    }
                    values.get(id).cloned().unwrap_or(Value::Null)
                }

                // Semantic tags are tool treatment only: transparent to the value.
                Node::Semantic { inner, .. } => values.get(inner).cloned().unwrap_or(Value::Null),

                // Schema nodes carry types, not values: they evaluate to
                // Null and never export (extracted from value fields at
                // parse). Their constraint arguments evaluate normally;
                // C-031 reads those values during validation.
                Node::SchemaField { .. } | Node::SchemaTable { .. } | Node::SchemaArray { .. } => {
                    Value::Null
                }
                Node::Ident(name) => {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        code: "W001".into(),
                        message: format!("unresolved identifier '{}'", name),
                        path: Some(name.clone()),
                        span: module.node_spans.get(&node_id).cloned(),
                        repair: None,
                        confidence: 1.0,
                        file: module.node_files.get(&node_id).cloned(),
                    });
                    Value::Null
                }

                Node::Interpolate { parts } => {
                    let mut out = String::new();
                    for part in parts {
                        match part {
                            InterpPart::Literal(s) => out.push_str(s),
                            InterpPart::Expr(id) => {
                                if let Some(v) = values.get(id) {
                                    match v {
                                        Value::String(s) => out.push_str(s),
                                        Value::Int(i) => out.push_str(&i.to_string()),
                                        Value::Float(f) => out.push_str(&f.to_string()),
                                        Value::Bool(b) => out.push_str(&b.to_string()),
                                        Value::DurationMs(ms) => out.push_str(&format!("{}ms", ms)),
                                        Value::Quantity(n, unit) => {
                                            out.push_str(&unit.format_amount(*n))
                                        }
                                        Value::Secret(_) => {
                                            return Err(EvalError::secret_leak(module, node_id));
                                        }
                                        Value::Null => out.push_str("null"),
                                        // Structured values have no string
                                        // form: interpolating them is a type
                                        // error, never a placeholder. Blobs
                                        // interpolate as their stable identity
                                        // reference (never payload bytes).
                                        Value::Blob {
                                            hash,
                                            tainted,
                                            media_type,
                                            payload,
                                            ..
                                        } => {
                                            let identity = crate::blob::verified_identity(
                                                hash,
                                                *tainted,
                                                media_type,
                                                payload.as_ref(),
                                            );
                                            out.push_str(&format!(
                                                "$active.blob(blake3:{})",
                                                crate::blob::hash_hex(&identity)
                                            ));
                                        }
                                        Value::Array(_) | Value::Table(_) => {
                                            return Err(EvalError::type_error(
                                                format!(
                                                    "cannot interpolate a {} into a string",
                                                    match v {
                                                        Value::Array(_) => "array",
                                                        _ => "table",
                                                    }
                                                ),
                                                module,
                                                node_id,
                                            ));
                                        }
                                    }
                                } else {
                                    out.push_str("null");
                                }
                            }
                        }
                    }
                    Value::String(out)
                }

                Node::Binary { op, lhs, rhs } => {
                    let l = values.get(lhs).cloned().unwrap_or(Value::Null);
                    let r = values.get(rhs).cloned().unwrap_or(Value::Null);
                    eval_binary(*op, &l, &r)
                        .map_err(|m| EvalError::type_error(m, module, node_id))?
                }

                Node::Unary { op, expr } => {
                    let v = values.get(expr).cloned().unwrap_or(Value::Null);
                    eval_unary(*op, &v).map_err(|m| EvalError::type_error(m, module, node_id))?
                }

                Node::Array { elements } => {
                    // A cone keeps only materialised elements: a non-member
                    // was skipped above and has no value in the map.
                    let els: Vec<Value> = elements
                        .iter()
                        .filter(|e| demand.contains(**e))
                        .map(|e| values.get(e).cloned().unwrap_or(Value::Null))
                        .collect();
                    Value::Array(els)
                }

                Node::Table { fields } => {
                    let mut map = indexmap::IndexMap::new();
                    for (k, id) in fields {
                        // A cone omits undemanded siblings, so the shape is
                        // the demanded spine plus its subtree (§2.2.2).
                        if !demand.contains(*id) {
                            continue;
                        }
                        map.insert(k.clone(), values.get(id).cloned().unwrap_or(Value::Null));
                    }
                    if let Some(pending) = module.merge_pending.get(&node_id) {
                        let reachable = merge_reachable
                            .as_ref()
                            .is_none_or(|set| set.contains(&node_id));
                        if reachable {
                            apply_pending_merges(
                                module,
                                node_id,
                                pending,
                                &mut map,
                                &values,
                                &mut diagnostics,
                                &mut account,
                                deadline,
                                &budget,
                                secret_profile.as_ref(),
                                &self.env,
                                &mut merge_reported,
                                cone,
                            )?;
                        }
                    }
                    Value::Table(map)
                }

                Node::Quantity { value, unit } => {
                    let v = values.get(value).cloned().unwrap_or(Value::Null);
                    normalize_quantity(&v, unit)
                        .map_err(|m| EvalError::type_error(m, module, node_id))?
                }

                _ => Value::Null,
            };
            // Record the node's real size (LANG-C-001) and charge the
            // growth of a dynamically built string at its producer, so a
            // doubling interpolation is accounted once per step and every
            // later alias is charged through the Ref arm.
            if (node_id.0 as usize) < actual_sizes.len() {
                let actual = match module.get(node_id) {
                    Some(Node::Array { elements }) => 1u64.saturating_add(
                        elements
                            .iter()
                            .map(|e| actual_sizes.get(e.0 as usize).copied().unwrap_or(0))
                            .fold(0u64, u64::saturating_add),
                    ),
                    Some(Node::Table { fields }) => {
                        if module.merge_pending.contains_key(&node_id) {
                            // Deferred keys are not AST fields; measure
                            // the merged value so Ref clones charge the
                            // real growth instead of the field bound.
                            measure_merge_value(&value)
                        } else {
                            1u64.saturating_add(
                                fields
                                    .values()
                                    .map(|e| actual_sizes.get(e.0 as usize).copied().unwrap_or(0))
                                    .fold(0u64, u64::saturating_add),
                            )
                        }
                    }
                    Some(Node::Ref(t)) => actual_sizes.get(t.0 as usize).copied().unwrap_or(0),
                    Some(Node::Interpolate { .. }) => match &value {
                        Value::String(text) => 1u64.saturating_add(text.len() as u64),
                        _ => measure_value(&value),
                    },
                    _ => measure_value(&value),
                };
                actual_sizes[node_id.0 as usize] = actual;
                // Mirror the node at the report bias, so a later `Ref` clone
                // charges `report_size - report_bound` through `report_amount`
                // — the enforcement delta with material bytes replaced by the
                // marker. Containers aggregate their (already clamped) child
                // reports; a leaf is sized by its redaction-safe measure,
                // which matches the value its own arm charged. Leaves set the
                // same entry at their charge site (their charge reads it), so
                // this pass only re-affirms them and fills every other node.
                if has_material {
                    if let Some(profile) = secret_profile.as_ref() {
                        let material = node_reports_material(
                            module,
                            node_id.0 as usize,
                            &report_material,
                            secret_profile.as_ref(),
                        );
                        let shape_material = matches!(&value, Value::Secret(_))
                            || matches!(&value, Value::Blob { tainted: true, .. });
                        let reported = match module.get(node_id) {
                            Some(Node::Array { elements }) => report_node_size(
                                material,
                                1u64.saturating_add(
                                    elements
                                        .iter()
                                        .map(|e| {
                                            reported_sizes.get(e.0 as usize).copied().unwrap_or(0)
                                        })
                                        .fold(0u64, u64::saturating_add),
                                ),
                            ),
                            Some(Node::Table { fields }) => report_node_size(
                                material,
                                if module.merge_pending.contains_key(&node_id) {
                                    measure_report_value(&value, profile, &self.env)
                                } else {
                                    1u64.saturating_add(
                                        fields
                                            .values()
                                            .map(|e| {
                                                reported_sizes
                                                    .get(e.0 as usize)
                                                    .copied()
                                                    .unwrap_or(0)
                                            })
                                            .fold(0u64, u64::saturating_add),
                                    )
                                },
                            ),
                            Some(Node::Ref(t)) => report_node_size(
                                material,
                                reported_sizes.get(t.0 as usize).copied().unwrap_or(0),
                            ),
                            _ => report_node_size(
                                material || shape_material,
                                measure_report_value(&value, profile, &self.env),
                            ),
                        };
                        reported_sizes[node_id.0 as usize] = reported;
                    }
                }
                if matches!(module.get(node_id), Some(Node::Interpolate { .. })) {
                    let bound = dp_bounds.get(node_id.0 as usize).copied().unwrap_or(1);
                    let growth = actual.saturating_sub(bound);
                    account.charge_scoped(
                        module,
                        Some(node_id),
                        growth,
                        report_amount(
                            has_material,
                            &reported_sizes,
                            &report_bounds,
                            node_id.0 as usize,
                            growth,
                        ),
                    )?;
                }
            }
            // Memoize the fresh result under its fingerprint for
            // later calls (misses only — hits continued above).
            if let Some(memo) = memo.as_mut() {
                memo.last_eval_count += 1;
                if let Some(fp) = memo_fp {
                    let fresh = diagnostics[diag_mark..].to_vec();
                    memo.map.insert(fp, (value.clone(), fresh));
                }
            }

            if steps.is_some() {
                let material_path = secret_profile
                    .as_ref()
                    .and_then(|profile| profile.material_path_for(node_id));
                // A profile selection can be material at its path
                // without the shared node being globally tainted.
                let node_material = secret_profile
                    .as_ref()
                    .is_some_and(|profile| profile.node_is_secret(node_id));
                // Containers keep their shape (the trace stays useful),
                // and every material leaf is marked by path: a number
                // used as a locator is material too. A binding-only
                // natural label yields to the material profile path so
                // the step names the exported location.
                let natural_path = step_paths
                    .as_ref()
                    .and_then(|paths| paths.get(&node_id))
                    .cloned();
                let step_path = match (&natural_path, material_path) {
                    (Some(natural), Some(material))
                        if !secret_profile
                            .as_ref()
                            .is_some_and(|profile| profile.is_data_path(natural)) =>
                    {
                        Some(material.to_string())
                    }
                    (Some(natural), _) => Some(natural.clone()),
                    (None, Some(material)) => Some(material.to_string()),
                    (None, None) => None,
                };
                // A step whose own path is material is a taint hit;
                // a shared node rendered under its benign natural path
                // is not (the material path redacts in the container).
                let on_material_path =
                    material_path.is_some_and(|path| step_path.as_deref() == Some(path));
                // The renderer also marks a step by its path; the flag
                // must agree or `--break taint` misses a marker step.
                let path_material = step_path.as_deref().is_some_and(|path| {
                    secret_profile
                        .as_ref()
                        .is_some_and(|profile| profile.path_is_secret(path))
                });
                // A reference or binding step can be the only place a
                // short path-material value is rendered: the scalar's
                // export seed is length-gated, so register its text for
                // the step trace explicitly.
                let node_path_material = secret_profile.as_ref().is_some_and(|profile| {
                    profile.node_is_path_material(node_id)
                        && !profile.node_is_export_visible(node_id)
                });
                let sensitive =
                    node_material || on_material_path || path_material || node_path_material;
                match &value {
                    // A runtime secret is sensitive material wherever
                    // it appears; register its text so a copy (string,
                    // media type, or blob payload) redacts too.
                    Value::Secret(text) => sensitive_values.insert(text.to_string()),
                    Value::String(text)
                        if node_material || on_material_path || node_path_material =>
                    {
                        let env_read = matches!(module.get(node_id), Some(Node::Env { .. }));
                        // Register the evaluated text of a material
                        // scalar so a container rendering redacts it.
                        // Only intrinsically material nodes and env
                        // reads do so: a benign literal that merely also
                        // appears at a profile path stays visible in its
                        // own step.
                        if node_material || env_read || node_path_material {
                            sensitive_values.insert(text.clone());
                        }
                    }
                    _ => {}
                }
                pending_steps.push(PendingStep {
                    node_id,
                    path: step_path,
                    kind: value_kind_name(&value).to_string(),
                    sensitive,
                    codes: diagnostics[diag_start..]
                        .iter()
                        .map(|d| d.code.clone())
                        .collect(),
                });
            }
            if let Some(out) = seeds.as_deref_mut() {
                if let Some(profile) = secret_profile.as_ref() {
                    let structural = profile.node_is_secret(node_id);
                    let path_material = profile.node_is_path_material(node_id)
                        && !profile.node_is_export_visible(node_id);
                    if structural || path_material {
                        let text = match &value {
                            Value::Secret(text) => Some(text.to_string()),
                            Value::String(text) => Some(text.clone()),
                            _ => None,
                        };
                        // A short path-material value would corrupt
                        // unrelated strings; structural secrets of any
                        // length still seed.
                        if let Some(text) = text {
                            if structural || text.len() >= 4 {
                                out.insert(text);
                            }
                        }
                    }
                }
            }
            values.insert(node_id, value);
        }

        // Render the collected steps once the sensitive set is complete,
        // so evaluation order cannot leak a secret into another step.
        if let Some(list) = steps {
            let renderer = secret_profile
                .as_ref()
                .map(|profile| StepRenderer::new(profile, &sensitive_values));
            for step in pending_steps {
                let Some(value) = values.get(&step.node_id) else {
                    continue;
                };
                let (mut rendered, material) = match renderer.as_ref() {
                    Some(renderer) => renderer.render(step.path.as_deref(), value, step.sensitive),
                    None => (crate::convert::value_to_json(value), false),
                };
                let replaced = sensitive_values.redact(&mut rendered);
                // The path is display text built from authored keys; a
                // material key inside it is redacted like any other copy.
                let path = step.path.map(|path| {
                    renderer
                        .as_ref()
                        .map(|renderer| renderer.profile.redact_path(&path))
                        .unwrap_or(path)
                });
                list.push(StepEvent {
                    node: step.node_id.0,
                    path,
                    kind: step.kind,
                    tainted: step.sensitive || material || replaced || value_has_secret(value),
                    value: rendered,
                    codes: step.codes,
                });
            }
        }

        let root = module.root.ok_or(EvalError::MissingRoot)?;
        let result = values.get(&root).cloned().unwrap_or(Value::Null);
        let transforms = TransformCtx {
            cone,
            budget,
            profile: secret_profile.as_ref(),
            env: &self.env,
        };
        let result = self.apply_profile(
            module,
            &values,
            result,
            &mut diagnostics,
            &mut account,
            transforms,
        )?;
        let mut result = result;
        self.apply_patches(
            module,
            &values,
            &mut result,
            &mut diagnostics,
            &mut account,
            transforms,
        )?;
        // Patch edits compose arena values at paths and can deepen the
        // tree past every statically-checked root (profile merges only
        // replace key-wise and cannot deepen). Re-check result depth
        // iteratively when patches ran; the tree is alias-free (every
        // Ref clone materializes), so a plain walk stays linear.
        if !module.patches.is_empty() && budget.max_depth != 0 {
            check_result_depth(&result, budget.max_depth)?;
        }
        // Schema validation runs last: post-evaluation, post-profile,
        // post-patch, pre-export (spec/validation.md §1). Resilient
        // mode may insert schema defaults (C-032), so the tree is
        // mutable here. Constraint predicates run after schema
        // validation (spec/annotations.md §3). Both share the
        // evaluation deadline computed before the walk
        // (spec/budgets.md §2). `validate` is false only for the
        // repair planner's value snapshot (L-P53), which needs the tree
        // while the diagnostics are exactly what it plans from.
        if validate {
            self.validate_schema(
                module,
                &values,
                &mut result,
                &mut diagnostics,
                BudgetCx {
                    deadline,
                    wall_limit: budget.max_wall_ms,
                    account: &mut account,
                    material: secret_profile.as_ref(),
                    env: &self.env,
                },
                demand,
            )?;
            self.validate_annotations(
                module,
                &values,
                &result,
                &mut diagnostics,
                deadline,
                budget.max_wall_ms,
            )?;
            // L-P19: root-schema cross-field checks run after annotations,
            // against the same final tree.
            self.validate_checks(
                module,
                &values,
                &result,
                &mut diagnostics,
                deadline,
                budget.max_wall_ms,
                demand,
                &mut account,
            )?;
        }
        // LANG-F-004: the envelope's `tainted` is a sender assertion about
        // its PAYLOAD; the receiver verifies it against that subtree's
        // structural taint, not the whole document (an unrelated root
        // secret must not fail a clean payload). A claim that cannot be
        // verified - missing, malformed, or without a payload - fails
        // closed with E035. Run here rather than in schema validation
        // because an envelope can exist without a schema block.
        use crate::envelope::TaintClaim;
        // LANG-F-004: a payload can carry material that is not
        // Secret-kinded — a plain value used as a `secret(...)` locator
        // (a `let`, a table field, an env lookup) or a runtime copy of
        // one. Verification therefore consults the same structural
        // material set every export uses, built once and reused for the
        // diagnostic redaction pass below.
        let has_secret = module
            .arena
            .iter()
            .any(|node| matches!(node, crate::ir::Node::Secret { .. }));
        let mut material_profile: Option<SecretProfile> = None;
        // Build the material profile whenever the document can carry
        // material, even if the SOURCE has no envelope: a profile/patch
        // can inject one, and the post-check hashes/verifies its payload
        // (review H-02).
        if has_secret || !module.profiles.is_empty() {
            let _ = material_profile.get_or_insert_with(|| {
                SecretProfile::with_context(
                    module,
                    self.profile_override.as_deref(),
                    Some(&self.env),
                    seeds.as_deref(),
                )
            });
        }
        let taint_error = match crate::envelope::sender_taint(module) {
            TaintClaim::NoEnvelope => None,
            TaintClaim::Missing => Some(
                "envelope has no `tainted` claim: a receiver cannot verify the payload's taint"
                    .to_string(),
            ),
            TaintClaim::Malformed => Some(
                "envelope.tainted is not a literal boolean: the payload's taint cannot be verified"
                    .to_string(),
            ),
            TaintClaim::Claim(claimed) => match envelope_payload(&result) {
                None => Some(
                    "envelope has no `payload` member: the taint claim cannot be verified"
                        .to_string(),
                ),
                Some(payload) => {
                    if has_secret || !module.profiles.is_empty() {
                        let _ = material_profile.get_or_insert_with(|| {
                            SecretProfile::with_context(
                                module,
                                self.profile_override.as_deref(),
                                Some(&self.env),
                                seeds.as_deref(),
                            )
                        });
                    }
                    let actual = value_has_secret(payload)
                        || material_profile.as_ref().is_some_and(|profile| {
                            // Both sources count: material content
                            // anywhere in the payload (text, a blob's
                            // media type or retained bytes), and a path
                            // the export redacts whatever kind sits there.
                            payload_has_material_content(payload, profile)
                                || payload_has_material_path(payload, profile, "envelope.payload")
                        });
                    if claimed == actual {
                        None
                    } else {
                        Some(format!(
                            "envelope.tainted is {claimed} but the evaluated payload is {actual} \
                             (a tainted claim must match the payload's structural taint)"
                        ))
                    }
                }
            },
        };
        if let Some(message) = taint_error {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "E035".into(),
                message,
                path: Some("envelope.tainted".into()),
                span: None,
                repair: None,
                confidence: 1.0,
                file: None,
            });
            if !self.resilient {
                // Redact carried schema/annotation diagnostics before the
                // early return; the taint refusal must not leak material
                // through an adjacent message (review M8).
                if let Some(profile) = &material_profile {
                    for diagnostic in &mut diagnostics {
                        if let Some(path) = &diagnostic.path {
                            diagnostic.path = Some(profile.redact_path(path));
                        }
                        if let Some(text) = profile.redact_text(&diagnostic.message) {
                            diagnostic.message = text;
                        }
                    }
                }
                return Err(EvalError::ValidationFailed { diagnostics });
            }
        }
        // Post-transform envelope verification (L-T06 review C1/C2):
        // profiles and patches can rewrite or inject `envelope`, so the
        // evaluated tree is checked again. Failures are fatal in both
        // modes: a receiver must not act on an unverifiable envelope.
        if let Some(gate) = &self.envelope_gate {
            let effective_profile = self.profile_override.clone().or_else(|| match &result {
                Value::Table(map) => match map.get("active_profile") {
                    Some(Value::String(name)) => Some(name.clone()),
                    _ => None,
                },
                _ => None,
            });
            let context = crate::envelope::CheckContext {
                held: self.capabilities,
                granted,
                budget: &budget,
                profile: effective_profile.as_deref(),
                material: material_profile.as_ref(),
                trust: self.envelope_trust.as_ref(),
            };
            crate::envelope::check_evaluated(&result, module, gate, &context)?;
        }
        // Closing wall-clock sample: small documents take zero walk
        // samples, so the deadline is checked once more at the end.
        if let Some(end) = deadline {
            if Instant::now() >= end {
                return Err(EvalError::budget_exceeded(
                    BudgetKind::WallClock,
                    budget.max_wall_ms,
                    module,
                    None,
                ));
            }
        }
        // Diagnostics can quote schema prose (a media type, format, or
        // requires target). Redact authored text so every consumer —
        // CLI check/eval, LSP diagnostics, repair findings — sees the
        // same safe message. The analysis is skipped when the document
        // defines no secret at all.
        if has_secret || !module.profiles.is_empty() {
            let profile = match material_profile {
                // Built above for the taint claim, with the runtime seeds.
                Some(profile) => profile,
                None => SecretProfile::with_context(
                    module,
                    self.profile_override.as_deref(),
                    Some(&self.env),
                    seeds.as_deref(),
                ),
            };
            // Host and computed values are not module literals; seed a
            // runtime redactor from the evaluated nodes as well.
            let runtime = crate::validate::runtime_secret_redactor(&values, &profile);
            for diagnostic in &mut diagnostics {
                if let Some(path) = &diagnostic.path {
                    let redacted = profile.redact_path(path);
                    diagnostic.path = Some(
                        runtime
                            .redact_every_occurrence(&redacted)
                            .unwrap_or(redacted),
                    );
                }
                if let Some(repair) = &mut diagnostic.repair {
                    profile.redact_repair(repair);
                    runtime.redact_repair(repair);
                }
                if let Some(message) = profile.redact_authored_text(&diagnostic.message) {
                    diagnostic.message = message;
                }
                if let Some(message) = runtime.redact_every_occurrence(&diagnostic.message) {
                    diagnostic.message = message;
                }
            }
        }
        crate::trace::record(
            "eval_done",
            crate::trace::count_nodes(&result),
            &diagnostics,
        );
        // Report what this demand cost (L-P40 §3.4): the accountant's
        // final spend against the cumulative ceiling, so a caller sees
        // the charge without a second evaluation.
        if let Some(out) = charge_out {
            // The printed total is the report-side balance: material bytes
            // sized at the marker, so it never varies with a secret's length
            // (C2). Enforcement still ran on `spent`.
            *out = ChargeReport {
                total: account.reported,
                limit: account.limit,
            };
        }
        Ok((result, diagnostics))
    }

    /// Evaluate with per-value layer provenance (LANG-F-014): the returned
    /// map records which layer (`profile:<name>`, `patch:<op>`) wrote each
    /// merged path last. The value tree and diagnostics are exactly those
    /// of [`Engine::evaluate`]; this is bookkeeping, not a semantic change.
    /// The map is empty when no profile or patch contributed.
    pub fn evaluate_with_layers(
        &self,
        module: &Module,
    ) -> Result<(Value, Vec<Diagnostic>, LayerMap), EvalError> {
        let owned = crate::layers::begin();
        let outcome = self.evaluate(module);
        let layers = if owned {
            crate::layers::finish()
        } else {
            LayerMap::new()
        };
        outcome.map(|(value, diagnostics)| (value, diagnostics, layers))
    }

    /// Evaluate and return the L-P09 merge provenance (path -> winner
    /// and losing source identities). The map is bounded and never
    /// contains values.
    pub fn evaluate_with_merges(
        &self,
        module: &Module,
    ) -> Result<(Value, Vec<Diagnostic>, MergeProvenance), EvalError> {
        let owned = crate::unify::begin_recording();
        let outcome = self.evaluate(module);
        let provenance = crate::unify::finish_recording(owned).unwrap_or_default();
        outcome.map(|(value, diagnostics)| (value, diagnostics, provenance))
    }

    /// Layers plus merge provenance in one evaluation (inspect).
    pub fn evaluate_with_layers_and_merges(
        &self,
        module: &Module,
    ) -> Result<(Value, Vec<Diagnostic>, LayerMap, MergeProvenance), EvalError> {
        self.evaluate_with_layers_merges_and_seeds(module)
            .map(|(value, diagnostics, layers, merges, _)| (value, diagnostics, layers, merges))
    }

    /// One evaluation returning the tree, layers, merge provenance, and
    /// runtime secret seeds (inspect needs all four; a second evaluation
    /// would double the wall-clock cost and could diverge).
    pub fn evaluate_with_layers_merges_and_seeds(
        &self,
        module: &Module,
    ) -> Result<
        (
            Value,
            Vec<Diagnostic>,
            LayerMap,
            MergeProvenance,
            crate::secret::SecretRedactor,
        ),
        EvalError,
    > {
        let owned_layers = crate::layers::begin();
        let owned_merges = crate::unify::begin_recording();
        let (outcome, seeds) = self.evaluate_collecting_seeds(module);
        let layers = if owned_layers {
            crate::layers::finish()
        } else {
            LayerMap::new()
        };
        let provenance = crate::unify::finish_recording(owned_merges).unwrap_or_default();
        outcome.map(|(value, diagnostics)| (value, diagnostics, layers, provenance, seeds))
    }

    /// The demanded form of [`Engine::evaluate_with_layers_merges_and_seeds`]
    /// (L-P40 §2.4): `inspect --path P` walks the cone `P` and still needs
    /// the layer, merge, and seed provenance its report renders, so one
    /// demanded walk supplies all four (a second pass would double the wall
    /// cost and could diverge on a marginal budget). `Demand::All` reproduces
    /// the whole-document result byte for byte.
    pub fn evaluate_with_layers_merges_and_seeds_demanded(
        &self,
        module: &Module,
        demand: &Demand,
    ) -> Result<
        (
            Value,
            Vec<Diagnostic>,
            LayerMap,
            MergeProvenance,
            crate::secret::SecretRedactor,
        ),
        EvalError,
    > {
        let closure = self.demand_closure(module, demand);
        let owned_layers = crate::layers::begin();
        let owned_merges = crate::unify::begin_recording();
        let mut seeds = crate::secret::SecretRedactor::new();
        let outcome =
            self.evaluate_inner_demanded(module, &closure, None, None, Some(&mut seeds), None);
        let layers = if owned_layers {
            crate::layers::finish()
        } else {
            LayerMap::new()
        };
        let provenance = crate::unify::finish_recording(owned_merges).unwrap_or_default();
        outcome.map(|(value, diagnostics)| (value, diagnostics, layers, provenance, seeds))
    }

    /// Reject a patch whose targets overlap an `@ai_immutable` path
    /// (spec/annotations.md §4, C-034). Every target is checked; the first
    /// overlap fails the whole patch application with E019 at the
    /// directive's span. Profiles and default fills are not patch edits
    /// and are unaffected.
    fn check_immutable(
        module: &Module,
        patch_id: NodeId,
        targets: &[&str],
    ) -> Result<(), EvalError> {
        for target in targets {
            if let Some(immutable) = Self::immutable_overlap(module, target) {
                return Err(EvalError::patch_error(
                    format!("patch target `{target}` overlaps ai_immutable `{immutable}`"),
                    module,
                    patch_id,
                ));
            }
        }
        Ok(())
    }

    /// First `@ai_immutable: true` annotation path overlapping `target` —
    /// in either direction, compared segment-wise so ancestor and
    /// descendant edits are both caught — or None. Annotation paths sort
    /// first so multi-overlap reports are deterministic. Unparseable
    /// paths on either side cannot overlap (patch paths were validated at
    /// parse; annotation paths are scope joins).
    fn immutable_overlap(module: &Module, target: &str) -> Option<String> {
        let target_segs = validate_patch_path(target).ok()?;
        let mut paths: Vec<&String> = module
            .annotations
            .values()
            .flatten()
            .filter(|a| matches!(&a.kind, crate::ir::AnnotationKind::AiImmutable(true)))
            .map(|a| &a.path)
            .collect();
        paths.sort();
        for apath in paths {
            let Ok(asegs) = validate_patch_path(apath) else {
                continue;
            };
            let shared = target_segs.len().min(asegs.len());
            if shared > 0 && target_segs[..shared] == asegs[..shared] {
                return Some(apath.clone());
            }
        }
        None
    }
    /// The static refusals a patch directive must pass before any tree
    /// work, and the path that decides cone membership. They read only
    /// the module — document-metadata rules, `ai_immutable` annotations,
    /// and path syntax, never an evaluated value — so a document the
    /// full evaluation refuses statically is refused for every cone
    /// (L-P40 §5, §8.2). Returns the directive's target path and its
    /// parsed segments for the in-cone application.
    fn check_patch_static(
        module: &Module,
        patch_id: NodeId,
        op: &PatchOp,
    ) -> Result<(String, Vec<PathSeg>), EvalError> {
        match op {
            PatchOp::Set { path, .. } | PatchOp::Remove { path } => {
                if path_targets_document_metadata(path) {
                    return Err(EvalError::patch_error(
                        format!("patch cannot rewrite document metadata `{path}`"),
                        module,
                        patch_id,
                    ));
                }
                Self::check_immutable(module, patch_id, &[path])?;
                let segs = validate_patch_path(path).map_err(|why| {
                    EvalError::patch_error(
                        format!("invalid patch path `{path}`: {why}"),
                        module,
                        patch_id,
                    )
                })?;
                Ok((path.clone(), segs))
            }
            PatchOp::Rename { from, to } => {
                // L-P41/M2: `to` is a plain leaf; checks that speak in
                // dotted paths (metadata, ai_immutable) need the full
                // destination under the source's parent. A host-built
                // module could pass a dotted leaf, which the source
                // parser rejects; refuse it here too.
                if to.is_empty() || to.contains('.') || to.contains('[') || to.contains(']') {
                    return Err(EvalError::patch_error(
                        format!("rename target `{to}` must be a plain key name"),
                        module,
                        patch_id,
                    ));
                }
                let to_path = crate::ir::rename_destination(from, to);
                if path_targets_document_metadata(from) || path_targets_document_metadata(&to_path)
                {
                    return Err(EvalError::patch_error(
                        format!("patch cannot rewrite document metadata `{from}` -> `{to_path}`"),
                        module,
                        patch_id,
                    ));
                }
                Self::check_immutable(module, patch_id, &[from, &to_path])?;
                let segs = validate_patch_path(from).map_err(|why| {
                    EvalError::patch_error(
                        format!("invalid patch path `{from}`: {why}"),
                        module,
                        patch_id,
                    )
                })?;
                Ok((from.clone(), segs))
            }
        }
    }

    /// Apply structural patch directives in source order (spec §2.9):
    /// after evaluation (and profile merge), before schema validation.
    /// Values were computed with the base tree intact, so a patch may
    /// reference the very key it overwrites. Missing remove/rename
    /// targets warn (W004); an existing rename target is E019.
    fn apply_patches(
        &self,
        module: &Module,
        values: &HashMap<NodeId, Value>,
        result: &mut Value,
        diagnostics: &mut Vec<Diagnostic>,
        account: &mut Accountant,
        ctx: TransformCtx<'_>,
    ) -> Result<(), EvalError> {
        use crate::ir::PatchOp;

        // Redaction-safe report size of a staged value: the enforcement
        // measure when no analysis ran, else the marker-clamped size, so a
        // patched-in secret never shows up as a length in `ChargeReport.total`
        // (C2). Without material the two agree, keeping the total unchanged.
        let report_size = |v: &Value| -> u64 {
            ctx.profile
                .map_or_else(|| measure_value(v), |p| measure_report_value(v, p, ctx.env))
        };

        // L-P41: validate-then-commit. The whole set is applied to a
        // staging clone; only when every directive succeeds does it
        // replace the caller's tree. A fatal directive (metadata rewrite,
        // immutable target, invalid path, rename collision) therefore
        // leaves the tree byte-identical, structurally rather than only
        // because the caller discards an Err. W004 stays a warning: a
        // blocked or missing target is skipped, not a failing directive.
        if module.patches.is_empty() {
            return Ok(());
        }
        // The staging materialisation is real work: charge it so a large
        // patched tree cannot clone for free.
        account.charge_scoped(module, None, measure_value(result), report_size(result))?;
        let mut staged = result.clone();

        for &patch_id in &module.patches {
            let (op, span, file) = match module.get(patch_id) {
                Some(Node::PatchDirective { op }) => (
                    op.clone(),
                    module.node_spans.get(&patch_id).cloned(),
                    module.node_files.get(&patch_id).cloned(),
                ),
                _ => continue,
            };
            // Static refusals run for every patch, in or out of the cone
            // (§5, §8.2). The cone then gates only the tree work: a patch
            // outside the cone writes to another path and cannot change a
            // demanded value, so a cone does not apply it (L-P40 §2.2.3).
            // A rename writes its destination as well as its source, so it
            // is demanded when either path is in the cone. A directive that
            // writes the envelope always runs: the envelope is always part
            // of the cone (it gates E035), so skipping it would leave the
            // gate reading an unpatched envelope (§2.2.4).
            let (target, segs) = Self::check_patch_static(module, patch_id, &op)?;
            let demanded = match (&op, ctx.cone) {
                (_, None) => true,
                (PatchOp::Rename { from, to }, Some(cone)) => {
                    let dest = crate::ir::rename_destination(from, to);
                    demand::path_in_cone(&target, cone)
                        || demand::path_in_cone(&dest, cone)
                        || crate::envelope::is_envelope_path(&target)
                        || crate::envelope::is_envelope_path(&dest)
                }
                (_, Some(cone)) => {
                    demand::path_in_cone(&target, cone)
                        || crate::envelope::is_envelope_path(&target)
                }
            };
            if !demanded {
                continue;
            }
            let mut warn = |code: &str, message: String| {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: code.into(),
                    message,
                    path: None,
                    span: span.clone(),
                    repair: None,
                    confidence: 1.0,
                    file: file.clone(),
                });
            };
            match op {
                PatchOp::Set { value, .. } => {
                    let v = values.get(&value).cloned().unwrap_or(Value::Null);
                    // Cloned values cost against the cumulative budget:
                    // many patches sharing one large value must trip
                    // like many aliases do. Path intermediates cost one
                    // unit per segment (input-proportioned tables).
                    account.charge_scoped(
                        module,
                        Some(patch_id),
                        measure_value(&v).saturating_add(segs.len() as u64),
                        report_size(&v).saturating_add(segs.len() as u64),
                    )?;
                    // Failed sets roll back: navigation may have
                    // materialized intermediate tables before hitting a
                    // blocked hop, and a failed patch must leave no trace.
                    // The snapshot is a real clone: charge it like one.
                    account.charge_scoped(
                        module,
                        Some(patch_id),
                        measure_value(&staged),
                        report_size(&staged),
                    )?;
                    let snapshot = staged.clone();
                    if !patch_set(&mut staged, &segs, v) {
                        staged = snapshot;
                        warn(
                            "W004",
                            format!("patch target `{target}` is blocked or out of bounds"),
                        );
                    } else if crate::layers::is_active() {
                        crate::layers::record(&target, "patch:set");
                    }
                }
                PatchOp::Remove { .. } => {
                    // L-P41: every directive pays for its own work; a
                    // flood of removes cannot run unbudgeted.
                    account.charge(module, Some(patch_id), segs.len() as u64)?;
                    if !patch_remove(&mut staged, &segs) {
                        warn("W004", format!("remove target `{target}` does not exist"));
                    }
                }
                PatchOp::Rename { to, .. } => {
                    // A rename walks the source and writes the new key:
                    // charge both.
                    account.charge(module, Some(patch_id), segs.len().saturating_add(1) as u64)?;
                    match patch_rename(&mut staged, &segs, &to) {
                        RenameOutcome::Ok => {}
                        RenameOutcome::Missing => {
                            warn("W004", format!("rename source `{target}` does not exist"));
                        }
                        RenameOutcome::Exists => {
                            return Err(EvalError::patch_error(
                                format!("rename target `{to}` already exists"),
                                module,
                                patch_id,
                            ));
                        }
                    }
                }
            }
        }
        // Every directive succeeded: commit the staged set in one move.
        *result = staged;
        Ok(())
    }

    /// Select and merge the active profile (spec §3.5). The `--profile`
    /// override wins; otherwise the root value's `active_profile` field
    /// decides. Missing key → base only. Unknown name or non-string →
    /// W002. A profile is the Profile priority over the base; type
    /// changes win without a warning (L-P09). Every profile evaluates
    /// whether selected or not (strictness catches rot in inactive
    /// environments); selection reads the BASE key only.
    fn apply_profile(
        &self,
        module: &Module,
        values: &HashMap<NodeId, Value>,
        result: Value,
        diagnostics: &mut Vec<Diagnostic>,
        account: &mut Accountant,
        ctx: TransformCtx<'_>,
    ) -> Result<Value, EvalError> {
        if module.profiles.is_empty() {
            // No profiles defined: the effective selector (override first,
            // then the key) still validates, then base stands.
            let selected: Option<String> = match &self.profile_override {
                Some(name) => Some(name.clone()),
                None => match &result {
                    Value::Table(map) => match map.get("active_profile") {
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(_) => {
                            diagnostics.push(Diagnostic {
                                severity: Severity::Warning,
                                code: "W002".into(),
                                message: "active_profile must be a string".into(),
                                path: Some("active_profile".into()),
                                span: if self.profile_override.is_some() {
                                    None
                                } else {
                                    active_key_span(module, &result)
                                },
                                repair: None,
                                confidence: 1.0,
                                file: if self.profile_override.is_some() {
                                    None
                                } else {
                                    active_key_file(module, &result)
                                },
                            });
                            return Ok(result);
                        }
                        None => None,
                    },
                    _ => None,
                },
            };
            if let Some(name) = selected {
                // With no profiles defined, any effective selector is unknown.
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "W002".into(),
                    message: format!(
                        "unknown profile {}",
                        profile_selector_label(module, &name, self.profile_override.is_some())
                    ),
                    path: Some("active_profile".into()),
                    span: if self.profile_override.is_some() {
                        None
                    } else {
                        active_key_span(module, &result)
                    },
                    repair: None,
                    confidence: 1.0,
                    file: if self.profile_override.is_some() {
                        None
                    } else {
                        active_key_file(module, &result)
                    },
                });
            }
            return Ok(result);
        }
        // A material selector must not choose between profiles: which
        // tree merges would reveal the value. Refuse before branching.
        if self.profile_override.is_none() {
            if let Some(root) = module.root {
                if let Some(Node::Table { fields }) = module.get(root) {
                    if let Some(selector) = fields.get("active_profile") {
                        let profile = crate::secret::SecretProfile::analyze(module);
                        if profile.node_is_secret(*selector)
                            || profile.path_is_secret("active_profile")
                        {
                            return Err(EvalError::ProfileError {
                                message:
                                    "active_profile selector is material; refusing to choose a profile"
                                        .into(),
                                span: module.node_spans.get(selector).cloned(),
                                file: module.node_files.get(selector).cloned(),
                            });
                        }
                    }
                }
            }
        }
        // Every profile is checked, selected or not: an inactive body
        // with metadata would otherwise fail only after --profile selects
        // it (strictness catches rot in inactive environments).
        for (profile_name, profile_node) in &module.profiles {
            if let Some(Node::Table { fields, .. }) = module.get(*profile_node) {
                if let Some(key) = fields.keys().find(|key| is_profile_metadata(key)) {
                    return Err(EvalError::ProfileError {
                        message: format!(
                            "profile \"{profile_name}\" cannot rewrite document metadata `{key}`"
                        ),
                        span: module.node_spans.get(profile_node).cloned(),
                        file: module.node_files.get(profile_node).cloned(),
                    });
                }
            }
        }
        let base_table = match &result {
            Value::Table(map) => map.clone(),
            _ => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "W002".into(),
                    message: "profiles cannot merge into a non-table root".into(),
                    path: Some("active_profile".into()),
                    span: None,
                    repair: None,
                    confidence: 1.0,
                    file: None,
                });
                return Ok(result);
            }
        };
        let selected: Option<String> = match &self.profile_override {
            Some(name) => Some(name.clone()),
            None => match base_table.get("active_profile") {
                None => return Ok(result),
                Some(Value::String(s)) => Some(s.clone()),
                Some(other) => {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        code: "W002".into(),
                        message: format!(
                            "active_profile must be a string, got {}",
                            value_kind_name(other)
                        ),
                        path: Some("active_profile".into()),
                        span: if self.profile_override.is_some() {
                            None
                        } else {
                            active_key_span(module, &result)
                        },
                        repair: None,
                        confidence: 1.0,
                        file: if self.profile_override.is_some() {
                            None
                        } else {
                            active_key_file(module, &result)
                        },
                    });
                    return Ok(result);
                }
            },
        };
        let name = match selected {
            Some(n) => n,
            None => return Ok(result),
        };
        let Some(&profile_id) = module.profiles.get(&name) else {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                code: "W002".into(),
                message: format!(
                    "unknown profile {}",
                    profile_selector_label(module, &name, self.profile_override.is_some())
                ),
                path: Some("active_profile".into()),
                span: if self.profile_override.is_some() {
                    None
                } else {
                    active_key_span(module, &result)
                },
                repair: None,
                confidence: 1.0,
                file: if self.profile_override.is_some() {
                    None
                } else {
                    active_key_file(module, &result)
                },
            });
            return Ok(result);
        };
        // Document metadata is fixed at parse; a profile must not
        // rewrite the mode, capability declaration, or schema contract
        // after the handshake read them. `active_profile` is allowed but
        // never reselects (spec §3.5).
        let mut profile_value = values.get(&profile_id).cloned().unwrap_or(Value::Null);
        if let Value::Table(map) = &mut profile_value {
            // The selector was read from the base; a profile value must
            // not change the exported selector.
            map.shift_remove("active_profile");
        }
        let base_value = Value::Table(base_table);
        let before = measure_merge_value(&base_value);
        let profile_size = measure_merge_value(&profile_value);
        let profile_label = profile_selector_label(module, &name, self.profile_override.is_some());
        // F-014 layers: record the profile-owned paths before the union
        // (scalars replaced, keys added; table/table pairs recurse).
        if crate::layers::is_active() {
            let layer = format!(
                "profile:{}",
                profile_label.trim_matches(|c| c == '\'' || c == '"')
            );
            if let Value::Table(base_table) = &base_value {
                record_profile_layers(base_table, &profile_value, &layer, "");
            }
        }
        // L-P09: a profile is the strongest layer (Profile over Local).
        // Tables union; a profile scalar replaces the base scalar; no
        // override warning (provenance records the winner in phase 5).
        let base_source =
            crate::unify::MergeSource::new(crate::unify::MergePriority::Local, None, 0, "base");
        let profile_source = crate::unify::MergeSource::new(
            crate::unify::MergePriority::Profile,
            module.node_files.get(&profile_id).cloned(),
            module
                .node_spans
                .get(&profile_id)
                .map_or(0, |span| span.line),
            format!("profile {profile_label}"),
        );
        // Engine ceilings apply to the profile union, like any merge.
        let limits = crate::unify::MergeLimits {
            max_depth: if ctx.budget.max_depth == 0 {
                crate::unify::MAX_MERGE_DEPTH
            } else {
                (ctx.budget.max_depth as usize).min(crate::unify::MAX_MERGE_DEPTH)
            },
            max_nodes: if ctx.budget.max_nodes == 0 {
                crate::unify::MAX_MERGE_NODES
            } else {
                ctx.budget.max_nodes.min(crate::unify::MAX_MERGE_NODES)
            },
            max_bytes: if ctx.budget.max_total_value_size == 0 {
                crate::unify::MAX_MERGE_BYTES
            } else {
                ctx.budget
                    .max_total_value_size
                    .min(crate::unify::MAX_MERGE_BYTES)
            },
        };
        let mut provenance = MergeProvenance::new();
        let merged = crate::unify::unify_with_limits(
            &base_value,
            &base_source,
            &profile_value,
            &profile_source,
            "",
            &mut provenance,
            &limits,
        )
        .map_err(|error| match error {
            MergeError::Budget(budget) => {
                let kind = match budget.kind {
                    MergeBudgetKind::Depth => BudgetKind::Depth,
                    MergeBudgetKind::Nodes => BudgetKind::Nodes,
                    MergeBudgetKind::Bytes => BudgetKind::TotalSize,
                };
                EvalError::budget_exceeded(kind, budget.limit, module, Some(profile_id))
            }
            MergeError::Conflict(conflict) => EvalError::ProfileError {
                message: conflict.message(),
                span: module.node_spans.get(&profile_id).cloned(),
                file: module.node_files.get(&profile_id).cloned(),
            },
        })?;
        crate::unify::record(&provenance);
        let after = measure_merge_value(&merged);
        if ctx.budget.max_value_size != 0 && after > ctx.budget.max_value_size {
            return Err(EvalError::budget_exceeded(
                BudgetKind::ValueSize,
                ctx.budget.max_value_size,
                module,
                Some(profile_id),
            ));
        }
        // Charge the larger of the copied profile and the true growth. The
        // enforcement balance pays the real bytes; the printed total pays a
        // marker for every material byte, so the outcome cannot reveal a
        // secret's length (C2). A document with no profile analysed reports
        // the merge measure itself, keeping its total unchanged.
        let report_size = |v: &Value| -> u64 {
            ctx.profile.map_or_else(
                || measure_merge_value(v),
                |p| measure_report_value(v, p, ctx.env),
            )
        };
        account.charge_scoped(
            module,
            Some(profile_id),
            profile_size.max(after.saturating_sub(before)),
            report_size(&profile_value)
                .max(report_size(&merged).saturating_sub(report_size(&base_value))),
        )?;
        Ok(merged)
    }
}

/// Root metadata keys the parser resolves before evaluation. Patches,
/// profiles, and resilient fills may not rewrite them: the exported
/// marker must match the mode/capability/profile set the host saw, and
/// `schema` is the contract, not data.
pub(crate) fn is_document_metadata(key: &str) -> bool {
    matches!(
        key,
        "active_mode" | "active_capabilities" | "active_profile" | "schema"
    )
}

/// Keys a profile body may not rewrite. `active_profile` is allowed but
/// ignored for selection and stripped from the exported overlay.
pub(crate) fn is_profile_metadata(key: &str) -> bool {
    matches!(key, "active_mode" | "active_capabilities" | "schema")
}

/// True when a patch path targets document metadata or anything under
/// it. `schema.hacked` is not the key `schema`, but writing it still
/// fabricates the metadata table the host resolved from the source.
pub(crate) fn path_targets_document_metadata(path: &str) -> bool {
    let root = path.split(['.', '[']).next().unwrap_or(path);
    is_document_metadata(root)
}

/// Diagnostic-safe profile selector: the literal the document spells,
/// a CLI override, or `(computed selector)`. A computed selector's
/// value may be material, so it is never echoed.
fn profile_selector_label(module: &Module, name: &str, override_active: bool) -> String {
    if override_active || crate::secret::active_profile_name(module).as_deref() == Some(name) {
        format!("'{name}'")
    } else {
        "(computed selector)".to_string()
    }
}

/// Span of the root `active_profile` key's node, when it exists.
fn active_key_span(module: &Module, result: &Value) -> Option<crate::diag::Span> {
    let Value::Table(_) = result else {
        return None;
    };
    let root = module.root?;
    let Node::Table { fields } = module.get(root)? else {
        return None;
    };
    let id = fields.get("active_profile")?;
    module.node_spans.get(id).cloned()
}

/// File of the root `active_profile` key's node, when recorded.
fn active_key_file(module: &Module, result: &Value) -> Option<String> {
    let Value::Table(_) = result else {
        return None;
    };
    let root = module.root?;
    let Node::Table { fields } = module.get(root)? else {
        return None;
    };
    let id = fields.get("active_profile")?;
    module.node_files.get(id).cloned()
}

/// Short kind name for diagnostics (never includes values, so secrets
/// cannot leak through it). Numbers share a kind; quantities carry
/// their unit so different dimensions compare unequal.
pub(crate) fn value_kind_name(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "boolean".into(),
        Value::Int(_) | Value::Float(_) => "number".into(),
        Value::String(_) => "string".into(),
        Value::DurationMs(_) => "duration".into(),
        Value::Quantity(_, unit) => format!("quantity:{}", unit.suffix()),
        Value::Array(_) => "array".into(),
        Value::Table(_) => "table".into(),
        Value::Secret(_) => "secret".into(),
        Value::Blob { .. } => "blob".into(),
    }
}

/// Record F-014 layer provenance for paths a profile owns: an added key or
/// a scalar replacement is profile-owned; table/table pairs recurse.
fn record_profile_layers(
    base: &indexmap::IndexMap<String, Value>,
    overlay: &Value,
    layer: &str,
    path: &str,
) {
    let Value::Table(fields) = overlay else {
        return;
    };
    for (key, value) in fields {
        let child = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        match (base.get(key), value) {
            (Some(Value::Table(base_sub)), Value::Table(_)) => {
                // The profile contributes to the parent table as well as
                // its leaves; inspect at the parent path must show the
                // profile layer.
                crate::layers::record(&child, layer);
                record_profile_layers(base_sub, value, layer, &child);
            }
            (_, Value::Table(fields)) => {
                // A scalar-to-table replacement owns the whole subtree.
                crate::layers::record(&child, layer);
                record_profile_subtree(fields, layer, &child);
            }
            (_, Value::Array(items)) => {
                crate::layers::record(&child, layer);
                for (index, item) in items.iter().enumerate() {
                    let item_path = format!("{child}[{index}]");
                    crate::layers::record(&item_path, layer);
                    if let Value::Table(nested) = item {
                        record_profile_subtree(nested, layer, &item_path);
                    }
                }
            }
            _ => crate::layers::record(&child, layer),
        }
    }
}

fn record_profile_subtree(fields: &indexmap::IndexMap<String, Value>, layer: &str, path: &str) {
    for (key, value) in fields {
        let child = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        crate::layers::record(&child, layer);
        match value {
            Value::Table(nested) => record_profile_subtree(nested, layer, &child),
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    let item_path = format!("{child}[{index}]");
                    crate::layers::record(&item_path, layer);
                    if let Value::Table(nested) = item {
                        record_profile_subtree(nested, layer, &item_path);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Outcome of a rename application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameOutcome {
    Ok,
    Missing,
    Exists,
}

/// Navigate to the parent of a path's final segment. With `create`,
/// missing intermediate keys materialize as tables (or arrays when the
/// following segment indexes); without it, navigation is read-only and
/// anything missing returns None. A scalar blocking the way, or an
/// array index that does not exist, always returns None (no sparse
/// extension, no clobbering). Read-only navigation keeps failed
/// remove/rename operations side-effect free (spec §2.9).
fn patch_parent<'a>(root: &'a mut Value, segs: &[PathSeg], create: bool) -> Option<&'a mut Value> {
    let mut cur = root;
    let last = segs.len().checked_sub(1)?;
    let mut i = 0;
    while i < last {
        match &segs[i] {
            PathSeg::Key(k) => {
                let need_array = matches!(segs.get(i + 1), Some(PathSeg::Index(_)));
                cur = match cur {
                    Value::Table(map) => {
                        if !create && !map.contains_key(k) {
                            return None;
                        }
                        map.entry(k.clone()).or_insert_with(|| {
                            if need_array {
                                Value::Array(Vec::new())
                            } else {
                                Value::Table(indexmap::IndexMap::new())
                            }
                        })
                    }
                    _ => return None,
                };
            }
            PathSeg::Index(idx) => {
                cur = match cur {
                    Value::Array(items) => items.get_mut(*idx)?,
                    _ => return None,
                };
            }
        }
        i += 1;
    }
    Some(cur)
}

/// Set the value at `path`, creating missing intermediate tables.
/// Returns false when the path is blocked (scalar in the way) or an
/// array index does not exist.
fn patch_set(root: &mut Value, segs: &[PathSeg], value: Value) -> bool {
    let Some(last) = segs.last() else {
        return false;
    };
    let Some(parent) = patch_parent(root, segs, true) else {
        return false;
    };
    match (parent, last) {
        (Value::Table(map), PathSeg::Key(k)) => {
            map.insert(k.clone(), value);
            true
        }
        (Value::Array(items), PathSeg::Index(i)) => {
            if let Some(slot) = items.get_mut(*i) {
                *slot = value;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Delete the value at `path`. Array removals shift later elements.
/// Returns false when the target does not exist.
fn patch_remove(root: &mut Value, segs: &[PathSeg]) -> bool {
    let Some(last) = segs.last() else {
        return false;
    };
    let Some(parent) = patch_parent(root, segs, false) else {
        return false;
    };
    match (parent, last) {
        (Value::Table(map), PathSeg::Key(k)) => map.shift_remove(k).is_some(),
        (Value::Array(items), PathSeg::Index(i)) if *i < items.len() => {
            items.remove(*i);
            true
        }
        _ => false,
    }
}

/// Rename a key within its parent table. Returns Missing when the source
/// path does not resolve (checked before the target, so a missing
/// source warns W004 even when the target exists), Exists when the new
/// name is taken.
fn patch_rename(root: &mut Value, segs: &[PathSeg], new_name: &str) -> RenameOutcome {
    let Some(last) = segs.last() else {
        return RenameOutcome::Missing;
    };
    // Renaming an array element is meaningless: only plain keys rename.
    if !matches!(last, PathSeg::Key(_)) {
        return RenameOutcome::Missing;
    }
    let Some(parent) = patch_parent(root, segs, false) else {
        return RenameOutcome::Missing;
    };
    let Value::Table(map) = parent else {
        return RenameOutcome::Missing;
    };
    let PathSeg::Key(old) = last else {
        return RenameOutcome::Missing;
    };
    if !map.contains_key(old) {
        return RenameOutcome::Missing;
    }
    if map.contains_key(new_name) {
        return RenameOutcome::Exists;
    }
    match map.shift_remove(old) {
        Some(v) => {
            map.insert(new_name.to_string(), v);
            RenameOutcome::Ok
        }
        None => RenameOutcome::Missing,
    }
}

fn normalize_quantity(v: &Value, unit: &Unit) -> Result<Value, String> {
    let num = match v {
        Value::Int(n) => *n as f64,
        Value::Float(f) => *f,
        _ => {
            return Err(format!(
                "quantity requires numeric value, got {}",
                value_kind_name(v)
            ));
        }
    };

    if let Some(ms) = unit.normalize_time_to_ms(num) {
        return Ok(Value::DurationMs(ms));
    }
    if let Some(bytes) = unit.normalize_to_bytes(num) {
        // Represent bytes as Int for simplicity in the alpha.
        // Saturate: u64 byte counts past i64::MAX must not wrap
        // negative through `as` (found by audit: `20000000TiB`).
        return Ok(Value::Int(i64::try_from(bytes).unwrap_or(i64::MAX)));
    }
    // Rates and custom units retain their dimension (e.g. 100/req/s).
    // Percent canonicalizes to the dimensionless ratio (100% = 1.0);
    // values are NOT clamped: negative and over-100 percents are meaningful
    // (spec Ch. 4 §4.1).
    if *unit == Unit::Percent {
        return quantity_result(num / 100.0, Unit::Percent);
    }
    // Frequency canonicalizes to f64 hertz (fractional Hz are real).
    // Negative frequencies clamp at zero, mirroring time canonicalization
    // (spec Ch. 4 §4.1).
    let hz_factor = match unit {
        Unit::Hertz => Some(1.0),
        Unit::Kilohertz => Some(1_000.0),
        Unit::Megahertz => Some(1_000_000.0),
        Unit::Gigahertz => Some(1_000_000_000.0),
        Unit::Terahertz => Some(1_000_000_000_000.0),
        _ => None,
    };
    if let Some(factor) = hz_factor {
        return quantity_result((num * factor).max(0.0), Unit::Hertz);
    }
    // Bits canonicalize to f64 bits; data rates to f64 bits/sec with MB/s
    // converting through megabytes (1 MB/s = 8,000,000 bps), so cross-spelled
    // rates compare equal. Literal magnitudes clamp at zero.
    let bit_factor = match unit {
        Unit::Bits => Some((1.0, false)),
        Unit::Kilobits => Some((1_000.0, false)),
        Unit::Megabits => Some((1_000_000.0, false)),
        Unit::Gigabits => Some((1_000_000_000.0, false)),
        Unit::BitsPerSecond => Some((1.0, true)),
        Unit::KilobitsPerSecond => Some((1_000.0, true)),
        Unit::MegabitsPerSecond => Some((1_000_000.0, true)),
        Unit::GigabitsPerSecond => Some((1_000_000_000.0, true)),
        Unit::MegabytesPerSecond => Some((8_000_000.0, true)),
        _ => None,
    };
    if let Some((factor, is_rate)) = bit_factor {
        let canonical = if is_rate {
            Unit::BitsPerSecond
        } else {
            Unit::Bits
        };
        return quantity_result((num * factor).max(0.0), canonical);
    }
    // AI-age units: token counts, inference throughput, compute. Longest-match
    // prefix composition is inherent (the lexer consumes the maximal run and
    // the whitelist carries each spelling). Literal magnitudes clamp at zero.
    let ai_factor = match unit {
        Unit::Tokens => Some((1.0, Unit::Tokens)),
        Unit::KiloTokens => Some((1_000.0, Unit::Tokens)),
        Unit::MegaTokens => Some((1_000_000.0, Unit::Tokens)),
        Unit::TokensPerSecond => Some((1.0, Unit::TokensPerSecond)),
        Unit::Flops => Some((1.0, Unit::Flops)),
        Unit::TeraFlops => Some((1_000_000_000_000.0, Unit::Flops)),
        Unit::PetaFlops => Some((1_000_000_000_000_000.0, Unit::Flops)),
        _ => None,
    };
    if let Some((factor, canonical)) = ai_factor {
        return quantity_result((num * factor).max(0.0), canonical);
    }
    // Domain SI units: pressure (Pa; bar = 1e5, psi = 6894.757293168),
    // electrical (V/A/W), energy (J; kWh = 3.6e6), force (N). Each
    // canonicalizes to its coherent SI value; literal magnitudes clamp.
    let si_factor = match unit {
        Unit::Pascal => Some((1.0, Unit::Pascal)),
        Unit::Bar => Some((100_000.0, Unit::Pascal)),
        Unit::Psi => Some((6_894.757_293_168, Unit::Pascal)),
        Unit::Volts => Some((1.0, Unit::Volts)),
        Unit::Amperes => Some((1.0, Unit::Amperes)),
        Unit::Watts => Some((1.0, Unit::Watts)),
        Unit::Joules => Some((1.0, Unit::Joules)),
        Unit::KilowattHours => Some((3_600_000.0, Unit::Joules)),
        Unit::Newtons => Some((1.0, Unit::Newtons)),
        _ => None,
    };
    if let Some((factor, canonical)) = si_factor {
        return quantity_result((num * factor).max(0.0), canonical);
    }
    // Angles (deg→rad), rotation (retained rpm), length (m), mass (kg).
    let mech_factor = match unit {
        Unit::Degrees => Some((std::f64::consts::PI / 180.0, Unit::Radians)),
        Unit::Radians => Some((1.0, Unit::Radians)),
        Unit::Rpm => Some((1.0, Unit::Rpm)),
        Unit::Meters => Some((1.0, Unit::Meters)),
        Unit::Kilometers => Some((1_000.0, Unit::Meters)),
        Unit::Kilograms => Some((1.0, Unit::Kilograms)),
        Unit::Grams => Some((0.001, Unit::Kilograms)),
        _ => None,
    };
    if let Some((factor, canonical)) = mech_factor {
        return quantity_result((num * factor).max(0.0), canonical);
    }
    // Temperature canonicalizes to kelvin through affine transforms.
    // A value below absolute zero is not a temperature: reject it rather
    // than clamping to 0 K, which would make `-500Cel` silently equal
    // absolute zero while `-5K` is refused (L-P44, inheriting B-011's
    // no-silent-clamp rule). Decibels are retained (log scale, no
    // conversion). Both are ordering-only (spec Ch. 4 §4.1).
    match unit {
        Unit::Kelvin => {
            if num < 0.0 {
                return Err(BELOW_ABSOLUTE_ZERO.to_string());
            }
            return quantity_result(num, Unit::Kelvin);
        }
        Unit::Celsius => {
            let kelvin = num + 273.15;
            if kelvin < 0.0 {
                return Err(BELOW_ABSOLUTE_ZERO.to_string());
            }
            return quantity_result(kelvin, Unit::Kelvin);
        }
        Unit::Fahrenheit => {
            let kelvin = (num - 32.0) * 5.0 / 9.0 + 273.15;
            if kelvin < 0.0 {
                return Err(BELOW_ABSOLUTE_ZERO.to_string());
            }
            return quantity_result(kelvin, Unit::Kelvin);
        }
        Unit::Decibel => return quantity_result(num, Unit::Decibel),
        _ => {}
    }
    quantity_result(num, unit.clone())
}

/// A temperature below 0 K is not a temperature (the B-011 rule applied
/// to the absolute-temperature floor).
const BELOW_ABSOLUTE_ZERO: &str = "absolute temperature below 0 K is not a temperature";

/// IEEE has two zeros; the language has one. Normalize the sign so the
/// canonical JSON export and content hash agree (0.0 == -0.0).
fn canonical_zero(v: f64) -> f64 {
    if v == 0.0 {
        0.0
    } else {
        v
    }
}

pub(crate) fn eval_binary(op: BinOp, lhs: &Value, rhs: &Value) -> Result<Value, String> {
    use BinOp::*;
    match (op, lhs, rhs) {
        // Integer arithmetic saturates: user input must never panic
        // (debug) or wrap (release). Consistent with the DurationMs
        // arms below and the budget code's saturating style.
        // `MIN / -1` is the only overflow division: checked, saturates.
        (Add, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.saturating_add(*b))),
        (Sub, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.saturating_sub(*b))),
        (Mul, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.saturating_mul(*b))),
        (Div, Value::Int(a), Value::Int(b)) if *b != 0 => {
            Ok(Value::Int(a.checked_div(*b).unwrap_or(i64::MAX)))
        }

        // A float result that cannot be represented is an error, not
        // an `inf` that later exports as null (LANG-B-901).
        (Add, Value::Float(a), Value::Float(b)) => float_result(a + b),
        (Sub, Value::Float(a), Value::Float(b)) => float_result(a - b),
        (Mul, Value::Float(a), Value::Float(b)) => float_result(a * b),
        (Div, Value::Float(a), Value::Float(b)) if *b != 0.0 => float_result(a / b),

        // Division by zero names itself (LANG-A-007): the generic
        // "unsupported binary op" text sent readers hunting for a type
        // error when the operands were fine and the divisor was zero.
        (Div, Value::Int(_), Value::Int(0)) => Err("division by zero".to_string()),
        (Div, Value::Float(_), Value::Float(b)) if *b == 0.0 => Err("division by zero".to_string()),
        (Div, Value::Int(_), Value::Float(b)) if *b == 0.0 => Err("division by zero".to_string()),
        (Div, Value::Float(_), Value::Int(0)) => Err("division by zero".to_string()),

        // Mixed int/float: the spec says mixed arithmetic promotes to
        // Float, so every operator gets both operand orders. Every
        // operand combination has its division-by-zero arm above.
        (Add, Value::Int(a), Value::Float(b)) => float_result(*a as f64 + b),
        (Add, Value::Float(a), Value::Int(b)) => float_result(a + *b as f64),
        (Sub, Value::Int(a), Value::Float(b)) => float_result(*a as f64 - b),
        (Sub, Value::Float(a), Value::Int(b)) => float_result(a - *b as f64),
        (Mul, Value::Int(a), Value::Float(b)) => float_result(*a as f64 * b),
        (Mul, Value::Float(a), Value::Int(b)) => float_result(a * *b as f64),
        (Div, Value::Int(a), Value::Float(b)) if *b != 0.0 => float_result(*a as f64 / b),
        (Div, Value::Float(a), Value::Int(b)) if *b != 0 => float_result(a / *b as f64),

        (Eq, a, b) => Ok(Value::Bool(value_eq(a, b))),
        (Ne, a, b) => Ok(Value::Bool(!value_eq(a, b))),

        (Lt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
        (Le, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
        (Gt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
        (Ge, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),

        // Ordering is numeric and promotes mixed Int/Float (spec section
        // 3.3). `cmp_int_float` compares exactly even beyond 2^53, where
        // a plain `as f64` conversion would round.
        (Lt, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
        (Le, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
        (Gt, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
        (Ge, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a >= b)),
        (Lt, Value::Int(a), Value::Float(b)) => Ok(Value::Bool(
            cmp_int_float(*a, *b) == Some(std::cmp::Ordering::Less),
        )),
        (Lt, Value::Float(a), Value::Int(b)) => Ok(Value::Bool(
            cmp_int_float(*b, *a) == Some(std::cmp::Ordering::Greater),
        )),
        (Le, Value::Int(a), Value::Float(b)) => Ok(Value::Bool(matches!(
            cmp_int_float(*a, *b),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ))),
        (Le, Value::Float(a), Value::Int(b)) => Ok(Value::Bool(matches!(
            cmp_int_float(*b, *a),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ))),
        (Gt, Value::Int(a), Value::Float(b)) => Ok(Value::Bool(
            cmp_int_float(*a, *b) == Some(std::cmp::Ordering::Greater),
        )),
        (Gt, Value::Float(a), Value::Int(b)) => Ok(Value::Bool(
            cmp_int_float(*b, *a) == Some(std::cmp::Ordering::Less),
        )),
        (Ge, Value::Int(a), Value::Float(b)) => Ok(Value::Bool(matches!(
            cmp_int_float(*a, *b),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ))),
        (Ge, Value::Float(a), Value::Int(b)) => Ok(Value::Bool(matches!(
            cmp_int_float(*b, *a),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ))),

        // Duration arithmetic (time is the only dimension with a dedicated
        // Value; bytes normalize to Int). Scalar mul/div, same-dimension
        // add/sub and ordering. A negative RESULT is not a duration
        // (L-P44, inheriting the B-011 pin: clamping it to 0ms turns an
        // authoring error into "expires immediately"), so it is T001.
        // Dimension mismatches fall through to the named error below.
        (Add, Value::DurationMs(a), Value::DurationMs(b)) => {
            // Overflow saturates at the representable maximum; that is a
            // deliberate overflow policy, not a sign clamp.
            Ok(Value::DurationMs(a.saturating_add(*b)))
        }
        (Sub, Value::DurationMs(a), Value::DurationMs(b)) => match a.checked_sub(*b) {
            Some(ms) => Ok(Value::DurationMs(ms)),
            None => Err(NEGATIVE_DURATION_SUBTRACTION.to_string()),
        },
        (Mul, Value::DurationMs(a), Value::Int(b)) => {
            duration_scaled(*a, *b as f64, NEGATIVE_DURATION_SCALE)
        }
        (Mul, Value::Int(a), Value::DurationMs(b)) => {
            duration_scaled(*b, *a as f64, NEGATIVE_DURATION_SCALE)
        }
        (Mul, Value::DurationMs(a), Value::Float(b)) => {
            duration_scaled(*a, *b, NEGATIVE_DURATION_SCALE)
        }
        (Mul, Value::Float(a), Value::DurationMs(b)) => {
            duration_scaled(*b, *a, NEGATIVE_DURATION_SCALE)
        }
        (Div, Value::DurationMs(a), Value::Int(b)) if *b != 0 => {
            duration_scaled(*a, 1.0 / *b as f64, NEGATIVE_DURATION_DIVISION)
        }
        (Div, Value::DurationMs(a), Value::Float(b)) if *b != 0.0 => {
            duration_scaled(*a, 1.0 / *b, NEGATIVE_DURATION_DIVISION)
        }
        (Div, Value::DurationMs(a), Value::DurationMs(b)) if *b != 0 => {
            // A ratio can exceed i64::MAX (both operands saturate at
            // u64::MAX); saturate the result rather than wrapping.
            Ok(Value::Int(i64::try_from(*a / *b).unwrap_or(i64::MAX)))
        }
        // A scalar divided by time is the per-second rate. Bytes are Int
        // at evaluation, so this is the bytes/duration case; the schema
        // type supplies the byte meaning. The result is Float because a
        // rate can be fractional (spec Ch. 4 §4.3, L-P44).
        (Div, Value::Int(a), Value::DurationMs(b)) if *b != 0 => {
            float_result(*a as f64 * 1000.0 / *b as f64)
        }
        (Div, Value::Float(a), Value::DurationMs(b)) if *b != 0 => {
            float_result(a * 1000.0 / *b as f64)
        }
        // A zero time divisor names the divisor, not a dimension: the
        // operands are scalar and time, and the operation is defined
        // except at zero (review F1).
        (Div, Value::Int(_), Value::DurationMs(0))
        | (Div, Value::Float(_), Value::DurationMs(0)) => Err("division by zero".to_string()),
        // Division by zero names itself, like the scalar arms above.
        (Div, Value::DurationMs(_), Value::Int(0)) => Err("division by zero".to_string()),
        (Div, Value::DurationMs(_), Value::Float(b)) if *b == 0.0 => {
            Err("division by zero".to_string())
        }
        (Div, Value::DurationMs(_), Value::DurationMs(0)) => Err("division by zero".to_string()),
        (Lt, Value::DurationMs(a), Value::DurationMs(b)) => Ok(Value::Bool(a < b)),
        (Le, Value::DurationMs(a), Value::DurationMs(b)) => Ok(Value::Bool(a <= b)),
        (Gt, Value::DurationMs(a), Value::DurationMs(b)) => Ok(Value::Bool(a > b)),
        (Ge, Value::DurationMs(a), Value::DurationMs(b)) => Ok(Value::Bool(a >= b)),

        // Same-suffix quantity arithmetic (percent, rates, custom):
        // same-dimension add/sub/compare, scalar mul/div, same-dimension
        // division yields a bare ratio. Cross-suffix operations fall through
        // to the dimensional TypeError below (spec Ch. 4 §4.1/§4.3).
        // Absolute temperatures and decibels are excluded from arithmetic
        // (affine/log values do not add); their comparisons below still
        // apply (Unit::is_non_arithmetic).
        (Add, Value::Quantity(a, ua), Value::Quantity(b, ub))
            if ua == ub && !ua.is_non_arithmetic() =>
        {
            quantity_result(a + b, ua.clone())
        }
        (Sub, Value::Quantity(a, ua), Value::Quantity(b, ub))
            if ua == ub && !ua.is_non_arithmetic() =>
        {
            quantity_result(a - b, ua.clone())
        }
        (Mul, Value::Quantity(a, u), Value::Int(n)) if !u.is_non_arithmetic() => {
            quantity_result(a * *n as f64, u.clone())
        }
        (Mul, Value::Quantity(a, u), Value::Float(f)) if !u.is_non_arithmetic() => {
            quantity_result(a * f, u.clone())
        }
        (Div, Value::Quantity(a, u), Value::Int(n)) if *n != 0 && !u.is_non_arithmetic() => {
            quantity_result(a / *n as f64, u.clone())
        }
        (Div, Value::Quantity(a, u), Value::Float(f)) if *f != 0.0 && !u.is_non_arithmetic() => {
            quantity_result(a / f, u.clone())
        }
        (Div, Value::Quantity(a, ua), Value::Quantity(b, ub))
            if ua == ub && *b != 0.0 && !ua.is_non_arithmetic() =>
        {
            float_result(a / b)
        }
        // Division by zero names itself for quantities too. The affine
        // guard runs first: arithmetic on a temperature is undefined
        // regardless of the divisor, so that rule wins (review F5).
        (Div, Value::Quantity(_, u), Value::Int(0)) if !u.is_non_arithmetic() => {
            Err("division by zero".to_string())
        }
        (Div, Value::Quantity(_, u), Value::Float(f)) if *f == 0.0 && !u.is_non_arithmetic() => {
            Err("division by zero".to_string())
        }
        (Div, Value::Quantity(_, u), Value::Quantity(b, _))
            if *b == 0.0 && !u.is_non_arithmetic() =>
        {
            Err("division by zero".to_string())
        }
        (Lt, Value::Quantity(a, ua), Value::Quantity(b, ub)) if ua == ub => Ok(Value::Bool(a < b)),
        (Le, Value::Quantity(a, ua), Value::Quantity(b, ub)) if ua == ub => Ok(Value::Bool(a <= b)),
        (Gt, Value::Quantity(a, ua), Value::Quantity(b, ub)) if ua == ub => Ok(Value::Bool(a > b)),
        (Ge, Value::Quantity(a, ua), Value::Quantity(b, ub)) if ua == ub => Ok(Value::Bool(a >= b)),

        // Absolute temperature and decibels are equality/ordering-only:
        // affine and logarithmic values do not compose (L-P44).
        (Add | Sub | Mul | Div, Value::Quantity(_, u), _) if u.is_non_arithmetic() => {
            Err(non_arithmetic_quantity_error(op, u))
        }
        (Add | Sub | Mul | Div, _, Value::Quantity(_, u)) if u.is_non_arithmetic() => {
            Err(non_arithmetic_quantity_error(op, u))
        }

        (And, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(*a && *b)),
        (Or, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(*a || *b)),

        _ => {
            // Dimension names only: the operands may be material assembled
            // from a secret source, so the message must be value-free. A
            // unit suffix is authored syntax, never a magnitude.
            let dimensional = matches!(lhs, Value::DurationMs(_) | Value::Quantity(..))
                || matches!(rhs, Value::DurationMs(_) | Value::Quantity(..));
            if dimensional {
                Err(format!(
                    "dimension mismatch: {op:?} combines {} and {}",
                    dimension_label(lhs),
                    dimension_label(rhs)
                ))
            } else {
                Err(format!(
                    "unsupported binary op {op:?} on {} and {}",
                    value_kind_name(lhs),
                    value_kind_name(rhs)
                ))
            }
        }
    }
}

/// The B-011 pin on the arithmetic surface (L-P44): a negative duration
/// result is rejected, never clamped to "expires immediately".
const NEGATIVE_DURATION_SUBTRACTION: &str =
    "duration subtraction produced a negative result (a duration is non-negative)";
const NEGATIVE_DURATION_SCALE: &str =
    "scaling a duration by a negative factor produced a negative duration";
const NEGATIVE_DURATION_DIVISION: &str =
    "dividing a duration by a negative factor produced a negative duration";

/// Dimension label for a binary operand in a diagnostic. `Int`/`Float`
/// are scalars (byte counts arrive here too), so the label never claims a
/// byte dimension the runtime does not carry.
fn dimension_label(v: &Value) -> String {
    match v {
        Value::DurationMs(_) => "time (duration)".to_string(),
        Value::Quantity(_, unit) => format!("quantity:{}", unit.suffix()),
        Value::Int(_) | Value::Float(_) => "scalar (number)".to_string(),
        other => value_kind_name(other),
    }
}

/// Arithmetic on an affine/logarithmic quantity names the rule that
/// refuses it rather than the units twice.
fn non_arithmetic_quantity_error(op: BinOp, unit: &Unit) -> String {
    let kind = if matches!(unit, Unit::Decibel) {
        "decibels (quantity:dB); logarithmic values have no arithmetic"
    } else {
        "absolute temperature (quantity:K); affine quantities have no arithmetic"
    };
    format!("{op:?} is not defined for {kind}")
}

/// Scale a duration in milliseconds by a factor. A negative result is
/// not a duration (the B-011 pin): return `None` so the caller rejects
/// it instead of clamping it to "expires immediately". Overflow still
/// saturates at the representable maximum (the documented policy).
enum ScaleError {
    /// The mathematically negative result is not a duration.
    Negative,
    /// A NaN factor has no representable duration result.
    Nan,
}

/// Scale a duration and map the failure to the operation's message.
fn duration_scaled(ms: u64, factor: f64, negative: &'static str) -> Result<Value, String> {
    match scale_dur(ms, factor) {
        Ok(ms) => Ok(Value::DurationMs(ms)),
        Err(ScaleError::Negative) => Err(negative.to_string()),
        Err(ScaleError::Nan) => Err("float operation produced NaN".to_string()),
    }
}

/// Scale a duration in milliseconds by a factor. A negative result is
/// not a duration (the B-011 pin): return `Negative` so the caller
/// rejects it instead of clamping it to "expires immediately". +inf
/// saturates at the representable maximum (the documented duration
/// overflow policy, the same as addition). A NaN factor is reported as
/// NaN, never as a negative result.
fn scale_dur(ms: u64, factor: f64) -> Result<u64, ScaleError> {
    // An integral multiplier is exact: multiplying through f64 loses
    // milliseconds beyond 2^53 (review round 3). A zero factor is
    // covered here (0 ms, whatever the sign of zero).
    if factor.fract() == 0.0 && factor >= 0.0 && factor <= u64::MAX as f64 {
        return Ok(ms.saturating_mul(factor as u64));
    }
    let scaled = ms as f64 * factor;
    // Test the unrounded product: a mathematically negative result stays
    // negative even when it would round to zero milliseconds.
    if scaled.is_nan() {
        Err(ScaleError::Nan)
    } else if scaled < 0.0 {
        Err(ScaleError::Negative)
    } else {
        Ok(scaled.round() as u64)
    }
}

#[cfg(test)]
mod dimension_rule_tests {
    use super::*;

    #[test]
    fn a_nan_scalar_is_reported_as_nan_not_a_negative_duration() {
        // A NaN value cannot come from source (LANG-B-009), but the type
        // step must not mislabel one as a sign error.
        let error = eval_binary(
            BinOp::Mul,
            &Value::DurationMs(1000),
            &Value::Float(f64::NAN),
        )
        .expect_err("NaN has no duration result");
        assert_eq!(error, "float operation produced NaN");
    }

    #[test]
    fn duration_scaling_saturates_on_overflow_like_addition() {
        // The documented duration overflow policy: saturation at the
        // representable maximum, never a wrap or a silent sign flip.
        let value = eval_binary(BinOp::Mul, &Value::DurationMs(1000), &Value::Float(1e300))
            .expect("overflow saturates");
        assert!(matches!(value, Value::DurationMs(u64::MAX)), "{value:?}");
    }

    #[test]
    fn negating_a_zero_duration_is_zero_but_a_nonzero_one_is_not_a_duration() {
        assert!(matches!(
            eval_unary(UnOp::Neg, &Value::DurationMs(0)),
            Ok(Value::DurationMs(0))
        ));
        assert!(eval_unary(UnOp::Neg, &Value::DurationMs(1)).is_err());
    }
}

pub(crate) fn eval_unary(op: UnOp, v: &Value) -> Result<Value, String> {
    match (op, v) {
        (UnOp::Neg, Value::Int(n)) => Ok(Value::Int(n.saturating_neg())),
        (UnOp::Neg, Value::Float(f)) => Ok(Value::Float(canonical_zero(-f))),
        // Quantities negate by amount, keeping the unit: discounts, deltas
        // and signed rates are meaningful, and nothing here clamps
        // (spec Ch. 4 §4.1). Durations intentionally do NOT get this arm.
        (UnOp::Neg, Value::Quantity(a, u)) => {
            let negated = -*a;
            // Absolute temperature has a physical floor: the 0 K clamp on
            // literals is defensive, and an explicit negation must not
            // bypass it into a below-zero temperature (L-P44 review F3).
            if matches!(u, Unit::Kelvin) {
                if negated < 0.0 {
                    return Err(
                        "negation produced a value below absolute zero (quantity:K)".to_string()
                    );
                }
                // -0 K is 0 K: normalize the sign of zero so the
                // canonical export (and content hash) matches 0 K.
                return Ok(Value::Quantity(
                    if negated == 0.0 { 0.0 } else { negated },
                    u.clone(),
                ));
            }
            Ok(Value::Quantity(canonical_zero(negated), u.clone()))
        }
        // A zero duration negated is still zero; any other negation is
        // not a duration (the B-011 pin).
        (UnOp::Neg, Value::DurationMs(0)) => Ok(Value::DurationMs(0)),
        (UnOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        _ => Err(format!(
            // Kind name only, for the same reason as binary errors.
            "unsupported unary op {op:?} on {}",
            value_kind_name(v)
        )),
    }
}

/// Exact ordering between an Int and a Float. `None` when the float is
/// NaN. A plain `i as f64` comparison rounds above 2^53, so this compares
/// the integral part as i64 and only looks at the fractional part when
/// the integer parts are equal.
pub(crate) fn cmp_int_float(i: i64, f: f64) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    if f.is_nan() {
        return None;
    }
    // Beyond the i64 range the ordering is decided by the sign alone.
    if f >= i64::MAX as f64 {
        return Some(Ordering::Less);
    }
    if f < i64::MIN as f64 {
        return Some(Ordering::Greater);
    }
    let trunc = f.trunc();
    let ti = trunc as i64;
    match i.cmp(&ti) {
        Ordering::Equal if f > trunc => Some(Ordering::Less),
        Ordering::Equal if f < trunc => Some(Ordering::Greater),
        other => Some(other),
    }
}

/// Wrap a float arithmetic result: a non-finite outcome (overflow,
/// NaN) is a hard T001 instead of a value that exports as null.
/// (LANG-B-901; the division-by-zero arms above name that cause
/// specifically.)
fn float_result(v: f64) -> Result<Value, String> {
    if v.is_finite() {
        Ok(Value::Float(canonical_zero(v)))
    } else if v.is_nan() {
        Err("float operation produced NaN".to_string())
    } else {
        Err("float overflow: the result is not representable".to_string())
    }
}

/// The same finiteness guard for quantity arithmetic: a magnitude that
/// overflows to infinity is a T001, never a quantity the exporter would
/// turn into `null` (LANG-B-901).
fn quantity_result(v: f64, unit: Unit) -> Result<Value, String> {
    if !v.is_finite() {
        return Err(if v.is_nan() {
            "float operation produced NaN".to_string()
        } else {
            "float overflow: the result is not representable".to_string()
        });
    }
    Ok(Value::Quantity(canonical_zero(v), unit))
}
/// The evaluated `envelope.payload` subtree, when the document has one.
/// The taint claim describes this payload, not the whole document
/// (LANG-F-004).
fn envelope_payload(value: &Value) -> Option<&Value> {
    match value {
        Value::Table(root) => match root.get("envelope") {
            Some(Value::Table(envelope)) => envelope.get("payload"),
            _ => None,
        },
        _ => None,
    }
}

fn value_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        // Structural equality (spec section 3.3): exact f64 comparison,
        // not an epsilon window, so two distinct values never compare
        // equal. NaN stays unequal to itself, as IEEE defines it.
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::DurationMs(x), Value::DurationMs(y)) => x == y,
        (Value::Quantity(x, ux), Value::Quantity(y, uy)) => x == y && ux == uy,
        _ => false,
    }
}

#[cfg(test)]
mod patch_transaction_tests {
    use super::*;

    fn value_table(pairs: &[(&str, Value)]) -> Value {
        Value::Table(
            pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.clone()))
                .collect::<indexmap::IndexMap<String, Value>>(),
        )
    }

    #[test]
    fn a_fatal_directive_leaves_the_tree_untouched() {
        // L-P41: the whole set stages on a clone. The first directive
        // applies there; the second (a metadata rewrite) is fatal, so the
        // caller's tree must be byte-identical to before.
        let module = crate::parse(
            "@patch(\"a\", 1)\n@patch(\"active_mode\", 1)\nb = 0\n",
            "patch-transaction",
        )
        .unwrap_or_else(|diags| panic!("parse: {diags:?}"));
        let engine = Engine::new();
        let mut value = value_table(&[("a", Value::Int(0)), ("b", Value::Int(0))]);
        let before = value.clone();
        let values = HashMap::new();
        let mut diagnostics = Vec::new();
        let mut account = Accountant {
            spent: 0,
            reported: 0,
            limit: 1_000_000,
            deadline: None,
            wall_limit: 0,
            bytes_since_sample: 0,
        };
        let error = engine
            .apply_patches(
                &module,
                &values,
                &mut value,
                &mut diagnostics,
                &mut account,
                test_ctx(),
            )
            .expect_err("a metadata rewrite is fatal");
        assert!(
            error.to_string().contains("metadata"),
            "unexpected error: {error}"
        );
        assert_eq!(
            crate::convert::value_to_pretty_json(&value),
            crate::convert::value_to_pretty_json(&before),
            "a failed set must leave the caller's tree byte-identical"
        );
    }

    #[test]
    fn a_successful_set_commits_as_one_move() {
        let module = crate::parse(
            "@patch(\"a\", 1)\n@patch(\"c\", 2)\nb = 0\n",
            "patch-commit",
        )
        .unwrap_or_else(|diags| panic!("parse: {diags:?}"));
        let engine = Engine::new();
        let mut value = value_table(&[("a", Value::Int(0)), ("b", Value::Int(0))]);
        let mut diagnostics = Vec::new();
        let mut account = Accountant {
            spent: 0,
            reported: 0,
            limit: 1_000_000,
            deadline: None,
            wall_limit: 0,
            bytes_since_sample: 0,
        };
        engine
            .apply_patches(
                &module,
                &HashMap::new(),
                &mut value,
                &mut diagnostics,
                &mut account,
                test_ctx(),
            )
            .expect("the set commits");
        let Value::Table(map) = &value else {
            panic!("root stays a table");
        };
        // Values are absent from the map, so the sets write Null at `a`
        // and create `c`; the point is the whole set committed.
        assert!(map.contains_key("c"), "the second directive applied");
        assert!(map.contains_key("a") && map.contains_key("b"));
    }
}

#[cfg(test)]
mod patch_charge_tests {
    use super::*;

    #[test]
    fn a_set_snapshot_is_charged() {
        // L-P41/L2: the per-Set rollback snapshot is a real clone; with a
        // budget just above the staging charge, the snapshot must trip it.
        let module = crate::parse("@patch(\"a\", 1)\na = 0\nb = 0\nc = 0\n", "patch-charge")
            .unwrap_or_else(|diags| panic!("parse: {diags:?}"));
        let engine = Engine::new();
        let mut value = Value::Table(
            [
                ("a".to_string(), Value::Int(0)),
                ("b".to_string(), Value::Int(0)),
                ("c".to_string(), Value::Int(0)),
            ]
            .into_iter()
            .collect::<indexmap::IndexMap<String, Value>>(),
        );
        let staging = measure_value(&value);
        let mut diagnostics = Vec::new();
        let mut account = Accountant {
            spent: 0,
            reported: 0,
            limit: staging + 1,
            deadline: None,
            wall_limit: 0,
            bytes_since_sample: 0,
        };
        let error = engine
            .apply_patches(
                &module,
                &HashMap::new(),
                &mut value,
                &mut diagnostics,
                &mut account,
                test_ctx(),
            )
            .expect_err("the snapshot charge must trip the total budget");
        assert_eq!(error.code(), "X006", "{error}");
    }
}

#[cfg(test)]
mod demand_eval_tests {
    use super::*;

    /// A document exercising the walk's value shapes: a `let` binding, a
    /// reference, an interpolation, a binary expression, nested tables,
    /// and an array. `Demand::All` must reproduce it byte-for-byte.
    const MIXED: &str = "\
let base = 5
name: \"cfg\"
keep:
  b:
    c: 1
  arr: [1, 2, 3]
  s: \"${name}-x\"
  n: base
  sum: 2 + 3
other:
  nested:
    deep: true
";

    /// The acceptance tests for L-P40 (design §8). The charge assertions
    /// are on `ChargeReport` and the X006 code, never on elapsed time.
    #[test]
    fn all_demand_is_byte_identical_to_evaluate() {
        // §8.6 (core side): the default demand is the whole document, so
        // the value AND the diagnostics match `evaluate` exactly.
        let module = crate::parse(MIXED, "mixed").expect("parse");
        let engine = Engine::hermetic();
        let (value, diagnostics) = engine.evaluate(&module).expect("evaluate");
        let outcome = engine
            .evaluate_demanded(&module, &Demand::All)
            .expect("demanded All");
        assert_eq!(
            crate::convert::value_to_pretty_json(&outcome.value),
            crate::convert::value_to_pretty_json(&value),
            "All must reproduce the whole tree byte-for-byte"
        );
        assert_eq!(
            format!("{:?}", outcome.diagnostics),
            format!("{:?}", diagnostics),
            "All must reproduce the diagnostics byte-for-byte"
        );
    }

    #[test]
    fn cone_projects_the_same_value_as_the_full_tree() {
        // §6.4/§8.4: for every demanded path, the cone's projection equals
        // the full run's projection. The cone omits undemanded siblings.
        let module = crate::parse(MIXED, "mixed").expect("parse");
        let engine = Engine::hermetic();
        let (full, _) = engine.evaluate(&module).expect("evaluate");
        for path in [
            "keep.b.c",
            "keep.arr",
            "keep.s",
            "keep.n",
            "keep.sum",
            "other.nested.deep",
        ] {
            let outcome = engine
                .evaluate_demanded(&module, &Demand::Cone(path.to_string()))
                .expect("cone");
            let cone = crate::convert::project_path(&outcome.value, path)
                .expect("the cone holds the demanded path");
            let whole = crate::convert::project_path(&full, path).expect("the full tree holds it");
            assert_eq!(
                crate::convert::value_to_pretty_json(cone),
                crate::convert::value_to_pretty_json(whole),
                "cone projection differs at {path}"
            );
        }
    }

    #[test]
    fn cone_spares_an_undemanded_amplifier_that_all_charges() {
        // §8.1/§8.3/§8.7. `loud.a` doubles a large env string: the static
        // DP pass bounds it at 3 units (env reads are opaque leaves), but
        // its measured growth is ~2x the env bytes. Under a tight
        // `max_total_value_size` the whole walk materialises it and trips
        // X006; a cone that never touches it stays cheap. This is the
        // permanent negative control: if the demand filter regressed, the
        // `All` branch (X006, amplifier charged) is what would move.
        let module = crate::parse(
            "keep: 7\nloud:\n  a: \"${env.BIG}${env.BIG}\"\n",
            "amplifier",
        )
        .expect("parse");
        let big = "x".repeat(4096);
        let env = std::collections::HashMap::from([("BIG".to_string(), big.clone())]);
        let mut engine = Engine::hermetic().with_env(env);
        engine.budget = Budget {
            max_total_value_size: 1024,
            ..Budget::unlimited()
        };

        let full = engine.evaluate_demanded(&module, &Demand::All);
        let error = full.expect_err("All must charge the amplifier and trip X006");
        assert_eq!(error.code(), "X006", "{error}");

        let outcome = engine
            .evaluate_demanded(&module, &Demand::Cone("keep".to_string()))
            .expect("the cone does not pay for the undemanded amplifier");
        assert!(
            outcome.charge.total < big.len() as u64,
            "the cone charged the amplifier: charge {} for {} env bytes",
            outcome.charge.total,
            big.len()
        );
        let keep =
            crate::convert::project_path(&outcome.value, "keep").expect("keep is in the cone");
        assert_eq!(crate::convert::value_to_pretty_json(keep), "7");
    }

    #[test]
    fn the_charge_report_sizes_a_secret_at_the_marker_not_its_length() {
        // C2: `ChargeReport.total` must not vary with a secret's byte
        // length. `secret.env` is the sharpest case — the secret node and
        // its `env` locator each materialise the bytes — and the aliases,
        // interpolations, and container copies around it must clamp the
        // same way. This is the negative control for the report-side
        // accountant: before it, a 32-byte value charged 84 and an 8-byte
        // one 36, so any regression moves these two totals apart.
        let module = crate::parse(
            "x = secret.env(\"ACTIVE_TEST_TOK\")\n\
             alias = x\n\
             pair: [x, x]\n\
             deep:\n  a:\n    b: x\n",
            "c2-report",
        )
        .expect("parse");
        let totals: Vec<u64> = ["abcdefgh", &"x".repeat(32)]
            .iter()
            .map(|value| {
                let env = HashMap::from([("ACTIVE_TEST_TOK".to_string(), value.to_string())]);
                let engine = Engine::hermetic().with_env(env);
                engine
                    .evaluate_demanded(&module, &Demand::All)
                    .expect("a marker-sized report stays inside the default budget")
                    .charge
                    .total
            })
            .collect();
        assert_eq!(
            totals[0], totals[1],
            "the charge total leaked the secret's length: {totals:?}"
        );
    }

    #[test]
    fn a_cone_cannot_hide_an_undemanded_static_shape_trip() {
        // §8.2: the static shape phase is whole-document, so an undemanded
        // array over `max_array_len` refuses the cone with the same code
        // and the same (path-independent) ordering as the full run.
        let module = crate::parse("keep: 1\nbig: [0, 0, 0, 0, 0, 0, 0, 0]\n", "static-shape")
            .expect("parse");
        let mut engine = Engine::hermetic();
        engine.budget = Budget {
            max_array_len: 4,
            ..Budget::unlimited()
        };
        let all = engine
            .evaluate_demanded(&module, &Demand::All)
            .expect_err("All trips the array bound");
        let cone = engine
            .evaluate_demanded(&module, &Demand::Cone("keep".to_string()))
            .expect_err("the cone cannot hide the array bound");
        assert_eq!(all.code(), cone.code(), "{all} vs {cone}");
    }

    #[test]
    fn a_cone_refuses_ambient_input_exactly_like_the_full_run() {
        // §8.5: purity is a whole-arena pre-scan, so a cone in a document
        // with an env read refuses E047 just as the full run does - it
        // cannot fetch around the gate by demanding a clean path.
        let module = crate::parse("keep: 1\nread: env.SECRET\n", "pure").expect("parse");
        let engine = Engine::hermetic().with_pure(true);
        let all = engine
            .evaluate_demanded(&module, &Demand::All)
            .expect_err("pure refuses the env read");
        let cone = engine
            .evaluate_demanded(&module, &Demand::Cone("keep".to_string()))
            .expect_err("the cone cannot fetch around the pure gate");
        assert_eq!(all.code(), "E047", "{all}");
        assert_eq!(cone.code(), "E047", "{cone}");
    }

    #[test]
    fn cone_runs_are_deterministic() {
        // §8.4: two runs of the same cone demand agree on value, charge,
        // and diagnostics.
        let module = crate::parse(MIXED, "mixed").expect("parse");
        let engine = Engine::hermetic();
        let first = engine
            .evaluate_demanded(&module, &Demand::Cone("keep.b.c".to_string()))
            .expect("cone run 1");
        let second = engine
            .evaluate_demanded(&module, &Demand::Cone("keep.b.c".to_string()))
            .expect("cone run 2");
        assert_eq!(
            crate::convert::value_to_pretty_json(&first.value),
            crate::convert::value_to_pretty_json(&second.value)
        );
        assert_eq!(first.charge, second.charge);
        assert_eq!(
            format!("{:?}", first.diagnostics),
            format!("{:?}", second.diagnostics)
        );
    }

    #[test]
    fn cone_diagnostics_are_the_full_run_subsequence() {
        // §8.4: W001 (unresolved identifier) fires per node during the
        // walk, so a demanded and an undemanded subtree can each carry
        // one. The cone reports only the demanded closure's warning, and
        // its whole record list is an ordered subsequence of the full
        // run's (the walk order is the same topological order filtered).
        let module = crate::parse(
            "keep:\n  u: undefined_keep\noutside:\n  u: undefined_outside\n",
            "diag-subset",
        )
        .expect("parse");
        let engine = Engine::hermetic();
        let full = engine
            .evaluate_demanded(&module, &Demand::All)
            .expect("all");
        let cone = engine
            .evaluate_demanded(&module, &Demand::Cone("keep".to_string()))
            .expect("cone");

        let records = |diags: &[Diagnostic]| -> Vec<(Option<String>, String)> {
            diags
                .iter()
                .map(|d| (d.path.clone(), d.code.clone()))
                .collect()
        };
        let full_records = records(&full.diagnostics);
        let cone_records = records(&cone.diagnostics);
        let w001 = |rs: &[(Option<String>, String)]| -> usize {
            rs.iter().filter(|(_, code)| code == "W001").count()
        };
        assert!(
            w001(&full_records) > w001(&cone_records),
            "the undemanded warning must be absent from the cone: full {full_records:?}, cone {cone_records:?}"
        );
        let mut remaining = full_records.iter();
        for record in &cone_records {
            assert!(
                remaining.any(|candidate| candidate == record),
                "cone record {record:?} is not an ordered subsequence of the full run"
            );
        }
    }

    #[test]
    fn a_cone_inside_the_envelope_reproduces_the_full_run() {
        // Review-C1 repro B, case 0034: a Data-mode document whose
        // envelope subtree holds `version` and `tainted` beside the
        // payload. When the cone walk stopped at the first envelope
        // member it dropped those structural fields, and the post-walk
        // gate then read a partial envelope and fabricated E033/E035 the
        // full run never emits. Value AND diagnostics must match.
        let module = crate::parse(
            "active_mode = \"data\"\n\
             other = secret(\"root-only-secret\")\n\
             envelope = { version = \"1.0.0\", tainted = false, payload = { ok = true, port = 8080 } }\n",
            "0034-cone",
        )
        .expect("parse");
        let engine = Engine::hermetic();
        let full = engine
            .evaluate_demanded(&module, &Demand::All)
            .expect("the full Data-mode run is clean");
        for path in ["envelope.payload", "envelope.version"] {
            let cone = engine
                .evaluate_demanded(&module, &Demand::Cone(path.to_string()))
                .expect("the cone inside the envelope is clean too");
            assert_eq!(
                crate::convert::value_to_pretty_json(
                    crate::convert::project_path(&cone.value, path).expect("path is in the cone")
                ),
                crate::convert::value_to_pretty_json(
                    crate::convert::project_path(&full.value, path)
                        .expect("path is in the full run")
                ),
                "the cone value at `{path}` must equal the full run's projection"
            );
            assert_eq!(
                format!("{:?}", cone.diagnostics),
                format!("{:?}", full.diagnostics),
                "the cone at `{path}` fabricated or dropped diagnostics"
            );
        }
    }

    #[test]
    fn a_cone_of_a_container_valued_patch_projects_the_full_run_spine() {
        // Review-C1 repro A: `@patch("a.b", a)` copies the whole table `a`
        // into `a.b`, and `a` is also the demanded path's spine ancestor.
        // The cone's `a.b` was truncated to the patch's own value because
        // `a`'s other fields were left unmaterialised. The projected cone
        // must equal the full run's projection.
        let module = crate::parse(
            "a:\n  x: 1\n  y: 2\n  b: 0\n@patch(\"a.b\", a)\nkeep: 5\n",
            "cone-patch-ancestor",
        )
        .expect("parse");
        let engine = Engine::hermetic();
        let full = engine
            .evaluate_demanded(&module, &Demand::All)
            .expect("the full run is clean");
        let cone = engine
            .evaluate_demanded(&module, &Demand::Cone("a.b".to_string()))
            .expect("the cone of the patched spine is clean");
        let cone_at = crate::convert::project_path(&cone.value, "a.b").expect("in the cone");
        let full_at = crate::convert::project_path(&full.value, "a.b").expect("in the full run");
        let cone_json = crate::convert::value_to_pretty_json(cone_at);
        assert_eq!(
            cone_json,
            crate::convert::value_to_pretty_json(full_at),
            "the cone's `a.b` was truncated against the full run"
        );
        assert!(
            cone_json.contains("\"x\""),
            "the patched container's other fields were dropped: {cone_json}"
        );
    }

    #[test]
    fn demanded_inspect_walk_matches_the_whole_document_run() {
        // §2.4: `inspect --path P` takes ONE demanded walk that still
        // carries the layer, merge, and seed provenance the report renders.
        // `Demand::All` must agree with the shipped whole-document method,
        // and a cone must project to the same subtree as the full run does.
        let module = crate::parse(MIXED, "mixed").expect("parse");
        let engine = Engine::hermetic();

        let (whole, whole_diags, _, _, _) = engine
            .evaluate_with_layers_merges_and_seeds(&module)
            .expect("whole");
        let (all, all_diags, _, _, _) = engine
            .evaluate_with_layers_merges_and_seeds_demanded(&module, &Demand::All)
            .expect("demanded All");
        assert_eq!(
            crate::convert::value_to_pretty_json(&all),
            crate::convert::value_to_pretty_json(&whole),
            "All must reproduce the whole tree"
        );
        assert_eq!(format!("{all_diags:?}"), format!("{whole_diags:?}"));

        let (cone, _, _, _, _) = engine
            .evaluate_with_layers_merges_and_seeds_demanded(
                &module,
                &Demand::Cone("keep.b.c".to_string()),
            )
            .expect("cone");
        let projected = crate::convert::project_path(&cone, "keep.b.c").expect("in the cone");
        let leaf = crate::convert::project_path(&whole, "keep.b.c").expect("in the whole");
        assert_eq!(
            crate::convert::value_to_pretty_json(projected),
            crate::convert::value_to_pretty_json(leaf),
            "the demanded inspect walk must project to the full value's leaf"
        );
    }

    #[test]
    fn cone_of_a_reference_to_a_named_table_projects_its_fields() {
        // Review-F1: the demanded path is a reference to a named table.
        // That table's rooted paths (`a.x`, `a.y`) lie outside the cone
        // `t`, so a member-only spine walk left its fields
        // unmaterialised and the projection came back empty. The
        // value-dependency walk must bring the whole referenced table in,
        // so the cone's projection of `t` equals the whole run's subtree.
        let module = crate::parse("a:\n  x: 1\n  y: 2\nt: a\n", "ref-table").expect("parse");
        let engine = Engine::hermetic();
        let (full, _) = engine.evaluate(&module).expect("evaluate");
        let outcome = engine
            .evaluate_demanded(&module, &Demand::Cone("t".to_string()))
            .expect("cone of t");
        let cone = crate::convert::project_path(&outcome.value, "t").expect("t is in the cone");
        let whole = crate::convert::project_path(&full, "t").expect("t is in the whole");
        let cone_json = crate::convert::value_to_pretty_json(cone);
        assert_eq!(
            cone_json,
            crate::convert::value_to_pretty_json(whole),
            "the cone of the reference must project the referenced table"
        );
        assert!(
            cone_json.contains("\"x\"") && cone_json.contains("\"y\""),
            "the referenced table's fields were dropped: {cone_json}"
        );
    }

    #[test]
    fn cone_does_not_fabricate_a_disabled_requires_trip() {
        // Review-F2: `cert` `requires` the sibling `tls`. Under the cone
        // `server.cert` that sibling lies outside the demand, so its
        // absence from the evaluated table is an artifact of the cone,
        // not a disabled trigger. The cone must return the value with no
        // T010 and match the plain run. The genuinely-disabled document
        // is the negative control: the plain run still trips T010, and
        // the cone omits it (a subsequence of the full run's records).
        let enabled = crate::parse(
            "schema = { server = Table { closed = true, tls = Bool, cert = String(requires = \"tls\") } }\nserver = { tls = true, cert = \"c\" }\n",
            "requires-cone",
        )
        .expect("parse");
        let engine = Engine::hermetic();
        let (full, full_diags) = engine.evaluate(&enabled).expect("the plain run is clean");
        assert!(
            !full_diags.iter().any(|d| d.code == "T010"),
            "the plain run must not report T010: {full_diags:?}"
        );
        let cone = engine
            .evaluate_demanded(&enabled, &Demand::Cone("server.cert".to_string()))
            .expect("the cone must not fabricate a T010");
        assert!(
            !cone.diagnostics.iter().any(|d| d.code == "T010"),
            "the cone fabricated a T010: {:?}",
            cone.diagnostics
        );
        let projected = crate::convert::project_path(&cone.value, "server.cert").expect("in cone");
        let whole = crate::convert::project_path(&full, "server.cert").expect("in full");
        assert_eq!(
            crate::convert::value_to_pretty_json(projected),
            crate::convert::value_to_pretty_json(whole)
        );

        let disabled = crate::parse(
            "schema = { server = Table { closed = true, tls = Bool, cert = String(requires = \"tls\") } }\nserver = { tls = false, cert = \"c\" }\n",
            "requires-cone-disabled",
        )
        .expect("parse");
        let (_, disabled_diags) = engine.evaluate(&disabled).expect("the plain run evaluates");
        assert!(
            disabled_diags.iter().any(|d| d.code == "T010"),
            "the plain run must trip T010: {disabled_diags:?}"
        );
        let cone = engine
            .evaluate_demanded(&disabled, &Demand::Cone("server.cert".to_string()))
            .expect("the disabled trigger is out of the cone's demand");
        assert!(
            !cone.diagnostics.iter().any(|d| d.code == "T010"),
            "the cone reported a T010 the cone's demand never evaluated: {:?}",
            cone.diagnostics
        );
    }

    #[test]
    fn cone_still_refuses_a_static_patch_overlap() {
        // Review-F3: patch targets are validated before the cone decides
        // whether to apply them. A patch onto an `ai_immutable` key is a
        // static refusal (§5, §8.2), so every cone refuses a document the
        // full run refuses — even when the frozen target lies outside the
        // cone. The negative control: the cone applies an in-cone patch.
        let immutable_out_of_cone = crate::parse(
            "# @ai_immutable: true\nkeep = 1\n# @ai_immutable: true\nother = 2\n@patch(\"other\", 9)\n",
            "patch-overlap",
        )
        .expect("parse");
        let engine = Engine::hermetic();
        let full = engine
            .evaluate(&immutable_out_of_cone)
            .expect_err("plain run");
        assert_eq!(full.code(), "E019", "{full:?}");
        let cone = engine
            .evaluate_demanded(&immutable_out_of_cone, &Demand::Cone("keep".to_string()))
            .expect_err("the cone must run the static patch refusal");
        assert_eq!(cone.code(), "E019", "{cone:?}");

        let immutable_in_cone = crate::parse(
            "# @ai_immutable: false\nkeep = 1\nother = 2\n@patch(\"keep\", 5)\n",
            "patch-in-cone",
        )
        .expect("parse");
        let cone = engine
            .evaluate_demanded(&immutable_in_cone, &Demand::Cone("keep".to_string()))
            .expect("an in-cone patch applies");
        let projected = crate::convert::project_path(&cone.value, "keep").expect("in cone");
        assert_eq!(crate::convert::value_to_pretty_json(projected), "5");
    }

    #[test]
    fn cone_runs_a_rename_named_by_source_or_destination() {
        // Review-M8: a rename writes its destination as well as its
        // source, so a cone is in scope of the directive when either path
        // lies in it. The pre-fix cone keyed only on `from`, so a demand
        // that named the destination skipped the directive: its source
        // never materialised, the move read as missing, and the cone
        // exited clean where the full run raised `E019`.
        let collision = crate::parse(
            "a:\n  x: 1\n  y: 2\n@rename(\"a.x\", \"y\")\n",
            "rename-collide",
        )
        .expect("parse");
        let engine = Engine::hermetic();
        let full = engine.evaluate(&collision).expect_err("plain run");
        assert_eq!(full.code(), "E019", "{full:?}");
        // The destination cone names `a.y`, the key the rename would
        // overwrite: it must carry the directive, and the source value
        // subtree with it, so the move collides exactly as the full run.
        let dest = engine
            .evaluate_demanded(&collision, &Demand::Cone("a.y".to_string()))
            .expect_err("the destination cone must run the rename");
        assert_eq!(dest.code(), "E019", "{dest:?}");
        // The source cone names `a.x`, so the directive is in scope too;
        // the colliding key is outside its demand, so the move applies.
        let source = engine
            .evaluate_demanded(&collision, &Demand::Cone("a.x".to_string()))
            .expect("the source cone applies the rename");
        let moved = crate::convert::project_path(&source.value, "a.y").expect("the moved key");
        assert_eq!(crate::convert::value_to_pretty_json(moved), "1");
    }

    #[test]
    fn cone_seed_excludes_an_undemanded_static_siblings_weight() {
        // Review-F5: the cone accountant is seeded from the closure-scoped
        // DP total, so an undemanded sibling's static weight never counts
        // against the cone. Here `big` is a 901-unit static literal the
        // cone never materialises, while the demanded `keep.dyn` reads a
        // 1200-byte env value. The whole document is over
        // `max_total_value_size` and trips X006, but the cone is not: its
        // seed is its own members' bounds (the spine containers plus the
        // env leaf), not the whole graph's. The pre-fix seed summed the
        // whole-graph bounds of the members — root included — so it
        // inherited `big`'s weight and tripped X006 on a cone that
        // materialises ~1.2 KB.
        let big = "x".repeat(900);
        let source = format!("big = \"{big}\"\nkeep:\n  dyn: env.AMP\n");
        let module = crate::parse(&source, "cone-seed").expect("parse");
        let amp = "y".repeat(1200);
        let env = std::collections::HashMap::from([("AMP".to_string(), amp.clone())]);
        let mut engine = Engine::hermetic().with_env(env);
        engine.budget = Budget {
            max_total_value_size: 2000,
            ..Budget::unlimited()
        };

        let full = engine.evaluate_demanded(&module, &Demand::All);
        let error = full.expect_err("the whole document materialises big + the env value");
        assert_eq!(error.code(), "X006", "{error}");

        let outcome = engine
            .evaluate_demanded(&module, &Demand::Cone("keep".to_string()))
            .expect("the cone does not inherit the undemanded sibling's weight");
        assert!(
            outcome.charge.total < 1500,
            "the cone seed inherited `big`: charge {}",
            outcome.charge.total
        );
        let dyn_value = crate::convert::project_path(&outcome.value, "keep.dyn").expect("in cone");
        assert_eq!(
            crate::convert::value_to_pretty_json(dyn_value),
            format!("\"{amp}\"")
        );
    }

    #[test]
    fn cone_seeds_the_same_secret_text_as_the_full_run() {
        // Round-2 review (§2.2.6): the demanded path `keep` copies the text
        // `secret(a)` computes from `env.A`, while `a` is undemanded. The
        // cone must seed that same text, so the demanded value redacts
        // exactly as its projection in the full run. Without the
        // secret-locator rule the cone walk never evaluates `a`, the
        // seed set misses `"xy"`, and `keep` prints in the clear.
        let module = crate::parse(
            "let a = \"x${env.A}\"\nloud = secret(a)\nkeep = \"xy\"\n",
            "cone-secret",
        )
        .expect("parse");
        let env = std::collections::HashMap::from([("A".to_string(), "y".to_string())]);
        let engine = Engine::hermetic().with_env(env);

        let redact_keep = |demand: &Demand| {
            let outcome = engine
                .evaluate_with_seeds_demanded(&module, demand)
                .expect("evaluate");
            let profile =
                crate::secret::SecretProfile::for_engine(&module, &engine, &outcome.seeds);
            crate::convert::redact_value(&profile, &outcome.value)
                .project("keep")
                .expect("keep is in the tree")
        };

        let cone = redact_keep(&Demand::Cone("keep".to_string()));
        let projected = redact_keep(&Demand::All);
        let cone_text = crate::convert::value_to_pretty_json_materialised(&cone);
        assert_eq!(
            cone_text,
            crate::convert::value_to_pretty_json_materialised(&projected),
            "the cone must redact exactly as the projection of the full run"
        );
        assert!(
            cone_text.contains("***SECRET***"),
            "demanded value not redacted: {cone_text}"
        );
        assert!(
            !cone_text.contains("xy"),
            "secret text leaked under the cone: {cone_text}"
        );
    }

    #[test]
    fn cone_reports_an_unknown_profile_selector() {
        // Review-F8: `active_profile` names a profile that does not exist,
        // so the plain run reports W002. With no profile bodies the old
        // cone skipped every root selector, so the walk never evaluated
        // `active_profile`, the merge saw no selector, and the cone lost
        // W002 (and the gate saw no profile). The document-level selector
        // keys are always demanded.
        let module = crate::parse("active_profile = \"missing\"\nkeep = 1\n", "profile-cone")
            .expect("parse");
        let engine = Engine::hermetic();
        let (_, full_diags) = engine.evaluate(&module).expect("the plain run evaluates");
        assert!(
            full_diags.iter().any(|d| d.code == "W002"),
            "the plain run must report W002: {full_diags:?}"
        );
        let cone = engine
            .evaluate_demanded(&module, &Demand::Cone("keep".to_string()))
            .expect("the cone evaluates");
        assert!(
            cone.diagnostics.iter().any(|d| d.code == "W002"),
            "the cone dropped the root selector and lost W002: {:?}",
            cone.diagnostics
        );
    }

    #[test]
    fn cone_fills_a_demanded_fields_schema_defaults() {
        // Review-C1: `validate_*` reads a field's constraint arguments from
        // the evaluated node map, and `apply_fills` reads the `default`
        // node there too. An in-cone field's default must therefore be a
        // closure member: without it the resilient fill could not read the
        // default, so the cone reported two spurious T004 and dropped the
        // filled values. The full run fills `s.b` and `s.t.c`.
        let module = crate::parse(
            "schema {\n  s = Table { a = Int, b = Int(default = 3), t = Table { c = Int(default = 4) } }\n}\ns = { a = 1, t = {} }\nnoise = 5\n",
            "cone-default-fill",
        )
        .expect("parse");
        let engine = Engine::hermetic();
        let (full, full_diags) = engine.evaluate(&module).expect("the full run evaluates");
        assert!(
            !full_diags.iter().any(|d| d.code == "T004"),
            "the full run fills both defaults and reports no T004: {full_diags:?}"
        );
        let cone = engine
            .evaluate_demanded(&module, &Demand::Cone("s".to_string()))
            .expect("the cone fills the demanded defaults");
        assert!(
            !cone.diagnostics.iter().any(|d| d.code == "T004"),
            "the cone reported a T004 the full run does not: {:?}",
            cone.diagnostics
        );
        let s = crate::convert::project_path(&cone.value, "s").expect("s is in the cone");
        let whole = crate::convert::project_path(&full, "s").expect("s is in the full tree");
        assert_eq!(
            crate::convert::value_to_pretty_json(s),
            crate::convert::value_to_pretty_json(whole),
            "the cone's filled subtree must equal the full run's"
        );
    }

    #[test]
    fn cone_materialises_an_in_cone_annotation_operand() {
        // Review-H2: an annotation predicate reads its operands through the
        // evaluated node map, and an `Ident` resolves through
        // `module.bindings`. An in-cone annotation whose operand is an
        // out-of-cone `let` binding must still see it, or the cone drops a
        // T006 the full run reports.
        let module = crate::parse(
            "let max = 10\n# @constraint: value > max\nport = 5\nother = 7\n",
            "cone-annotation",
        )
        .expect("parse");
        let engine = Engine::hermetic();
        let (_, full_diags) = engine.evaluate(&module).expect("the full run evaluates");
        assert!(
            full_diags.iter().any(|d| d.code == "T006"),
            "the full run must report the failing constraint: {full_diags:?}"
        );
        let cone = engine
            .evaluate_demanded(&module, &Demand::Cone("port".to_string()))
            .expect("the cone evaluates");
        assert!(
            cone.diagnostics.iter().any(|d| d.code == "T006"),
            "the cone dropped an in-cone T006: {:?}",
            cone.diagnostics
        );
    }

    #[test]
    fn cone_applies_an_in_cone_merge_of_a_nested_repeated_table() {
        // Review-H3: `PendingMerge` carries both the scope-relative
        // `local` and the full-document `path`. A nested repeated table
        // (`t` twice inside `s`) records `local = "t.a"` and
        // `path = "s.t.a"`. The cone must scope the merge by the document
        // path, or a merge that lands inside the cone is skipped: the cone
        // then loses the E046 the full run reports and, separately, must
        // still materialise the second body's candidate so the merge can
        // read it.
        let module = crate::parse(
            "s {\n  t {\n    a = 1\n  }\n  t {\n    a = 2\n  }\n}\nother = 5\n",
            "cone-nested-merge",
        )
        .expect("parse");
        let engine = Engine::hermetic();
        let (full, full_diags) = engine.evaluate(&module).expect("the full run evaluates");
        assert!(
            full_diags.iter().any(|d| d.code == "E046"
                && d.path.as_deref() == Some("s.t.a")
                && d.message.contains("disagree")),
            "the full run reports the nested merge conflict: {full_diags:?}"
        );
        let cone = engine
            .evaluate_demanded(&module, &Demand::Cone("s".to_string()))
            .expect("the cone applies the demanded merge");
        assert!(
            cone.diagnostics.iter().any(|d| d.code == "E046"
                && d.path.as_deref() == Some("s.t.a")
                && d.message.contains("disagree")),
            "the cone dropped the in-cone E046 (or read a missing candidate): {:?}",
            cone.diagnostics
        );
        let s = crate::convert::project_path(&cone.value, "s").expect("s is in the cone");
        let whole = crate::convert::project_path(&full, "s").expect("s is in the full tree");
        assert_eq!(
            crate::convert::value_to_pretty_json(s),
            crate::convert::value_to_pretty_json(whole),
            "the cone's merged subtree must equal the full run's"
        );
    }
}
