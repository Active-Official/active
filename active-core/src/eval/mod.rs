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
    /// finite — `0` (unlimited) is never a preset; embedders accept
    /// that risk field-by-field, never by profile name.
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

/// Cumulative materialization account threaded through post-walk
/// transforms (patch sets, profile merges, default fills). The DP
/// pass seeds it with the arena total; each transform charges what
/// it clones. Trips X006 past the total limit.
pub(crate) struct Accountant {
    spent: u64,
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
    pub(crate) fn charge(
        &mut self,
        module: &Module,
        node: Option<NodeId>,
        amount: u64,
    ) -> Result<(), EvalError> {
        self.spent = self.spent.saturating_add(amount);
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
    let Ok((dp_total, dp_bounds, dp_max_depth)) = check_dp_bounds(module, &order, budget) else {
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
pub(crate) struct BudgetCx<'a> {
    pub deadline: Option<Instant>,
    pub wall_limit: u64,
    pub account: &'a mut Accountant,
}

/// Iterative value-size measure in DP units (scalars 1, strings and
/// secrets plus their bytes, composites 1 plus children). Matches
/// `check_dp_bounds` semantics for transform-site accounting.
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

/// Reference-aware materialization bounds (spec/budgets.md §2,
/// C-037): one linear dynamic-programming pass over the topological
/// order (dependencies first, so every bound is final when read).
/// `size` bounds materialized data units (scalars plus string bytes;
/// each `Ref` clone costs its target's bound — this is what kills
/// exponential reference expansion before anything materializes).
/// `depth` bounds value nesting through references. Both saturate at
/// `limit + 1` so the numbers stay tiny; tripping either aborts with
/// the offending node's span. Memoized per node, so shared subtrees
/// cost O(1) — the pass itself is linear. Returns the cumulative
/// bound total, the per-node bounds table (drives delta
/// accounting for dynamic values during the walk), and the maximum
/// value depth bound (drives Data Mode progressive warnings).
fn check_dp_bounds(
    module: &Module,
    order: &[petgraph::graph::NodeIndex],
    budget: &Budget,
) -> Result<(u64, Vec<u64>, u64), EvalError> {
    if budget.max_value_size == 0 && budget.max_depth == 0 && budget.max_total_value_size == 0 {
        return Ok((0, vec![1u64; module.arena.len()], 0));
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
        if i >= size.len() {
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
                    if c < size.len() {
                        child_size = sat_size(child_size, size[c]);
                        child_depth = child_depth.max(depth[c]);
                    }
                }
                size[i] = sat_size(1, child_size);
                depth[i] = child_depth.saturating_add(1).min(depth_cap);
            }
            Some(Node::Array { elements }) => {
                for child in elements {
                    let c = child.0 as usize;
                    if c < size.len() {
                        child_size = sat_size(child_size, size[c]);
                        child_depth = child_depth.max(depth[c]);
                    }
                }
                size[i] = sat_size(1, child_size);
                depth[i] = child_depth.saturating_add(1).min(depth_cap);
            }
            Some(Node::Ref(target)) => {
                let c = target.0 as usize;
                if c < size.len() {
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
                            if c < size.len() {
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
                    if c < size.len() {
                        child_size = sat_size(child_size, size[c]);
                    }
                }
                size[i] = sat_size(1, child_size);
            }
            Some(Node::Unary { expr, .. }) => {
                let c = expr.0 as usize;
                if c < size.len() {
                    size[i] = sat_size(1, size[c]);
                }
            }
            // Transparent wrapper: the tag is tool treatment only, so
            // the bound is the inner value's bound exactly.
            Some(Node::Semantic { inner, .. }) => {
                let c = inner.0 as usize;
                if c < size.len() {
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
                    if c < size.len() {
                        bound = sat_size(bound, size[c]);
                        child_depth = child_depth.max(depth[c]);
                    }
                }
                size[i] = bound;
                depth[i] = child_depth.saturating_add(1).min(depth_cap);
            }
            _ => {}
        }
        // `>= cap` (not `> limit`): identical for ordinary limits,
        // and still correct at `u64::MAX`, where `limit + 1`
        // saturates and only a truly astronomical bound trips.
        if budget.max_value_size != 0 && size[i] >= size_cap {
            return Err(EvalError::budget_exceeded(
                BudgetKind::ValueSize,
                budget.max_value_size,
                module,
                Some(id),
            ));
        }
        if budget.max_depth != 0 && depth[i] >= depth_cap {
            return Err(EvalError::budget_exceeded(
                BudgetKind::Depth,
                budget.max_depth,
                module,
                Some(id),
            ));
        }
        total = total.saturating_add(size[i]);
        if budget.max_total_value_size != 0
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
    Ok((total, size, max_depth_seen))
}

type MemoConfig = (bool, Option<String>, Budget, bool, bool, Mode, bool);

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
                        *hash
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
            materialise_blobs: false,
            reader_version: None,
            taint_probe: false,
            blob_salt: None,
            envelope_gate: Some(crate::envelope::EnvelopeGate::shipped()),
        }
    }

    /// Fixed environment snapshot instead of the process ambient. Names
    /// fold to uppercase, exactly like the process snapshot (LANG-H-003).
    pub fn with_env(mut self, env: std::collections::HashMap<String, String>) -> Self {
        self.env = canonical_env_map(env.into_iter());
        self
    }

    /// Lazy host lookup consulted on snapshot miss (F-016).
    pub fn with_env_fallback(mut self, lookup: EnvLookup) -> Self {
        self.env_fallback = Some(lookup);
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
        ) {
            let mut seeds = SecretRedactor::new();
            return self.evaluate_inner(module, None, None, Some(&mut seeds));
        }
        self.evaluate_inner(module, None, None, None)
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
        // LANG-C-009: a budget trip replaces the diagnostic list with the
        // error, so re-derive the progressive warnings the walk had already
        // collected and carry them on the error. Only the abort path pays
        // for this (one extra static scan).
        self.evaluate_walk(module, memo, steps, seeds).map_err(|e| {
            // Resolve before deriving W013: an unresolved zero field would
            // read as "limit 0" and skip the warning the success path
            // emits (review finding on LANG-C-009).
            attach_progressive_warnings(module, &self.budget.resolve(), e)
        })
    }

    fn evaluate_walk(
        &self,
        module: &Module,
        mut memo: Option<&mut IncrementalMemo>,
        steps: Option<&mut Vec<StepEvent>>,
        mut seeds: Option<&mut SecretRedactor>,
    ) -> Result<(Value, Vec<Diagnostic>), EvalError> {
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
        // Budgets first: fail fast on monsters before any real work.
        check_static_budgets(module, &budget)?;
        // `petgraph::is_cyclic_directed` walks the graph recursively and
        // overflows the stack on a long chain (a crafted document with
        // ~950 aliases killed the process). `toposort` is iterative and
        // reports the same cycle, so use it directly.
        let order = toposort(&module.graph, None).map_err(|_| EvalError::CycleDetected)?;
        // Reference-aware materialization bounds (size and depth): the
        // billion-laughs killer. Runs on the topological order so every
        // bound is final when read; saturating arithmetic keeps it
        // linear even for exponential shapes. Returns the cumulative
        // bound plus the per-node bounds and the maximum depth bound,
        // seeding the transform accountant below.
        // The walk needs the cumulative bound and per-node bounds; the
        // depth bound and max node size only feed the progressive warnings
        // (recomputed by `progressive_warnings` on the abort path).
        let (dp_total, dp_bounds, _dp_max_depth) = check_dp_bounds(module, &order, &budget)?;
        let mut account = Accountant {
            spent: dp_total,
            limit: budget.max_total_value_size,
            deadline,
            wall_limit: budget.max_wall_ms,
            bytes_since_sample: 0,
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
        // Structural secret material: locator subtrees plus every node
        // derived from one (containers, interpolations, references).
        // Computed once when a stepper asked for a trace or the module
        // builds blobs (their structural taint must match the tooling
        // model). The same analysis backs `inspect` and hover, so all
        // surfaces agree on what is material.
        let has_blobs = module
            .arena
            .iter()
            .any(|node| matches!(node, Node::ContentAddressed { .. }));
        // A caller that asked for runtime seeds needs the analysis even
        // when the document has no `secret(...)` node: a profile can be
        // material by selection alone, and a fail-closed bound trip
        // marks every env read.
        let secret_profile: Option<SecretProfile> = (steps.is_some()
            || has_blobs
            || seeds.is_some())
        .then(|| SecretProfile::analyze_with_profile(module, self.profile_override.as_deref()));
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
                        diagnostics.extend(diags.iter().map(|d| {
                            let mut fresh = d.clone();
                            // Unconditional: a missing current
                            // position means "unknown", and serving
                            // a stale position as fact is worse
                            // than serving none.
                            fresh.span = span.clone();
                            fresh.file = file.clone();
                            fresh
                        }));
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
                            actuals.insert(node_id, 1u64.saturating_add(s.len() as u64));
                            account.charge(module, Some(node_id), s.len() as u64)?;
                            Value::String(s.clone())
                        }
                        None => {
                            // Lazy host lookup (F-016): same charging as a
                            // snapshot hit, so lazy values cost eager values.
                            // A consulted miss falls through to defaults, so a
                            // callback that knows nothing changes nothing.
                            if let Some(lookup) = self.env_fallback.clone() {
                                if let Some(s) = lookup(&key) {
                                    actuals.insert(node_id, 1u64.saturating_add(s.len() as u64));
                                    account.charge(module, Some(node_id), s.len() as u64)?;
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
                        account.charge(module, Some(node_id), s.len() as u64)?;
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
                    account.charge(module, Some(node_id), size)?;
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
                        account.charge(module, Some(node_id), actual.saturating_sub(bound))?;
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
                                        Value::Blob { hash, .. } => out.push_str(&format!(
                                            "$active.blob(blake3:{})",
                                            crate::blob::hash_hex(hash)
                                        )),
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
                    let els: Vec<Value> = elements
                        .iter()
                        .map(|e| values.get(e).cloned().unwrap_or(Value::Null))
                        .collect();
                    Value::Array(els)
                }

                Node::Table { fields } => {
                    let mut map = indexmap::IndexMap::new();
                    for (k, id) in fields {
                        map.insert(k.clone(), values.get(id).cloned().unwrap_or(Value::Null));
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
                    Some(Node::Table { fields }) => 1u64.saturating_add(
                        fields
                            .values()
                            .map(|e| actual_sizes.get(e.0 as usize).copied().unwrap_or(0))
                            .fold(0u64, u64::saturating_add),
                    ),
                    Some(Node::Ref(t)) => actual_sizes.get(t.0 as usize).copied().unwrap_or(0),
                    Some(Node::Interpolate { .. }) => match &value {
                        Value::String(text) => 1u64.saturating_add(text.len() as u64),
                        _ => measure_value(&value),
                    },
                    _ => measure_value(&value),
                };
                actual_sizes[node_id.0 as usize] = actual;
                if matches!(module.get(node_id), Some(Node::Interpolate { .. })) {
                    let bound = dp_bounds.get(node_id.0 as usize).copied().unwrap_or(1);
                    account.charge(module, Some(node_id), actual.saturating_sub(bound))?;
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
                        .and_then(|renderer| renderer.profile.redact_text(&path))
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
        let result = self.apply_profile(module, &values, result, &mut diagnostics, &mut account)?;
        let mut result = result;
        self.apply_patches(module, &values, &mut result, &mut diagnostics, &mut account)?;
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
        // (spec/budgets.md §2).
        self.validate_schema(
            module,
            &values,
            &mut result,
            &mut diagnostics,
            BudgetCx {
                deadline,
                wall_limit: budget.max_wall_ms,
                account: &mut account,
            },
        )?;
        self.validate_annotations(
            module,
            &values,
            &result,
            &mut diagnostics,
            deadline,
            budget.max_wall_ms,
        )?;
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
                    let actual = value_has_secret(payload)
                        || (has_secret || !module.profiles.is_empty()) && {
                            let profile = material_profile.get_or_insert_with(|| {
                                SecretProfile::with_context(
                                    module,
                                    self.profile_override.as_deref(),
                                    Some(&self.env),
                                    seeds.as_deref(),
                                )
                            });
                            // Both sources count: material content
                            // anywhere in the payload (text, a blob's
                            // media type or retained bytes), and a path
                            // the export redacts whatever kind sits there
                            // (a profile override inherits materiality,
                            // so a number can occupy a material path).
                            payload_has_material_content(payload, profile)
                                || payload_has_material_path(payload, profile, "envelope.payload")
                        };
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
                return Err(EvalError::ValidationFailed { diagnostics });
            }
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
                // T004 names structural schema paths (the missing
                // field and a resolvable requires target); those must
                // stay readable or the diagnostic cannot be acted on.
                if diagnostic.code == "T004" {
                    continue;
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
    ) -> Result<(), EvalError> {
        use crate::ir::PatchOp;

        for &patch_id in &module.patches {
            let (op, span, file) = match module.get(patch_id) {
                Some(Node::PatchDirective { op }) => (
                    op.clone(),
                    module.node_spans.get(&patch_id).cloned(),
                    module.node_files.get(&patch_id).cloned(),
                ),
                _ => continue,
            };
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
                PatchOp::Set { path, value } => {
                    if path_targets_document_metadata(&path) {
                        return Err(EvalError::patch_error(
                            format!("patch cannot rewrite document metadata `{path}`"),
                            module,
                            patch_id,
                        ));
                    }
                    Self::check_immutable(module, patch_id, &[&path])?;
                    let segs = validate_patch_path(&path).map_err(|why| {
                        EvalError::patch_error(
                            format!("invalid patch path `{path}`: {why}"),
                            module,
                            patch_id,
                        )
                    })?;
                    let v = values.get(&value).cloned().unwrap_or(Value::Null);
                    // Cloned values cost against the cumulative budget:
                    // many patches sharing one large value must trip
                    // like many aliases do. Path intermediates cost one
                    // unit per segment (input-proportioned tables).
                    account.charge(
                        module,
                        Some(patch_id),
                        measure_value(&v).saturating_add(segs.len() as u64),
                    )?;
                    // Failed sets roll back: navigation may have
                    // materialized intermediate tables before hitting a
                    // blocked hop, and a failed patch must leave no trace.
                    let snapshot = result.clone();
                    if !patch_set(result, &segs, v) {
                        *result = snapshot;
                        warn(
                            "W004",
                            format!("patch target `{path}` is blocked or out of bounds"),
                        );
                    } else if crate::layers::is_active() {
                        crate::layers::record(&path, "patch:set");
                    }
                }
                PatchOp::Remove { path } => {
                    if path_targets_document_metadata(&path) {
                        return Err(EvalError::patch_error(
                            format!("patch cannot rewrite document metadata `{path}`"),
                            module,
                            patch_id,
                        ));
                    }
                    Self::check_immutable(module, patch_id, &[&path])?;
                    let segs = validate_patch_path(&path).map_err(|why| {
                        EvalError::patch_error(
                            format!("invalid patch path `{path}`: {why}"),
                            module,
                            patch_id,
                        )
                    })?;
                    if !patch_remove(result, &segs) {
                        warn("W004", format!("remove target `{path}` does not exist"));
                    }
                }
                PatchOp::Rename { from, to } => {
                    if path_targets_document_metadata(&from) || path_targets_document_metadata(&to)
                    {
                        return Err(EvalError::patch_error(
                            format!("patch cannot rewrite document metadata `{from}` -> `{to}`"),
                            module,
                            patch_id,
                        ));
                    }
                    Self::check_immutable(module, patch_id, &[&from, &to])?;
                    let segs = validate_patch_path(&from).map_err(|why| {
                        EvalError::patch_error(
                            format!("invalid patch path `{from}`: {why}"),
                            module,
                            patch_id,
                        )
                    })?;
                    match patch_rename(result, &segs, &to) {
                        RenameOutcome::Ok => {}
                        RenameOutcome::Missing => {
                            warn("W004", format!("rename source `{from}` does not exist"));
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
        Ok(())
    }

    /// Select and merge the active profile (spec §3.5). The `--profile`
    /// override wins; otherwise the root value's `active_profile` field
    /// decides. Missing key → base only. Unknown name or non-string →
    /// W002. Type changes on override → W003. Every profile evaluates
    /// whether selected or not (strictness catches rot in inactive
    /// environments); selection reads the BASE key only.
    fn apply_profile(
        &self,
        module: &Module,
        values: &HashMap<NodeId, Value>,
        result: Value,
        diagnostics: &mut Vec<Diagnostic>,
        account: &mut Accountant,
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
        let base_table = match &result {
            Value::Table(map) => map.clone(),
            _ => return Ok(result),
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
        // rewrite the mode or capability declaration after the
        // handshake read them.
        if let Some(Node::Table { fields, .. }) = module.get(profile_id) {
            if let Some(key) = fields.keys().find(|key| is_document_metadata(key)) {
                let message = format!("profile cannot rewrite document metadata `{key}`");
                return Err(EvalError::ProfileError {
                    message,
                    span: module.node_spans.get(&profile_id).cloned(),
                    file: module.node_files.get(&profile_id).cloned(),
                });
            }
        }
        let profile_value = values.get(&profile_id).cloned().unwrap_or(Value::Null);
        // Merging copies the profile subtree into the result: charge
        // its measured size (over-approximates replacement as
        // addition — fail-safe direction).
        account.charge(module, Some(profile_id), measure_value(&profile_value))?;
        let mut merged = base_table;
        // A computed selector's value may be material: never echo it
        // into a diagnostic (W003 uses this label).
        let profile_label = profile_selector_label(module, &name, self.profile_override.is_some());
        merge_profile_value(
            &mut merged,
            profile_value,
            &profile_label,
            String::new(),
            module.node_spans.get(&profile_id).cloned(),
            module.node_files.get(&profile_id).cloned(),
            diagnostics,
        );
        Ok(Value::Table(merged))
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

/// Merge a profile table into the base table in place (spec §3.5):
/// tables recurse, scalars last-wins (profile wins). Additions are
/// silent; a profile value that overrides a base value of a different
/// kind wins with a W003 warning naming the dotted path. `span`/`file`
/// locate the profile block for the warning.
fn merge_profile_value(
    base: &mut indexmap::IndexMap<String, Value>,
    overlay: Value,
    profile: &str,
    path: String,
    span: Option<crate::diag::Span>,
    file: Option<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Value::Table(fields) = overlay else {
        return;
    };
    for (k, v) in fields {
        let child = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        match base.get(&k) {
            None => {
                if crate::layers::is_active() {
                    crate::layers::record(
                        &child,
                        &format!(
                            "profile:{}",
                            profile.trim_matches(|c| c == '\'' || c == '"')
                        ),
                    );
                }
                base.insert(k, v);
            }
            Some(existing) => match (existing, &v) {
                (Value::Table(_), Value::Table(_)) => {
                    if let Some(Value::Table(base_sub)) = base.get_mut(&k) {
                        merge_profile_value(
                            base_sub,
                            v,
                            profile,
                            child,
                            span.clone(),
                            file.clone(),
                            diagnostics,
                        );
                    }
                }
                _ => {
                    if crate::layers::is_active() {
                        crate::layers::record(
                            &child,
                            &format!(
                                "profile:{}",
                                profile.trim_matches(|c| c == '\'' || c == '"')
                            ),
                        );
                    }
                    if value_kind_name(existing) != value_kind_name(&v) {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            code: "W003".into(),
                            message: format!(
                                "profile {profile} overrides '{child}' with a different type"
                            ),
                            path: Some(child),
                            span: span.clone(),
                            repair: None,
                            confidence: 1.0,
                            file: file.clone(),
                        });
                    }
                    base.insert(k, v);
                }
            },
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
    // Temperature canonicalizes to kelvin through affine transforms;
    // below-absolute-zero literals clamp at 0 K. Decibels are retained
    // (log scale, no conversion). Both are ordering-only (spec Ch. 4 §4.1).
    match unit {
        Unit::Kelvin => return quantity_result(num.max(0.0), Unit::Kelvin),
        Unit::Celsius => return quantity_result((num + 273.15).max(0.0), Unit::Kelvin),
        Unit::Fahrenheit => {
            return quantity_result(((num - 32.0) * 5.0 / 9.0 + 273.15).max(0.0), Unit::Kelvin)
        }
        Unit::Decibel => return quantity_result(num, Unit::Decibel),
        _ => {}
    }
    quantity_result(num, unit.clone())
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
        // add/sub and ordering. Mismatched dimensions (e.g. DurationMs + Int)
        // fall through to the type error below.
        (Add, Value::DurationMs(a), Value::DurationMs(b)) => {
            Ok(Value::DurationMs(a.saturating_add(*b)))
        }
        (Sub, Value::DurationMs(a), Value::DurationMs(b)) => {
            Ok(Value::DurationMs(a.saturating_sub(*b)))
        }
        (Mul, Value::DurationMs(a), Value::Int(b)) => {
            Ok(Value::DurationMs(scale_dur(*a, *b as f64)))
        }
        (Mul, Value::Int(a), Value::DurationMs(b)) => {
            Ok(Value::DurationMs(scale_dur(*b, *a as f64)))
        }
        (Mul, Value::DurationMs(a), Value::Float(b)) => Ok(Value::DurationMs(scale_dur(*a, *b))),
        (Mul, Value::Float(a), Value::DurationMs(b)) => Ok(Value::DurationMs(scale_dur(*b, *a))),
        (Div, Value::DurationMs(a), Value::Int(b)) if *b != 0 => {
            Ok(Value::DurationMs(scale_dur(*a, 1.0 / *b as f64)))
        }
        (Div, Value::DurationMs(a), Value::Float(b)) if *b != 0.0 => {
            Ok(Value::DurationMs(scale_dur(*a, 1.0 / *b)))
        }
        (Div, Value::DurationMs(a), Value::DurationMs(b)) if *b != 0 => {
            // A ratio can exceed i64::MAX (both operands saturate at
            // u64::MAX); saturate the result rather than wrapping.
            Ok(Value::Int(i64::try_from(*a / *b).unwrap_or(i64::MAX)))
        }
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
        (Lt, Value::Quantity(a, ua), Value::Quantity(b, ub)) if ua == ub => Ok(Value::Bool(a < b)),
        (Le, Value::Quantity(a, ua), Value::Quantity(b, ub)) if ua == ub => Ok(Value::Bool(a <= b)),
        (Gt, Value::Quantity(a, ua), Value::Quantity(b, ub)) if ua == ub => Ok(Value::Bool(a > b)),
        (Ge, Value::Quantity(a, ua), Value::Quantity(b, ub)) if ua == ub => Ok(Value::Bool(a >= b)),

        (And, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(*a && *b)),
        (Or, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(*a || *b)),

        _ => Err(format!(
            // Kind names only: the operands may be material assembled
            // from a secret source, so the message must be value-free.
            "unsupported binary op {op:?} on {} and {}",
            value_kind_name(lhs),
            value_kind_name(rhs)
        )),
    }
}

/// Scale a duration in milliseconds by a factor, clamped at zero.
fn scale_dur(ms: u64, factor: f64) -> u64 {
    (ms as f64 * factor).round().max(0.0) as u64
}

pub(crate) fn eval_unary(op: UnOp, v: &Value) -> Result<Value, String> {
    match (op, v) {
        (UnOp::Neg, Value::Int(n)) => Ok(Value::Int(n.saturating_neg())),
        (UnOp::Neg, Value::Float(f)) => Ok(Value::Float(-f)),
        // Quantities negate by amount, keeping the unit: discounts, deltas
        // and signed rates are meaningful, and nothing here clamps
        // (spec Ch. 4 §4.1). Durations intentionally do NOT get this arm.
        (UnOp::Neg, Value::Quantity(a, u)) => Ok(Value::Quantity(-a, u.clone())),
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
        Ok(Value::Float(v))
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
    Ok(Value::Quantity(v, unit))
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
