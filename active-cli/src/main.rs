use active_core::convert::{
    collect_items_with_profile, doc_to_html, doc_to_man, doc_to_markdown, json_text_to_active,
    jsonschema_to_active, schema_inspect_with_profile, toml_text_to_active, value_to_pretty_json,
    value_to_pretty_json_materialised, value_to_toml_string, value_to_toml_string_materialised,
    value_to_yaml_string, value_to_yaml_string_materialised, yaml_text_to_active,
};
use active_core::diag::{Diagnostic, Severity};
use active_core::{parse_file, Budget, Engine, EvalError, ParseOptions, Style};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

mod test_runner;

/// Write to stdout without panicking on a closed reader.
///
/// The print macros panic on `EPIPE` from any thread except the main one
/// (rust-lang/rust#46016), so `active eval ... | head -1` turned a normal
/// end of pipeline into exit 101 plus a traceback. Every stdout write
/// goes through here instead: a broken pipe leaves with the conventional
/// filter status (128 + SIGPIPE = 141), and any other write failure is
/// reported on stderr and fails the run, so a full disk cannot pass for
/// success.
fn write_stdout(args: std::fmt::Arguments<'_>, newline: bool) {
    use std::io::Write as _;
    let mut stdout = std::io::stdout().lock();
    let result = if newline {
        writeln!(stdout, "{args}")
    } else {
        write!(stdout, "{args}")
    };
    if let Err(err) = result {
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(141);
        }
        let _ = writeln!(std::io::stderr(), "active: cannot write to stdout: {err}");
        std::process::exit(1);
    }
}

/// `println!` with a broken pipe treated as end of output.
macro_rules! outln {
    ($($arg:tt)*) => {
        crate::write_stdout(format_args!($($arg)*), true)
    };
}

/// `print!` with a broken pipe treated as end of output.
macro_rules! out {
    ($($arg:tt)*) => {
        crate::write_stdout(format_args!($($arg)*), false)
    };
}

#[derive(Parser)]
#[command(
    name = "active",
    version,
    about = "Active configuration language toolchain"
)]
struct Cli {
    /// Maximum document size the parser accepts, in bytes (default
    /// 4194304; LANG-C-003). Oversized input fails with E029 before it
    /// is read or lexed. `0` disables the cap.
    #[arg(long, global = true, value_name = "BYTES")]
    max_input_bytes: Option<usize>,
    /// Parse-phase wall-clock base budget in milliseconds (default 100;
    /// LANG-C-003). The effective deadline adds 30 ms per KiB of input, so
    /// the bound scales with the document; `0` disables it. Raise the base
    /// only for hosts slower than the reference measured in
    /// spec/budgets.md §1b.
    #[arg(long, global = true, value_name = "MS")]
    parse_wall_ms: Option<u64>,
    #[command(subcommand)]
    command: Commands,
}

/// Process-wide parse limits from the global flags (LANG-C-003): the
/// parser is constructed in several command paths, and the limits are a
/// property of the process invocation, so they are resolved once here.
static PARSE_LIMITS: std::sync::OnceLock<(usize, u64)> = std::sync::OnceLock::new();

/// Apply the process-wide parse limits (LANG-C-003) to a constructed
/// option set. Absent flags leave the library defaults intact.
pub(crate) fn with_parse_limits(mut options: ParseOptions) -> ParseOptions {
    if let Some((bytes, ms)) = PARSE_LIMITS.get() {
        options.max_input_bytes = *bytes;
        options.max_wall_ms = *ms;
    }
    options
}

/// Evaluator resource limits (spec/budgets.md, C-037). Omitted flags
/// use the shipped profile; an explicit `0` disables that limit (the
/// `eval::UNLIMITED` sentinel - a library `Budget` zero means "shipped
/// default" instead, LANG-C-015). Options (rather
/// than clap defaults) let the audit path reject limits it cannot
/// store and let reduction report what the user asked for.
#[derive(Args, Clone)]
struct BudgetArgs {
    /// Maximum IR nodes visited during evaluation (default 100000)
    #[arg(long)]
    max_nodes: Option<u64>,
    /// Maximum value nesting depth (default 256)
    #[arg(long)]
    max_depth: Option<u64>,
    /// Longest single array (default 100000)
    #[arg(long)]
    max_array_len: Option<u64>,
    /// Total table keys across the document (default 100000)
    #[arg(long)]
    max_keys: Option<u64>,
    /// Wall-clock milliseconds for the whole evaluation (default 100)
    #[arg(long)]
    max_wall_ms: Option<u64>,
    /// Maximum materialized value size (default 10000000)
    #[arg(long)]
    max_value_size: Option<u64>,
    /// Maximum cumulative materialized size (default 100000000)
    #[arg(long)]
    max_total_value_size: Option<u64>,
    /// Named budget profile: strict (CI/serverless), interactive
    /// (REPL/playground), or trusted (vendored/generated input).
    /// A profile replaces every per-field limit above.
    #[arg(long, value_enum)]
    budget_profile: Option<BudgetProfile>,
}

/// Reject limits the audit document cannot represent: it stores them
/// as Active integers (i64), so `active check` would reject a larger
/// value with E023.
fn validate_audit_limits(budget: &Budget) -> anyhow::Result<()> {
    for (flag, value) in [
        ("max-nodes", budget.max_nodes),
        ("max-depth", budget.max_depth),
        ("max-array-len", budget.max_array_len),
        ("max-keys", budget.max_keys),
        ("max-wall-ms", budget.max_wall_ms),
        ("max-value-size", budget.max_value_size),
        ("max-total-value-size", budget.max_total_value_size),
    ] {
        if value > i64::MAX as u64 {
            anyhow::bail!(
                "--{flag} {value} exceeds i64::MAX; the audit document stores limits as Active integers (use 0 for unlimited)"
            );
        }
    }
    Ok(())
}

/// Named budget profiles. Parsed by clap, so a typo fails fast with
/// a usage error instead of a runtime bail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum BudgetProfile {
    /// CI/serverless: identical to the compiled-in defaults.
    Strict,
    /// REPL/playground: generous interactive ceilings.
    Interactive,
    /// Vendored/generated input: minimal guardrails.
    Trusted,
}

impl BudgetArgs {
    /// True when the user passed an explicit per-field limit (a named
    /// profile alone is reported separately by `resolve`).
    fn has_explicit(&self) -> bool {
        self.max_nodes.is_some()
            || self.max_depth.is_some()
            || self.max_array_len.is_some()
            || self.max_keys.is_some()
            || self.max_wall_ms.is_some()
            || self.max_value_size.is_some()
            || self.max_total_value_size.is_some()
    }

    fn budget(&self) -> Budget {
        // One meaning across surfaces (LANG-C-015): an ABSENT flag uses the
        // shipped default, and an explicit `0` is the documented opt-out
        // (the UNLIMITED sentinel). The core resolves a zero field to the
        // shipped default, so an explicit 0 must become the sentinel here
        // rather than travelling as a zero.
        let default = Budget::default();
        let pick = |v: Option<u64>, d: u64| match v {
            None => d,
            Some(0) => active_core::eval::UNLIMITED,
            Some(n) => n,
        };
        Budget {
            max_nodes: pick(self.max_nodes, default.max_nodes),
            max_depth: pick(self.max_depth, default.max_depth),
            max_array_len: pick(self.max_array_len, default.max_array_len),
            max_keys: pick(self.max_keys, default.max_keys),
            max_wall_ms: pick(self.max_wall_ms, default.max_wall_ms),
            max_value_size: pick(self.max_value_size, default.max_value_size),
            max_total_value_size: pick(self.max_total_value_size, default.max_total_value_size),
        }
    }

    /// Named budget profile (`strict`, `interactive`, `trusted`)
    /// or explicit per-field limits. A profile wins entirely when
    /// set; per-field flags apply only without a profile. Passing
    /// both prints a warning so a stale `--max-*` flag cannot
    /// silently do nothing.
    fn resolve(&self) -> anyhow::Result<Budget> {
        let budget = match self.budget_profile {
            None => self.budget(),
            Some(BudgetProfile::Strict) => Budget::strict(),
            Some(BudgetProfile::Interactive) => Budget::interactive(),
            Some(BudgetProfile::Trusted) => Budget::trusted(),
        };
        if self.budget_profile.is_some() && self.has_explicit() {
            eprintln!("warning: --budget-profile overrides the explicit --max-* flags");
        }
        // The audit document stores limits as Active integers (i64);
        // larger limits are rejected at the audit call site.
        Ok(budget)
    }
}

/// Machine-readable output shape for agent-facing commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutFormat {
    /// Human-readable lines (default).
    Text,
    /// JSON array for programmatic consumers.
    Json,
}

/// Decidable graph queries (PROPOSAL §2.6, CHECKLIST GQ.001–GQ.004):
/// a closed set over the dependency DAG plus value-tree queries.
/// Outputs are paths and identity metadata only — values and secrets
/// never appear.
#[derive(Debug, Clone, Subcommand)]
enum GraphQuery {
    /// Run one named query.
    Query {
        /// Input .active file (defaults to the outer `graph` file)
        #[arg(long)]
        from: Option<PathBuf>,
        /// Query name: dependents-of, dependencies-of, shortest-path,
        /// secret-tainted-cone, content-addressed-above-size
        #[arg(long)]
        query: String,
        /// Dotted key for dependents-of / dependencies-of; "a,b" pair
        /// for shortest-path
        #[arg(long)]
        key: Option<String>,
        /// Minimum blob size in bytes (content-addressed-above-size)
        #[arg(long, default_value_t = 1)]
        min_size: u64,
        /// Output format (defaults to the outer `--format`)
        #[arg(long)]
        format: Option<String>,
        /// Select a profile by name, overriding the active_profile key
        /// (accepted here as well as on the outer `graph` command)
        #[arg(long)]
        profile: Option<String>,
    },
}

#[derive(Subcommand)]
enum StoreAction {
    /// List stored content digests
    List {
        /// Print a JSON array
        #[arg(long)]
        json: bool,
        /// Store root (default: $ACTIVE_STORE_DIR or ~/.cache/active/store)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Store a file's bytes and print their SHA-256
    Add {
        /// File to store
        file: PathBuf,
        /// Store root (default: $ACTIVE_STORE_DIR or ~/.cache/active/store)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Delete entries not referenced by the given lockfiles
    Gc {
        /// Lockfile whose sha256 values are GC roots (repeatable)
        #[arg(long = "root", required_unless_present = "all")]
        roots: Vec<PathBuf>,
        /// Delete every entry, including ones a lockfile still names
        #[arg(long, conflicts_with = "roots")]
        all: bool,
        /// Report what would be removed without deleting
        #[arg(long)]
        dry_run: bool,
        /// Store root (default: $ACTIVE_STORE_DIR or ~/.cache/active/store)
        #[arg(long)]
        store: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum Commands {
    /// Parse and evaluate a .active file, print as JSON
    Eval {
        /// Input .active file
        file: PathBuf,
        /// Output format
        #[arg(short, long, default_value = "json")]
        format: String,
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). A sibling `active-lock.toml`, when present, is applied
        /// by default (L-P03): a pinned include whose bytes changed is
        /// E041. With no lockfile, signature verification is unchanged.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Verify a lockfile's recorded file hashes before evaluating;
        /// fails closed when the main document changed (LANG-I-002c). Version 2
        /// locks also pin the include graph by identity (L-P03): a changed
        /// include is E041 and an include absent from the lock is E041.
        /// A version 1 lock checks recorded includes as drift and admits
        /// a new edge by content with W017.
        #[arg(long)]
        lockfile: Option<PathBuf>,
        /// Store every include's parsed bytes in this content-addressed
        /// directory and deduplicate them by SHA-256 (L-P04)
        #[arg(long)]
        store: Option<PathBuf>,
        /// Content-keyed evaluation cache directory (L-P29): a repeat run
        /// with identical source, includes, mode, profile, budget,
        /// environment, and output variant prints the stored result
        /// without re-evaluating. Ignored with --audit-trail, --envelope,
        /// --trace, --store, --trust-keys, --lockfile, and
        /// --envelope-trust.
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
        /// Emit opt-in evaluation trace (counts and diagnostic codes
        /// only, never values, sources, or secrets) as JSONL on stderr
        #[arg(long)]
        trace: bool,
        /// Materialise untainted blob payloads into the output
        /// (`payload_b64` alongside the identity; spec/blob.md §5).
        /// Tainted blobs never materialise. Payload bytes are charged
        /// against the materialised-size budgets.
        #[arg(long)]
        materialise_blobs: bool,
        /// Emit only the demanded cone of the evaluated value
        /// (PROPOSAL §2.2): the budget accountant still guards the
        /// whole graph; the path selects what gets printed. The cost is a
        /// FULL evaluation (parse + walk + validate), so a cone fetch is a
        /// whole-document fetch (LANG-C-013, spec/agent-interchange-profile.md
        /// section 4b).
        /// Dotted/index path (`server.host`, `ports[0]`).
        #[arg(long)]
        path: Option<String>,
        /// Sender identity recorded in the audit trail when this process
        /// received the document from a peer (a transport supplies it;
        /// omitted for local runs). LANG-F-010.
        #[arg(long)]
        peer: Option<String>,
        /// Write a secret-redacted audit document of this evaluation
        /// (PROPOSAL §5.1): counts, diagnostic codes, content hash —
        /// itself a valid Active document, never raw values.
        #[arg(long)]
        audit_trail: Option<PathBuf>,
        /// Write a v2 tool-result envelope (JSON) here, for success and
        /// for failed attempts (L-T06)
        #[arg(long)]
        envelope: Option<PathBuf>,
        /// Trusted keys for v2 sender signatures (E045 when a signed
        /// envelope's key is unknown)
        #[arg(long)]
        envelope_trust: Option<PathBuf>,
        /// Named sandbox profile (strict|data|agent|trusted): selects
        /// the profile's budget preset and refuses to evaluate
        /// documents declaring capabilities outside the profile
        #[arg(long)]
        sandbox: Option<String>,
        /// Declare this host's reader version (`major.minor.patch`).
        /// A document whose schema `min_reader` is above it is refused
        /// with T007; without the flag the floors stay advisory
        /// (spec/schema-evolution.md §1).
        #[arg(long)]
        reader_version: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Check a file for errors / cycles / unresolved names. With
    /// --trust-keys a sibling `active-lock.toml` is applied by default,
    /// matching `eval` (L-P03).
    Check {
        file: PathBuf,
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
        /// Emit opt-in evaluation trace (counts and diagnostic codes
        /// only, never values, sources, or secrets) as JSONL on stderr
        #[arg(long)]
        trace: bool,
        /// Declare this host's reader version (`major.minor.patch`).
        /// A document whose schema `min_reader` is above it is refused
        /// with T007; without the flag the floors stay advisory
        /// (spec/schema-evolution.md §1).
        #[arg(long)]
        reader_version: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Show dependency graph (text, Mermaid, DOT, or JSON).
    /// With a subcommand, run a decidable graph query instead
    /// (PROPOSAL §2.6): `active graph [FILE] query --query <name>`.
    Graph {
        /// Input .active file (also the default for `query --from`)
        file: Option<PathBuf>,
        /// Output format
        #[arg(long, default_value = "text")]
        format: String,
        #[command(subcommand)]
        command: Option<GraphQuery>,
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
    },
    /// List what breaks if a key's value changes (transitive dependents)
    Impact {
        /// Dotted key path (or `let` name)
        key: String,
        file: PathBuf,
        /// Output format
        #[arg(long, default_value = "text")]
        format: String,
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
    },
    /// List what a key's value is built from (transitive dependencies)
    Why {
        /// Dotted key path (or `let` name)
        key: String,
        file: PathBuf,
        /// Output format
        #[arg(long, default_value = "text")]
        format: String,
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
    },
    /// Convert evaluated result to JSON, YAML, or TOML
    Convert {
        file: PathBuf,
        #[arg(short, long, default_value = "json")]
        to: String,
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
        /// Declare this host's reader version (`major.minor.patch`).
        /// A document whose schema `min_reader` is above it is refused
        /// with T007; without the flag the floors stay advisory
        /// (spec/schema-evolution.md §1).
        #[arg(long)]
        reader_version: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Import JSON, YAML, or TOML as Active source (best-effort)
    Import {
        /// Input file
        file: PathBuf,
        /// Source format
        #[arg(short, long, default_value = "json")]
        from: String,
    },
    /// Generate typed bindings from an Active schema
    Generate {
        /// Input .active file (must contain a schema block)
        #[arg(short, long)]
        from: PathBuf,
        /// Output target: `jsonschema` (alias `json-schema`), `openapi`,
        /// `typescript` (alias `ts`), `go`, `rust` (alias `serde`), or
        /// `python` (alias `py`)
        #[arg(short, long)]
        to: String,
        /// Root type name
        #[arg(long, default_value = "Config")]
        name: String,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
    },
    /// Inspect a schema block or one value path as queryable JSON
    Inspect {
        /// Input .active file
        #[arg(short, long)]
        from: PathBuf,
        /// Dotted/index path to inspect (defaults to the schema view)
        #[arg(long)]
        path: Option<String>,
        /// Output format: json (default) or text (path mode)
        #[arg(long, default_value = "json")]
        format: String,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
    },
    /// Step, breakpoint, replay, or reduce a document (debugger)
    Debug {
        /// Input .active file
        #[arg(short, long)]
        from: PathBuf,
        /// Stop at the first matching breakpoint: taint, path:<prefix>,
        /// code:<CODE>, node:<n>, budget:<percent>, or budget:nodes:<n>
        #[arg(long = "break", conflicts_with_all = ["replay", "minimize"])]
        break_at: Option<String>,
        /// Deterministic replay: evaluate twice and compare hashes,
        /// codes, and step traces (plus a wall-bounded preflight)
        #[arg(long, conflicts_with = "minimize")]
        replay: bool,
        /// Expected content hash for --replay (sha256:...)
        #[arg(long, requires = "replay")]
        expect: Option<String>,
        /// Reduce the document while the given diagnostic code survives
        #[arg(long)]
        minimize: Option<String>,
        /// Maximum reduction attempts (requires --minimize)
        #[arg(long, requires = "minimize")]
        max_attempts: Option<usize>,
        /// Output format: json (default) or text
        #[arg(long, default_value = "json")]
        format: String,
        /// Profile override for stepping, replay, and reduction
        /// (audit reproducibility)
        #[arg(long)]
        profile: Option<String>,
        /// Resilient evaluation for stepping, replay, and reduction
        /// (match a resilient audited run; `eval` is strict by default)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to escape the directory (match an audit or
        /// reduction run that used --allow-parent-escapes)
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Resource limits for stepping, replay, and reduction; pass
        /// the same limits the audited run used to reproduce its hash
        /// Declare this host's reader version (`major.minor.patch`).
        /// A document whose schema `min_reader` is above it is refused
        /// with T007; without the flag the floors stay advisory
        /// (spec/schema-evolution.md §1).
        #[arg(long)]
        reader_version: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
    },
    /// Export visualizer data: graph, taint, budget, schema, or repair
    Viz {
        /// What to export: graph, taint, budget, schema, repair
        kind: String,
        /// Input .active file
        #[arg(short, long)]
        from: PathBuf,
        /// graph output: json, mermaid, or dot (other kinds: json)
        #[arg(long, default_value = "json")]
        format: String,
        /// Repair goal: validate, preserve-key, or minimize-diff
        #[arg(long, default_value = "validate")]
        goal: String,
        /// Path to preserve (repeatable, repair only)
        #[arg(long)]
        preserve: Vec<String>,
        /// Maximum repair plan steps
        #[arg(long, default_value_t = 8)]
        max_steps: usize,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
    },
    /// Explain every diagnostic in a file with stable repair guidance
    Explain {
        /// Input .active file
        #[arg(short, long)]
        from: PathBuf,
        /// Output format for machine consumers
        #[arg(long, value_enum, default_value_t = OutFormat::Text)]
        format: OutFormat,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Declare this host's reader version (`major.minor.patch`).
        /// A document whose schema `min_reader` is above it is refused
        /// with T007; without the flag the floors stay advisory
        /// (spec/schema-evolution.md §1).
        #[arg(long)]
        reader_version: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Suggest minimal @patch/@rename directives for mechanical diagnostics
    Patch {
        /// Input .active file
        #[arg(short, long)]
        from: PathBuf,
        /// Output format for machine consumers
        #[arg(long, value_enum, default_value_t = OutFormat::Text)]
        format: OutFormat,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Declare this host's reader version (`major.minor.patch`).
        /// A document whose schema `min_reader` is above it is refused
        /// with T007; without the flag the floors stay advisory
        /// (spec/schema-evolution.md §1).
        #[arg(long)]
        reader_version: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Print the agent capability descriptor; optionally check its major
    Capability {
        /// Fail unless the descriptor major version equals this
        #[arg(long)]
        require_major: Option<u64>,
        /// Verify a document's declared capabilities against the
        /// advertised tools (negotiation handshake); fail on refused
        #[arg(long)]
        check_doc: Option<PathBuf>,
        /// Print one sandbox profile (strict|data|agent|trusted)
        #[arg(long)]
        profile: Option<String>,
    },
    /// Compare two documents' schema blocks for compatibility
    /// (PROPOSAL §2.4): added/removed fields, type changes, tightened
    /// blob constraints, version regressions, raised reader floors.
    /// Exits nonzero when any breaking (error-severity) finding exists.
    Compatibility {
        /// Old (baseline) .active file
        old: PathBuf,
        /// New (candidate) .active file
        new: PathBuf,
        /// Output format
        #[arg(short, long, default_value = "text")]
        format: String,
    },
    /// Repair a document toward a goal (PROPOSAL §2.1): multi-step
    /// plan of safe @patch/@rename directives applied through the
    /// shipped patch engine, re-evaluated each step. Stops on clean,
    /// stuck (same codes twice, H006), or unrepairable (H008).
    /// Exits nonzero unless the result is clean.
    Repair {
        /// Input .active file
        #[arg(long)]
        from: PathBuf,
        /// Repair goal: validate, preserve-key, minimize-diff
        #[arg(long, default_value = "validate")]
        goal: String,
        /// Dotted paths the loop must never touch (repeatable)
        #[arg(long)]
        preserve: Vec<String>,
        /// Maximum applied directives (capped at 64)
        #[arg(long, default_value_t = 8)]
        max_steps: usize,
        /// Output format for the plan
        #[arg(short, long, default_value = "text")]
        format: String,
        /// Write the repaired source here (directives prepended;
        /// skipped with a warning unless the result is clean)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Apply only steps up to this zero-based index (partial apply)
        #[arg(long)]
        apply_step: Option<usize>,
        /// Apply only steps whose target path starts with this prefix
        #[arg(long)]
        cherry_pick: Option<String>,
        /// Emit the unified browser model (diagnostics + plan) as JSON
        #[arg(long)]
        browser: bool,
        /// Browser filter: keep diagnostics with this code (repeatable)
        #[arg(long = "filter-code", requires = "browser")]
        filter_code: Vec<String>,
        /// Browser filter: keep diagnostics with this severity
        /// (error|warning|info|hint, repeatable)
        #[arg(long = "filter-severity", requires = "browser")]
        filter_severity: Vec<String>,
        /// Browser filter: keep diagnostics at or under this path
        /// prefix (repeatable)
        #[arg(long = "filter-path", requires = "browser")]
        filter_path: Vec<String>,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
        /// Declare this host's reader version (`major.minor.patch`).
        /// A document whose schema `min_reader` is above it is refused
        /// with T007; without the flag the floors stay advisory
        /// (spec/schema-evolution.md §1).
        #[arg(long)]
        reader_version: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Render schema, intent, and comments as documentation
    Doc {
        /// Input .active file
        #[arg(short, long)]
        from: PathBuf,
        /// Output format
        #[arg(short, long, default_value = "markdown")]
        to: String,
        /// Document title (defaults to the file stem)
        #[arg(long)]
        title: Option<String>,
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Declare this host's reader version (`major.minor.patch`).
        /// A document whose schema `min_reader` is above it is refused
        /// with T007; without the flag the floors stay advisory
        /// (spec/schema-evolution.md §1).
        #[arg(long)]
        reader_version: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Emit a reproducible lockfile (content hash + provenance)
    Lock {
        /// Input .active file
        file: PathBuf,
        /// Write lockfile here instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Store every include's parsed bytes in this content-addressed
        /// directory and deduplicate them by SHA-256 (L-P04)
        #[arg(long)]
        store: Option<PathBuf>,
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
        /// Declare this host's reader version (`major.minor.patch`).
        /// A document whose schema `min_reader` is above it is refused
        /// with T007; without the flag the floors stay advisory
        /// (spec/schema-evolution.md §1).
        #[arg(long)]
        reader_version: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Manage the content-addressed include store (L-P04)
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },
    /// Run schema validation, lint, fmt-check, and conformance vectors in one pass
    ///
    /// Exit 0 only when every check passes. Warnings fail only with
    /// --strict. No external test framework.
    Test {
        /// Files or directories to check (default: the conformance manifest only)
        paths: Vec<PathBuf>,
        /// Conformance manifest (default: <dir>/conformance/manifest.toml or ./conformance/manifest.toml)
        #[arg(long)]
        conformance: Option<PathBuf>,
        /// Fail on warnings as well as errors
        #[arg(long)]
        strict: bool,
        /// Environment value for evaluation as KEY=VALUE (repeatable;
        /// the test runner is hermetic otherwise)
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
    },
    /// Generate an Ed25519 key pair for overlay signing
    Keygen {
        /// Output prefix: writes <prefix>.secret (0600 on Unix) and
        /// <prefix>.public (0644 on Unix). Windows has no POSIX modes:
        /// both files inherit the directory ACL, so generate keys in a
        /// directory only the key owner can write.
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Sign a file into a detached `<file>.sig` sidecar
    ///
    /// The signature binds the identity path (and optional expiry), so it
    /// verifies only for that path: use `--as` with the exact include
    /// spelling the document will use.
    Sign {
        /// File to sign
        file: PathBuf,
        /// Secret key file (64 hex chars)
        #[arg(long)]
        key: PathBuf,
        /// Key id recorded in the sidecar
        #[arg(long)]
        kid: String,
        /// Identity path recorded and signed (defaults to the file path)
        #[arg(long = "as")]
        identity: Option<String>,
        /// Expiry date (YYYY-MM-DD); the sidecar fails closed after it
        #[arg(long)]
        expires: Option<String>,
        /// Replace an existing `.sig` sidecar (refused by default, so a
        /// mistaken re-sign cannot destroy the only copy)
        #[arg(long)]
        force: bool,
    },
    /// Verify a file against its sidecar and a trusted-keys file
    ///
    /// Verification checks the signature, the path binding, expiry, and
    /// revocation; the file path must be spelled as it was signed.
    Verify {
        /// File to verify
        file: PathBuf,
        /// Trusted keys TOML
        #[arg(long)]
        trust_keys: PathBuf,
    },
    /// Format files in a canonical style (spec/formatter.md)
    ///
    /// Output is LF-only on every platform. CRLF input is accepted and
    /// normalized to LF, so `--check` reports a CRLF file as needing
    /// reformatting until it is formatted once.
    Fmt {
        /// Input .active files (at least one)
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Output style: brace, indent (default), or preserve
        #[arg(long, default_value = "indent")]
        style: String,
        /// Rewrite files in place
        #[arg(long)]
        write: bool,
        /// CI mode: report files needing formatting, write nothing
        #[arg(long)]
        check: bool,
    },
    /// Lint files with independent rules (spec/linter.md)
    Lint {
        /// Input .active files (at least one)
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Disable a rule (repeatable); rule IDs are listed by
        /// `docs/lint-rules.md`
        #[arg(long = "disable-lint")]
        disable: Vec<String>,
        /// Project configuration file (.active-lint.toml). When
        /// absent, the nearest configuration above each input file
        /// is discovered automatically.
        #[arg(long = "config")]
        config_file: Option<PathBuf>,
        /// Output format: text (human), json (stable schema), sarif 2.1.0
        #[arg(long = "format", default_value = "text")]
        output_format: String,
        /// Allow includes to use absolute paths or escape the directory
        /// (so include-hygiene can report them)
        #[arg(long)]
        allow_parent_escapes: bool,
    },
    /// Schema query alias surface: `active schema inspect ...`
    Schema {
        #[command(subcommand)]
        action: SchemaAction,
    },
    /// Report the document mode and declared capabilities
    Mode {
        /// Input .active file
        #[arg(short, long)]
        from: PathBuf,
        /// Output format: text (default) or json
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Explain the effective budget and observed usage
    Budget {
        #[command(subcommand)]
        action: BudgetAction,
    },
    /// Check the toolchain, this binary, and (in a source checkout) the
    /// repository state, then print a fix list (INNOVATIONS L-P48)
    Doctor {
        /// Source checkout to inspect (defaults to the current directory)
        #[arg(long)]
        path: Option<PathBuf>,
        /// Output format: text (default) or json
        #[arg(long, default_value = "text")]
        format: String,
    },
}

/// `active schema ...` subcommands (QUERY.002).
#[derive(Subcommand)]
enum SchemaAction {
    /// Inspect a schema block or one value path
    Inspect {
        /// Input .active file
        #[arg(short, long)]
        from: PathBuf,
        /// Dotted/index path to inspect (defaults to the schema view)
        #[arg(long)]
        path: Option<String>,
        /// Output format: json (default) or text (path mode)
        #[arg(long, default_value = "json")]
        format: String,
        /// Verify the root document and its includes against a
        /// trusted-keys file (E032; C-052 signed includes, LANG-C-007
        /// root). Before LANG-C-007 this covered includes only.
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
    },
}

/// `active budget ...` subcommands (QUERY.004).
#[derive(Subcommand)]
enum BudgetAction {
    /// Explain limits, observed usage, and headroom
    Explain {
        /// Input .active file
        #[arg(short, long)]
        from: PathBuf,
        /// Output format: text (default) or json
        #[arg(long, default_value = "text")]
        format: String,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
    },
}

/// Lint report formats (LINT.008).
#[derive(Clone, Copy, PartialEq, Eq)]
enum LintFormat {
    Text,
    Json,
    Sarif,
}

impl LintFormat {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "sarif" => Ok(Self::Sarif),
            other => anyhow::bail!("unknown lint format `{other}` (want text|json|sarif)"),
        }
    }
}

/// Shared implementation of `inspect` and `schema inspect` (QUERY.002).
/// One `active doctor` check (INNOVATIONS L-P48).
struct DoctorCheck {
    id: &'static str,
    /// `pass`, `warn`, `fail`, or `skip`.
    status: &'static str,
    detail: String,
}

impl DoctorCheck {
    fn new(id: &'static str, status: &'static str, detail: impl Into<String>) -> Self {
        Self {
            id,
            status,
            detail: detail.into(),
        }
    }
    fn pass(id: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, "pass", detail)
    }
    fn warn(id: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, "warn", detail)
    }
    fn fail(id: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, "fail", detail)
    }
    fn skip(id: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, "skip", detail)
    }
}

/// Names that may never appear in `active-core`'s dependency closure
/// (DEC-002; mirrors `research_battery::rb_066`).
const NETWORK_BANNED: [&str; 22] = [
    "reqwest",
    "hyper",
    "tokio",
    "mio",
    "socket2",
    "h2",
    "tungstenite",
    "ureq",
    "isahc",
    "curl",
    "curl-sys",
    "openssl",
    "rustls",
    "native-tls",
    "hyper-util",
    "axum",
    "actix-web",
    "actix-rt",
    "warp",
    "rouille",
    "tiny_http",
    "quinn",
];

/// Banned crates present in `active-core`'s lockfile closure (DEC-002:
/// the ban is scoped to the evaluator, dev-dependencies excluded).
fn banned_in_evaluator_closure(manifest_dir: &Path) -> Result<Vec<String>, String> {
    let lock = std::fs::read_to_string(manifest_dir.join("..").join("Cargo.lock"))
        .map_err(|e| format!("cannot read Cargo.lock: {e}"))?;
    let mut deps_of: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut current: Option<String> = None;
    let mut in_deps = false;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            current = None;
            in_deps = false;
        } else if let Some(rest) = line.strip_prefix("name = \"") {
            if let Some(name) = rest.strip_suffix('"') {
                current = Some(name.to_string());
                deps_of.entry(name.to_string()).or_default();
            }
        } else if line == "dependencies = [" {
            in_deps = true;
        } else if in_deps {
            if line == "]" {
                in_deps = false;
            } else {
                let dep = line.trim_matches(|c| c == '"' || c == ',');
                let dep_name = dep.split_whitespace().next().unwrap_or("").to_string();
                if !dep_name.is_empty() {
                    if let Some(pkg) = &current {
                        deps_of.entry(pkg.clone()).or_default().push(dep_name);
                    }
                }
            }
        }
    }
    let manifest = std::fs::read_to_string(manifest_dir.join("Cargo.toml"))
        .map_err(|e| format!("cannot read Cargo.toml: {e}"))?;
    let read_section = |wanted: &str| -> Vec<String> {
        let mut names = Vec::new();
        let mut on = false;
        for line in manifest.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                on = line == wanted;
            } else if on && !line.starts_with('#') {
                if let Some((name, _)) = line.split_once('=') {
                    names.push(name.trim().trim_matches('"').to_string());
                }
            }
        }
        names
    };
    let dev = read_section("[dev-dependencies]");
    let normal = read_section("[dependencies]");
    let dev_only: Vec<String> = dev.into_iter().filter(|n| !normal.contains(n)).collect();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: Vec<String> = vec!["active-core".to_string()];
    seen.insert("active-core".to_string());
    let mut first_hop = true;
    while let Some(pkg) = queue.pop() {
        if let Some(deps) = deps_of.get(&pkg) {
            for dep in deps {
                if first_hop && dev_only.contains(dep) {
                    continue;
                }
                if seen.insert(dep.clone()) {
                    queue.push(dep.clone());
                }
            }
        }
        first_hop = false;
    }
    Ok(seen
        .into_iter()
        .filter(|n| NETWORK_BANNED.contains(&n.as_str()))
        .collect())
}

/// `active doctor`: check the binary, the language invariants that must
/// hold on any host, and (in a source checkout) the repository state.
/// Prints a report; exits non-zero when any check fails.
///
/// `explicit_path` is true when the caller named `--path`: asking for a
/// specific checkout makes its required files mandatory, so a missing or
/// unreadable manifest, lockfile, or committed FFI header is a failure
/// rather than a skip.
fn run_doctor(root: &Path, explicit_path: bool, format: &str) -> anyhow::Result<()> {
    let mut checks: Vec<DoctorCheck> = Vec::new();

    checks.push(DoctorCheck::pass(
        "version",
        format!(
            "active {} ({} {})",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    ));

    // The binary evaluates a known document to the known value.
    let self_eval = active_core::parse("a = 1\nb = a + 1\n", "doctor");
    match self_eval {
        Ok(module) => {
            let engine = Engine::new();
            match engine.evaluate(&module) {
                Ok((value, _)) => {
                    let json = active_core::convert::value_to_json(&value);
                    if json == serde_json::json!({"a": 1, "b": 2}) {
                        checks.push(DoctorCheck::pass(
                            "self-eval",
                            "a = 1, b = a + 1 -> {a:1,b:2}",
                        ));
                    } else {
                        checks.push(DoctorCheck::fail(
                            "self-eval",
                            format!("unexpected evaluation result: {json}"),
                        ));
                    }
                }
                Err(e) => checks.push(DoctorCheck::fail(
                    "self-eval",
                    format!("evaluation failed: {e}"),
                )),
            }
        }
        Err(d) => checks.push(DoctorCheck::fail(
            "self-eval",
            format!("the shipped parser rejected a valid document: {d:?}"),
        )),
    }

    // The refusal contract: a value that violates its schema must produce
    // an error-severity diagnostic, never a silent success.
    match active_core::parse(
        "schema = {\n  port = Int\n}\nport = \"not an int\"\n",
        "doctor-refusal",
    ) {
        Ok(module) => {
            let engine = Engine::new();
            let refused = match engine.evaluate(&module) {
                Ok((_, diags)) => diags
                    .iter()
                    .any(|d| d.severity == active_core::Severity::Error),
                Err(_) => true,
            };
            if refused {
                checks.push(DoctorCheck::pass(
                    "refusal",
                    "a schema violation reports an error-severity diagnostic",
                ));
            } else {
                checks.push(DoctorCheck::fail(
                    "refusal",
                    "a schema violation produced no error-severity diagnostic",
                ));
            }
        }
        Err(d) => checks.push(DoctorCheck::fail("refusal", format!("parse failed: {d:?}"))),
    }

    // Budgets fail closed: one node under a one-node cap is refused.
    match active_core::parse("a = 1\nb = 2\nc = 3\n", "doctor-budget") {
        Ok(module) => {
            let mut engine = Engine::new();
            engine.budget.max_nodes = 1;
            match engine.evaluate(&module) {
                Err(EvalError::BudgetExceeded { .. }) => checks.push(DoctorCheck::pass(
                    "budgets",
                    "a one-node cap refuses a three-node document",
                )),
                other => checks.push(DoctorCheck::fail(
                    "budgets",
                    format!("a one-node cap did not refuse the document: {other:?}"),
                )),
            }
        }
        Err(d) => checks.push(DoctorCheck::fail("budgets", format!("parse failed: {d:?}"))),
    }

    if Budget::wall_supported() {
        checks.push(DoctorCheck::pass(
            "clock",
            "wall-clock budgets are supported on this target",
        ));
    } else {
        checks.push(DoctorCheck::warn(
            "clock",
            "wall-clock budgets are unavailable on this target; static caps still apply",
        ));
    }

    // Source-checkout checks. A named --path makes them mandatory; the
    // default (cwd) only checks when it looks like a checkout, so an
    // installed binary run from anywhere still reports a useful.
    let manifest = root.join("Cargo.toml");
    let lock = root.join("Cargo.lock");
    let is_checkout = manifest.is_file() && lock.is_file();
    let strict = explicit_path || is_checkout;
    if !is_checkout {
        if strict {
            checks.push(DoctorCheck::fail(
                "source-tree",
                format!(
                    "{} is not a source checkout (Cargo.toml and Cargo.lock both required)",
                    root.display()
                ),
            ));
        } else {
            checks.push(DoctorCheck::skip(
                "source-tree",
                "not a source checkout (pass --path to a repository root to inspect one)",
            ));
        }
    } else {
        let text = std::fs::read_to_string(&manifest).unwrap_or_default();
        let mut on = false;
        let mut workspace_version = None;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                on = line == "[workspace.package]";
            } else if on {
                if let Some((key, value)) = line.split_once('=') {
                    if key.trim() == "version" {
                        workspace_version = Some(value.trim().trim_matches('"').to_string());
                    }
                }
            }
        }
        match workspace_version {
            Some(version) if version == env!("CARGO_PKG_VERSION") => {
                checks.push(DoctorCheck::pass(
                    "version-source",
                    format!("CLI and workspace agree on {version}"),
                ))
            }
            Some(version) => checks.push(DoctorCheck::fail(
                "version-source",
                format!(
                    "CLI {} but workspace package version {version}",
                    env!("CARGO_PKG_VERSION")
                ),
            )),
            None => checks.push(DoctorCheck::fail(
                "version-source",
                "no [workspace.package] version found",
            )),
        }

        match banned_in_evaluator_closure(&root.join("active-core")) {
            Ok(hits) if hits.is_empty() => checks.push(DoctorCheck::pass(
                "network-closure",
                "no network-capable crate in active-core's dependency closure",
            )),
            Ok(mut hits) => {
                // Deterministic report: HashSet order varies per run.
                hits.sort();
                checks.push(DoctorCheck::fail(
                    "network-closure",
                    format!("network-capable crates in the evaluator's closure: {hits:?}"),
                ))
            }
            Err(e) => checks.push(DoctorCheck::fail("network-closure", e)),
        }

        let header = root.join("active-ffi").join("active.h");
        if header.is_file() {
            checks.push(DoctorCheck::pass(
                "ffi-header",
                "active-ffi/active.h is present; drift gate: cargo test -p active-ffi --test header_drift",
            ));
        } else {
            // The header is committed: a checkout without it is broken.
            checks.push(DoctorCheck::fail(
                "ffi-header",
                "active-ffi/active.h is missing",
            ));
        }

        let wasm = ["pkg-web", "pkg-nodejs"].iter().any(|dir| {
            root.join("active-wasm")
                .join(dir)
                .join("active_wasm_bg.wasm")
                .is_file()
        });
        if wasm {
            checks.push(DoctorCheck::pass(
                "wasm-artifact",
                "a built WASM artifact is present under active-wasm/pkg-*",
            ));
        } else {
            checks.push(DoctorCheck::warn(
                "wasm-artifact",
                "no built WASM artifact (engine.wasm build required; LANG-G-023)",
            ));
        }
    }

    let fails = checks.iter().filter(|c| c.status == "fail").count();
    match format {
        "json" => {
            let items: Vec<serde_json::Value> = checks
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "status": c.status,
                        "detail": c.detail,
                    })
                })
                .collect();
            outln!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "ok": fails == 0,
                    "checks": items,
                }))?
            );
        }
        "text" => {
            for c in &checks {
                outln!("{:<5}{:<18}{}", c.status, c.id, c.detail);
            }
        }
        other => anyhow::bail!("unknown doctor format `{other}` (want text|json)"),
    }
    if fails > 0 {
        anyhow::bail!("doctor found {fails} failing check(s)");
    }
    Ok(())
}

fn run_inspect(
    from: &Path,
    path: Option<&str>,
    format: &str,
    trust_keys: &Option<PathBuf>,
    profile: &Option<String>,
) -> anyhow::Result<()> {
    let trust = load_trust(trust_keys)?;
    let module = parse_input(from, false, false, &trust)?;
    match path {
        None => {
            if format != "json" {
                anyhow::bail!("schema inspection supports --format json only");
            }
            let mut inspected = schema_inspect_with_profile(&module, profile.as_deref())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            // Schema prose can equal a host value; redact it with the
            // authored + process-env material set.
            let schema_profile = material_profile_with(&module, profile.as_deref())?;
            schema_profile.redact_json(&mut inspected);
            outln!(
                "{}",
                serde_json::to_string_pretty(&inspected).map_err(|e| anyhow::anyhow!("{e}"))?
            );
        }
        Some(path) => {
            let mut evaluation_engine = Engine::new();
            evaluation_engine.profile_override = profile.clone();
            // Retain payloads so a payload-resident secret is visible to
            // the taint check; the report strips payloads before output.
            evaluation_engine.materialise_blobs = true;
            evaluation_engine.taint_probe = true;
            let (value, _, seeds) = evaluation_engine
                .evaluate_with_seeds(&module)
                .map_err(|e| anyhow::anyhow!("cannot evaluate for inspection: {e}"))?;
            // LANG-F-014: report which layer wrote this value last. The
            // provenance run is a second evaluation (the first already
            // produced the tree for the redaction context); it is the same
            // evaluation plus bookkeeping, so results cannot differ.
            let layers = evaluation_engine
                .evaluate_with_layers(&module)
                .map(|(_, _, layers)| layers)
                .map_err(|e| anyhow::anyhow!("cannot evaluate for inspection: {e}"))?;
            let report = active_core::convert::inspect_path_with_layers(
                &module,
                &value,
                path,
                profile.as_deref(),
                Some(&evaluation_engine.env),
                Some(&seeds),
                &layers,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            match format {
                "json" => outln!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|e| anyhow::anyhow!("{e}"))?
                ),
                "text" => out!("{}", active_core::convert::render_inspection_text(&report)),
                other => anyhow::bail!("unknown inspect format `{other}` (want json|text)"),
            }
        }
    }
    Ok(())
}

fn options(resilient: bool, allow_parent_escapes: bool) -> ParseOptions {
    with_parse_limits(ParseOptions {
        resilient,
        allow_parent_escapes,
        ..ParseOptions::default()
    })
}

/// Parse options carrying an include lock (L-P03): include resolution
/// verifies each file against the pinned content hash.
fn options_with_lock(
    resilient: bool,
    allow_parent_escapes: bool,
    lock: Option<active_core::provenance::IncludeLock>,
    store: Option<PathBuf>,
) -> ParseOptions {
    let mut options = options(resilient, allow_parent_escapes);
    options.include_lock = lock;
    options.include_store = store;
    options
}

/// Load a trusted-keys file for overlay enforcement (C-052), or
/// nothing when the flag is absent (legacy behavior preserved).
fn load_trust(path: &Option<PathBuf>) -> anyhow::Result<Option<active_core::sign::TrustedKeys>> {
    match path {
        None => Ok(None),
        Some(file) => {
            let text = std::fs::read_to_string(file)
                .map_err(|e| anyhow::anyhow!("cannot read trust file {}: {e}", file.display()))?;
            active_core::sign::load_trusted_keys(&text)
                .map(Some)
                .map_err(|e| anyhow::anyhow!("{e}"))
        }
    }
}

/// Largest lockfile the CLI will read. A lockfile is a small provenance
/// document; an attacker-supplied sibling under `--trust-keys` must not
/// turn evaluation into an unbounded read.
const MAX_LOCKFILE_BYTES: u64 = 1 << 20;

/// Read and size-check one lockfile, then verify its recorded file
/// hashes (LANG-I-002c) before it can influence evaluation.
fn read_lockfile(
    path: &std::path::Path,
    expected_main: &std::path::Path,
    restrict_root: Option<&std::path::Path>,
) -> anyhow::Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("cannot read lockfile {}: {e}", path.display()))?;
    let mut text = String::new();
    // Read through a hard cap so a lockfile swapped after open cannot
    // outgrow the bound between a stat and the read.
    file.take(MAX_LOCKFILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| anyhow::anyhow!("cannot read lockfile {}: {e}", path.display()))?;
    if text.len() as u64 > MAX_LOCKFILE_BYTES {
        return Err(anyhow::anyhow!(
            "lockfile {} exceeds the {} byte limit",
            path.display(),
            MAX_LOCKFILE_BYTES
        ));
    }
    // Relative recorded paths resolve against the lockfile's directory,
    // and a lock naming another main document is refused (L-P03).
    active_core::provenance::verify_lockfile_files_scoped(
        &text,
        path.parent(),
        Some(expected_main),
        restrict_root,
    )
    .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(text)
}

/// Read an explicit lockfile and pick its resolution mode (L-P03). A
/// lockfile with spelling pins enforces them; a v1 lockfile written
/// before those existed resolves legacy content hashes in warn mode,
/// because it cannot name the includes it did not see. `parse` derives
/// the mode from the pins it found.
fn explicit_include_lock(
    path: &std::path::Path,
    expected_main: &std::path::Path,
    restrict_root: Option<&std::path::Path>,
) -> anyhow::Result<active_core::provenance::IncludeLock> {
    let text = read_lockfile(path, expected_main, restrict_root)?;
    active_core::provenance::IncludeLock::parse(&text).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Discover the sibling `active-lock.toml` used by trusted evaluation
/// (L-P03). Trust defaults to the lock when one exists; with none, the
/// established signature-only behavior is unchanged.
fn sibling_include_lock(
    file: &std::path::Path,
) -> anyhow::Result<Option<active_core::provenance::IncludeLock>> {
    let sibling = file.with_file_name("active-lock.toml");
    if !sibling.is_file() {
        return Ok(None);
    }
    Ok(Some(explicit_include_lock(&sibling, file, file.parent())?))
}

/// Parse a file, enforcing overlay signatures when trust is set, and
/// resolving includes against the sibling lockfile when one exists
/// (L-P03) so every trusted surface agrees with `eval`.
fn parse_input(
    file: &std::path::Path,
    resilient: bool,
    allow_parent_escapes: bool,
    trust: &Option<active_core::sign::TrustedKeys>,
) -> anyhow::Result<active_core::Module> {
    let name = file.to_string_lossy().to_string();
    let lock = match trust {
        Some(_) => sibling_include_lock(file)?,
        None => None,
    };
    let options = options_with_lock(resilient, allow_parent_escapes, lock, None);
    let result = match trust {
        Some(keys) => active_core::parse_file_with_trust(file, options, keys),
        None => parse_file(file, options),
    };
    result.map_err(|diags| {
        for d in &diags {
            report(d, &name);
        }
        anyhow::anyhow!("parse failed")
    })
}

/// Parse with signature enforcement but WITHOUT lockfile pins. Maintenance
/// commands (`lock`, `repair`) must be able to read a stale tree to
/// refresh or repair it; inline `sha256:` pins still apply because they
/// live in the document. Pin enforcement for lockfiles belongs to
/// evaluation surfaces.
fn parse_input_unpinned(
    file: &std::path::Path,
    resilient: bool,
    allow_parent_escapes: bool,
    trust: &Option<active_core::sign::TrustedKeys>,
    store: Option<PathBuf>,
) -> anyhow::Result<active_core::Module> {
    let name = file.to_string_lossy().to_string();
    let mut options = options(resilient, allow_parent_escapes);
    options.include_store = store;
    let result = match trust {
        Some(keys) => active_core::parse_file_with_trust(file, options, keys),
        None => parse_file(file, options),
    };
    result.map_err(|diags| {
        for d in &diags {
            report(d, &name);
        }
        anyhow::anyhow!("parse failed")
    })
}

/// Parse for the eval path, keeping the diagnostics on failure so a
/// requested audit can record them before the error propagates
/// (LANG-F-010). Read surfaces keep `parse_input` (pins enforced) or
/// `parse_input_unpinned` (maintenance only); both report and discard
/// the diagnostics. Carries an include lock (L-P03).
fn parse_input_or_diags_locked(
    file: &std::path::Path,
    resilient: bool,
    allow_parent_escapes: bool,
    trust: &Option<active_core::sign::TrustedKeys>,
    lock: Option<active_core::provenance::IncludeLock>,
    store: Option<PathBuf>,
) -> Result<active_core::Module, Vec<active_core::Diagnostic>> {
    let options = options_with_lock(resilient, allow_parent_escapes, lock, store);
    match trust {
        Some(keys) => active_core::parse_file_with_trust(file, options, keys),
        None => parse_file(file, options),
    }
}

/// Join a list for a diagnostic, capping untrusted length. A hostile
/// capability declaration must not become a megabyte error line.
fn capped_list(items: &[&str]) -> String {
    const SHOWN: usize = 20;
    let mut text = items
        .iter()
        .take(SHOWN)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > SHOWN {
        text.push_str(&format!(" (+{} more)", items.len() - SHOWN));
    }
    text
}

/// Negotiation handshake (PROPOSAL §6.3, CHECKLIST CAP.003): verify a
/// document's declared capabilities against the host's advertised
/// tools. Anything declared but not offered is refused, fail-closed,
/// before evaluation. Prints the verdict; Ok(()) only when nothing
/// is refused.
fn check_declared_against(
    file: &std::path::Path,
    descriptor: &serde_json::Value,
) -> anyhow::Result<()> {
    let name = file.to_string_lossy().to_string();
    let module = parse_input(file, false, false, &None)?;
    let host_tools: Vec<&str> = descriptor
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                .collect()
        })
        .unwrap_or_default();
    let refused = active_core::convert::refused_capabilities(&module.capabilities, &host_tools);
    if refused.is_empty() {
        if module.capabilities.is_empty() {
            outln!("capabilities ok (none declared)");
        } else {
            outln!(
                "capabilities ok: {}",
                capped_list(
                    &module
                        .capabilities
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                )
            );
        }
        let _ = name;
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "host refuses capabilities: {} (document {name} declares them, this host does not offer them)",
            capped_list(&refused)
        ))
    }
}

/// Drop retained blob payloads for hashing: the audit result must be
/// stable across `--materialise-blobs` (identity is the content).
fn strip_blob_payloads(value: &active_core::Value) -> active_core::Value {
    active_core::convert::strip_blob_payloads(value)
}

/// True when the arena builds any content-addressed value.
fn module_has_blobs(module: &active_core::Module) -> bool {
    module
        .arena
        .iter()
        .any(|node| matches!(node, active_core::ir::Node::ContentAddressed { .. }))
}

/// Evaluate an in-memory source in the given file context and report
/// whether it is clean plus its sorted diagnostic codes. Used by partial
/// repair so the reported verdict describes the applied subset.
fn source_verdict(engine: &Engine, source: &str, file: &std::path::Path) -> (bool, Vec<String>) {
    fn code_of(d: &active_core::Diagnostic) -> String {
        format!(
            "{}{}",
            d.code,
            d.path
                .as_deref()
                .map(|p| format!("@{p}"))
                .unwrap_or_default()
        )
    }
    let options = with_parse_limits(ParseOptions {
        enable_includes: true,
        ..Default::default()
    });
    let name = file.to_string_lossy();
    let (module, parse_diags) =
        active_core::parse_text_recovering_with_file(source, &name, options, file);
    let mut codes: Vec<String> = parse_diags.iter().map(code_of).collect();
    if parse_diags.is_empty() {
        match engine.evaluate(&module) {
            Ok((_, diags)) => codes.extend(diags.iter().map(code_of)),
            // Strict validation surfaces its diagnostics inside the
            // error; keep their paths so the verdict names the failing
            // location, exactly as the Ok path does.
            Err(active_core::EvalError::ValidationFailed { diagnostics }) => {
                codes.extend(diagnostics.iter().map(code_of));
            }
            Err(e) => codes.push(e.code().to_string()),
        }
    }
    codes.sort();
    codes.dedup();
    (codes.is_empty(), codes)
}

/// Write a v2 tool-result envelope (L-T06): success and adversarial
/// outcomes share the shape; a failed attempt carries `payload = null`
/// and a receipt with its failure class.
fn write_envelope_v2(
    path: &std::path::Path,
    capability: &str,
    tainted: bool,
    payload: serde_json::Value,
    result: Option<&str>,
    receipt: serde_json::Value,
    sender: Option<&str>,
) -> anyhow::Result<()> {
    let mut envelope = serde_json::Map::new();
    envelope.insert("version".into(), serde_json::json!("2.0.0"));
    envelope.insert("capability".into(), serde_json::json!(capability));
    envelope.insert("tainted".into(), serde_json::json!(tainted));
    envelope.insert("payload".into(), payload);
    envelope.insert("receipt".into(), receipt);
    if let Some(result) = result {
        envelope.insert("result".into(), serde_json::json!(result));
    }
    if let Some(sender) = sender {
        envelope.insert("sender".into(), serde_json::json!({ "id": sender }));
    }
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(envelope))
        .map_err(|e| anyhow::anyhow!("cannot render envelope: {e}"))?;
    std::fs::write(path, text).map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))
}

/// Secret-redacted audit document (PROPOSAL §5.1, CHECKLIST AUD.001):
/// counts, diagnostic codes, mode, and the content hash of the REDACTED
/// result — itself a valid Active document, never raw values or secrets.
/// Written on success, on validation failure, and on fatal engine errors
/// (LANG-F-010: the failure_class field names the stage). `value` is the
/// canonical redacted full tree, already payload-free.
#[allow(clippy::too_many_arguments)]
fn write_audit_trail(
    path: &std::path::Path,
    file: &std::path::Path,
    module: &active_core::Module,
    value: Option<&active_core::Value>,
    diags: &[active_core::Diagnostic],
    resilient: bool,
    profile: Option<&str>,
    budget: &active_core::Budget,
    allow_parent_escapes: bool,
    materialise_blobs: bool,
    failure_class: Option<&str>,
    peer: Option<&str>,
) -> anyhow::Result<()> {
    fn quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                // A raw `$` opens interpolation; `\$` is the literal
                // dollar escape, so a path like `${x}.active` stays
                // data and the audit re-validates.
                '$' => out.push_str("\\$"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\0' => out.push_str("\\0"),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }
    let mut codes: Vec<String> = diags
        .iter()
        .map(|d| {
            format!(
                "{}{}",
                d.code,
                d.path
                    .as_deref()
                    .map(|p| format!("@{p}"))
                    .unwrap_or_default()
            )
        })
        .collect();
    codes.sort();
    codes.dedup();
    let codes_list = codes
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    // Outcome mirrors had_failure exactly: strict fails on any
    // warning; resilient tolerates warnings but never errors. A strict
    // validation failure has no result tree; `result = null` keeps the
    // audit valid Active.
    let failed = had_failure(diags, resilient);
    let result = match value {
        Some(value) => format!("\"{}\"", active_core::provenance::content_hash(value)),
        None => "null".to_string(),
    };
    let profile_line = profile
        .map(|name| format!("  profile = {},\n", quote(name)))
        .unwrap_or_default();
    // The resolved evaluation inputs are recorded so an audit is
    // self-describing: replay must use the same limits and parse flags.
    let budget_line = format!(
        "  budget = {{ nodes = {}, depth = {}, array_len = {}, keys = {}, wall_ms = {}, value_size = {}, total_size = {} }},\n  parse = {{ resilient = {}, allow_parent_escapes = {}, materialise_blobs = {} }},\n",
        budget.max_nodes,
        budget.max_depth,
        budget.max_array_len,
        budget.max_keys,
        budget.max_wall_ms,
        budget.max_value_size,
        budget.max_total_value_size,
        resilient,
        allow_parent_escapes,
        materialise_blobs,
    );
    // LANG-F-010: a failed attempt records WHY it failed (parse, budget,
    // validation, other) and, when a transport supplied one, the peer the
    // document came from - a rejected peer document leaves auditable
    // evidence instead of nothing.
    let class_line = failure_class
        .map(|class| format!("  failure_class = {},\n", quote(class)))
        .unwrap_or_default();
    let peer_line = peer
        .map(|id| format!("  peer = {},\n", quote(id)))
        .unwrap_or_default();
    // L-P03: the receipt records how every include was resolved (inline
    // pin, lockfile path/content, or unpinned), so a replay can prove the
    // include graph it ran.
    let include_line = if module.include_resolutions.is_empty() {
        String::new()
    } else {
        let entries = module
            .include_resolutions
            .iter()
            .map(|(pin, kind)| {
                let source = match kind {
                    active_core::ir::IncludeResolutionKind::Inline => "inline",
                    active_core::ir::IncludeResolutionKind::LockPath => "lock-path",
                    active_core::ir::IncludeResolutionKind::LockContent => "lock-content",
                    active_core::ir::IncludeResolutionKind::Unpinned => "unpinned",
                };
                format!(
                    "{{ path = {}, resolved = {}, sha256 = \"sha256:{}\", source = \"{source}\" }}",
                    quote(&pin.as_written),
                    quote(&pin.resolved),
                    pin.sha256
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("  include_resolutions = [{entries}],\n")
    };
    let doc = format!(
        "audit = {{\n  tool = \"active\",\n  version = {},\n  source = {},\n  mode = \"{}\",\n{}{}{}{}{}  nodes = {},\n  diagnostics = [{}],\n  result = {},\n  outcome = \"{}\"\n}}\n",
        quote(env!("CARGO_PKG_VERSION")),
        quote(&file.to_string_lossy()),
        match module.mode {
            active_core::Mode::Data => "data",
            active_core::Mode::Config => "config",
        },
        profile_line,
        budget_line,
        class_line,
        peer_line,
        include_line,
        module.arena.len(),
        codes_list,
        result,
        if failed { "failed" } else { "clean" },
    );
    std::fs::write(path, doc).map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))
}

/// LANG-F-010: a parse failure that was asked for an audit writes one. No
/// module exists, so the record carries the attempt's identity, the parse
/// diagnostics, the resolved budget, and `result = null`.
#[allow(clippy::too_many_arguments)]
fn write_failed_parse_audit(
    path: &std::path::Path,
    file: &std::path::Path,
    diags: &[active_core::Diagnostic],
    resilient: bool,
    allow_parent_escapes: bool,
    materialise_blobs: bool,
    peer: Option<&str>,
) -> anyhow::Result<()> {
    fn quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '$' => out.push_str("\\$"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\0' => out.push_str("\\0"),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }
    let mut codes: Vec<String> = diags.iter().map(|d| d.code.clone()).collect();
    codes.sort();
    codes.dedup();
    let codes_list = codes
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let peer_line = peer
        .map(|id| format!("  peer = {},\n", quote(id)))
        .unwrap_or_default();
    let doc = format!(
        "audit = {{\n  tool = \"active\",\n  version = {},\n  source = {},\n  mode = \"unknown\",\n  budget = {{ nodes = {}, depth = {}, array_len = {}, keys = {}, wall_ms = {}, value_size = {}, total_size = {} }},\n  parse = {{ resilient = {}, allow_parent_escapes = {}, materialise_blobs = {} }},\n{}  nodes = 0,\n  diagnostics = [{}],\n  result = null,\n  failure_class = \"parse\",\n  outcome = \"failed\"\n}}\n",
        quote(env!("CARGO_PKG_VERSION")),
        quote(&file.to_string_lossy()),
        active_core::eval::Budget::default().max_nodes,
        active_core::eval::Budget::default().max_depth,
        active_core::eval::Budget::default().max_array_len,
        active_core::eval::Budget::default().max_keys,
        active_core::eval::Budget::default().max_wall_ms,
        active_core::eval::Budget::default().max_value_size,
        active_core::eval::Budget::default().max_total_value_size,
        resilient,
        allow_parent_escapes,
        materialise_blobs,
        peer_line,
        codes_list,
    );
    std::fs::write(path, doc).map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))
}

/// Decidable graph query entry point (PROPOSAL §2.6, CHECKLIST
/// GQ.004): runs one named query over the parsed module, evaluating
/// only for value-dependent queries (secret-tainted-cone,
/// content-addressed-above-size). Outputs are paths and identity
/// metadata only — values and secrets never cross.
#[allow(clippy::too_many_arguments)]
fn run_graph_query(
    from: &std::path::Path,
    query: &str,
    key: Option<&str>,
    min_size: u64,
    format: &str,
    resilient: bool,
    allow_parent_escapes: bool,
    trust_keys: &Option<PathBuf>,
    profile: &Option<String>,
) -> anyhow::Result<()> {
    let trust = load_trust(trust_keys)?;
    let module = parse_input(from, resilient, allow_parent_escapes, &trust)?;
    let mut evaluation_engine = engine(resilient, profile, Budget::default(), None);
    let name = from.to_string_lossy().to_string();
    for d in &module.healing {
        report(d, &name);
    }
    let need_key = |what: &str| -> anyhow::Result<String> {
        key.map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("{what} needs --key <dotted-path>"))
    };
    let print_paths = |title: &str, paths: &[String]| -> anyhow::Result<()> {
        match format {
            "text" => {
                for p in paths {
                    outln!("{p}");
                }
                Ok(())
            }
            "json" => {
                outln!("{}", serde_json::json!({"query": title, "paths": paths}));
                Ok(())
            }
            other => anyhow::bail!("unsupported format: {other}"),
        }
    };
    match query {
        "dependents-of" => {
            let key = need_key("dependents-of")?;
            let id = active_core::graph::resolve_key(&module, &key)
                .ok_or_else(|| anyhow::anyhow!("unknown key '{key}' in {name}"))?;
            print_paths(query, &active_core::graph::impact_paths(&module, id))
        }
        "dependencies-of" => {
            let key = need_key("dependencies-of")?;
            let id = active_core::graph::resolve_key(&module, &key)
                .ok_or_else(|| anyhow::anyhow!("unknown key '{key}' in {name}"))?;
            print_paths(query, &active_core::graph::why_paths(&module, id))
        }
        "shortest-path" => {
            let pair = need_key("shortest-path (as \"from-key,to-key\")")?;
            let mut parts = pair.splitn(2, ',');
            let (from_key, to_key) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            if from_key.is_empty() || to_key.is_empty() {
                anyhow::bail!("shortest-path needs --key <from-key>,<to-key>");
            }
            let from_id = active_core::graph::resolve_key(&module, from_key)
                .ok_or_else(|| anyhow::anyhow!("unknown key '{from_key}' in {name}"))?;
            let to_id = active_core::graph::resolve_key(&module, to_key)
                .ok_or_else(|| anyhow::anyhow!("unknown key '{to_key}' in {name}"))?;
            match active_core::graph::shortest_path(&module, from_id, to_id) {
                Some(path) => print_paths(query, &path),
                None => anyhow::bail!("no path from '{from_key}' to '{to_key}'"),
            }
        }
        "secret-tainted-cone" | "content-addressed-above-size" => {
            if query.starts_with("secret") {
                // Payload-resident secrets are only visible when the
                // bytes are retained; the query emits paths, never bytes.
                evaluation_engine.materialise_blobs = true;
                evaluation_engine.taint_probe = true;
            }
            let (value, diags, seeds) = evaluation_engine
                .evaluate_with_seeds(&module)
                .map_err(|e| anyhow::anyhow!("evaluation failed: {e}"))?;
            for d in &diags {
                report(d, &name);
            }
            if query.starts_with("secret") {
                print_paths(
                    query,
                    &active_core::graph::secret_tainted_paths_with_seeds(
                        &module,
                        &value,
                        profile.as_deref(),
                        Some(&evaluation_engine.env),
                        Some(&seeds),
                    ),
                )
            } else {
                // Profile-aware: an identity the export would redact
                // (path materiality, a material media type, or a
                // payload-resident secret) is reported with the synthetic
                // redacted digest, never the hidden hash.
                let material_profile = active_core::secret::SecretProfile::for_engine(
                    &module,
                    &evaluation_engine,
                    &seeds,
                );
                let blobs = active_core::graph::blobs_above_size_with(
                    &value,
                    min_size,
                    &material_profile,
                );
                match format {
                    "text" => {
                        for (path, size, hash) in &blobs {
                            outln!("{path} size={size} blake3:{hash}");
                        }
                        Ok(())
                    }
                    "json" => {
                        outln!(
                            "{}",
                            serde_json::json!({"query": query, "blobs": blobs.iter().map(|(path, size, hash)| serde_json::json!({"path": path, "size": size, "hash": format!("blake3:{hash}")})).collect::<Vec<_>>()})
                        );
                        Ok(())
                    }
                    other => anyhow::bail!("unsupported format: {other}"),
                }
            }
        }
        other => anyhow::bail!(
            "unknown query: {other} (want dependents-of | dependencies-of | shortest-path | secret-tainted-cone | content-addressed-above-size)"
        ),
    }
}

fn engine(
    resilient: bool,
    profile: &Option<String>,
    budget: Budget,
    reader_version: Option<&str>,
) -> Engine {
    let mut engine = Engine::new(); // The CLI is strict by default: only --resilient tolerates
                                    // violations. Engine::new stays resilient for library use.
    engine.resilient = resilient;
    engine.profile_override = profile.clone();
    engine.budget = budget;
    engine.reader_version = reader_version.map(str::to_string);
    engine
}

/// Parse a file for graph queries (no evaluation needed — the
/// dependency structure is fully present after parsing).
fn parse_graph_input(
    file: &std::path::Path,
    resilient: bool,
    allow_parent_escapes: bool,
    trust: &Option<active_core::sign::TrustedKeys>,
) -> anyhow::Result<(active_core::Module, String)> {
    let name = file.to_string_lossy().to_string();
    let module = parse_input(file, resilient, allow_parent_escapes, trust)?;
    Ok((module, name))
}

/// Embedded agent capability descriptor (agent-kit/capability.schema.json).
/// Compiled in so installs answer the handshake without the repo tree.
const CAPABILITY_JSON: &str = include_str!("../../agent-kit/capability.schema.json");

/// Material profile for planning output: module prose plus the process
/// environment values its material env reads would resolve to. Hermetic
/// tools do not evaluate, so this is the widest context they can see.
fn material_profile(
    module: &active_core::Module,
) -> anyhow::Result<active_core::secret::SecretProfile> {
    material_profile_with(module, None)
}

/// Write a key file with pinned permissions (LANG-H-002/H-901).
///
/// Unix: the file is CREATED with `mode` (0600 secrets, 0644 public), so
/// there is no write-then-chmod window; the mode is pinned again after
/// opening so a pre-existing file cannot keep a wider mode. Windows has no
/// POSIX modes: the file inherits the directory ACL, which `keygen`'s help
/// states.
fn write_key_file(path: &std::path::Path, contents: &str, mode: u32) -> anyhow::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options
        .open(path)
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?;
    file.write_all(contents.as_bytes())
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?;
    file.sync_all()
        .map_err(|e| anyhow::anyhow!("cannot flush {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|e| anyhow::anyhow!("cannot chmod {}: {e}", path.display()))?;
    }
    Ok(())
}

/// LANG-K-001: a profile that hit an analysis bound treats every unknown
/// path as material, so presenting its tree would replace benign values
/// with `***SECRET***`. Value exports refuse loudly instead of lying; the
/// profile caps exceed the trusted budget, so this is the honest signal
/// for a host that raised the parse caps past the evaluator's scale.
fn complete_profile(
    profile: active_core::secret::SecretProfile,
) -> anyhow::Result<active_core::secret::SecretProfile> {
    if profile.truncated() {
        anyhow::bail!(
            "secret analysis hit its analysis bounds (path or work caps); refusing to \
             export a document whose benign values may be redacted"
        );
    }
    Ok(profile)
}

/// Material profile under an optional `--profile` override. The
/// override can select a path whose environment read is material, so
/// analyzing with the document selector alone would miss it.
fn material_profile_with(
    module: &active_core::Module,
    override_profile: Option<&str>,
) -> anyhow::Result<active_core::secret::SecretProfile> {
    let mut profile =
        active_core::secret::SecretProfile::analyze_with_profile(module, override_profile);
    let mut env = std::collections::HashMap::new();
    for node in &module.arena {
        if let active_core::ir::Node::Env { name, .. } = node {
            if profile.env_is_material(name) {
                // LANG-H-003: the engine folds env names to uppercase at
                // snapshot time; the tool profile reads the same spelling
                // so a mixed-case reference absorbs the right value.
                if let Ok(value) = std::env::var(active_core::eval::canonical_env_name(name)) {
                    env.insert(name.clone(), value);
                }
            }
        }
    }
    profile.absorb_environment(&env);
    complete_profile(profile)
}

/// Strict-evaluation diagnostics for the explain/patch surfaces: parse
/// errors abort in parse_input; validation diagnostics are the working
/// set; any other engine failure is fatal and reported.
fn collect_diags(
    module: &active_core::Module,
    filename: &str,
    budget: Budget,
) -> (Vec<Diagnostic>, bool) {
    match engine(false, &None, budget, None).evaluate(module) {
        Ok((_, diags)) => (diags, false),
        Err(EvalError::ValidationFailed { diagnostics }) => (diagnostics, false),
        Err(e) => {
            // LANG-G-005: a hard evaluation error is still explainable. The
            // error's code and message become one explanation item (the
            // same shape soft diagnostics use), so `--format json` carries
            // it in the stable array and text prints the canned guidance.
            // The command keeps exit 2: the document still fails.
            let diagnostic = Diagnostic {
                severity: active_core::Severity::Error,
                code: e.code().to_string(),
                message: format!("{e}"),
                path: None,
                span: e.span().cloned(),
                repair: None,
                confidence: 1.0,
                file: Some(filename.to_string()),
            };
            let _ = filename;
            (vec![diagnostic], true)
        }
    }
}

/// Stable (code, path, span) triples feeding both text and JSON output.
fn explain_items(diags: &[Diagnostic]) -> Vec<(String, Option<String>, Option<String>)> {
    diags
        .iter()
        .map(|d| {
            let span = d
                .span
                .as_ref()
                .map(|s| format!("{}:{}-{}:{}", s.line, s.column, s.end_line, s.end_column));
            (d.code.clone(), d.path.clone(), span)
        })
        .collect()
}

/// Stable `CODE path span: guidance` lines (value-free by construction).
fn explain_lines(diags: &[Diagnostic]) -> Vec<String> {
    active_core::convert::explain_all(&explain_items(diags))
}

/// True when the exit code must be non-zero: strict mode fails on any
/// Warning-or-Error; resilient mode fails only on Error.
fn had_failure(diags: &[Diagnostic], resilient: bool) -> bool {
    diags
        .iter()
        .any(|d| d.severity == Severity::Error || (!resilient && d.severity == Severity::Warning))
}

/// Load (or create) the per-user cache MAC key. The key lives outside
/// the cache directory, so the directory alone cannot reproduce a key
/// name or confirm a guessed environment value (L-P29 review F2/F3).
fn cache_signing_key() -> Result<([u8; active_core::cache::KEY_BYTES], Option<u32>), String> {
    let path = active_core::cache::default_key_path().ok_or_else(|| {
        format!(
            "cannot locate a cache key file; set {}",
            active_core::cache::KEY_ENV
        )
    })?;
    let key = active_core::cache::load_or_create_key(&path)?;
    Ok((key, active_core::cache::file_owner(&path)))
}

/// The v2 receipt stored in an L-P29 cache entry (L-T06 shape): it binds
/// the served value to the schema, profile, budget, mode, and capability
/// that produced it, so a cache file is provenance evidence rather than
/// an unverifiable blob.
fn cache_receipt(
    module: &active_core::Module,
    profile: Option<&str>,
    budget: &active_core::eval::Budget,
    value: &active_core::Value,
    capability: &str,
    diags: &[Diagnostic],
    resilient: bool,
) -> serde_json::Value {
    let failed = had_failure(diags, resilient);
    let bound = active_core::envelope::bound_content_hash(
        value,
        active_core::envelope::document_schema_version(module).as_deref(),
        profile,
        budget,
        module.mode,
        Some(capability),
        active_core::envelope::payload_tainted(value),
    );
    let resolved = budget.resolve();
    let mut receipt = serde_json::Map::new();
    receipt.insert(
        "outcome".into(),
        serde_json::json!(if failed { "failed" } else { "clean" }),
    );
    if failed {
        // Engine-level failures are classified when the envelope names
        // its stage; a cached resilient run only knows it did not pass.
        receipt.insert("failure_class".into(), serde_json::json!("other"));
    } else {
        receipt.insert("result".into(), serde_json::json!(bound));
    }
    receipt.insert("capability".into(), serde_json::json!(capability));
    receipt.insert(
        "mode".into(),
        serde_json::json!(match module.mode {
            active_core::Mode::Config => "config",
            active_core::Mode::Data => "data",
        }),
    );
    if let Some(profile) = profile {
        receipt.insert("profile".into(), serde_json::json!(profile));
    }
    if let Some(version) = active_core::envelope::document_schema_version(module) {
        receipt.insert("schema_version".into(), serde_json::json!(version));
    }
    receipt.insert(
        "budget".into(),
        serde_json::json!({
            "nodes": resolved.max_nodes,
            "depth": resolved.max_depth,
            "array_len": resolved.max_array_len,
            "keys": resolved.max_keys,
            "wall_ms": resolved.max_wall_ms,
            "value_size": resolved.max_value_size,
            "total_size": resolved.max_total_value_size,
        }),
    );
    receipt.insert(
        "diagnostics".into(),
        serde_json::json!(diags.iter().map(|d| d.code.clone()).collect::<Vec<_>>()),
    );
    serde_json::Value::Object(receipt)
}

/// Dump the opt-in evaluation trail as JSONL on stderr. Events carry
/// counts and diagnostic codes only, never values, sources, or secrets
/// (enforced by construction in active_core::trace).
fn dump_trail(enabled: bool) {
    if !enabled {
        return;
    }
    // Trace serialization cannot fail for the closed TraceEvent shape,
    // but a failed event must never abort a successful evaluation:
    // skip it so --trace stays observability-only.
    for event in active_core::trace::take_trail() {
        if let Ok(value) = serde_json::to_value(&event) {
            eprintln!("{value}");
        }
    }
}

/// Print a diagnostic with its file and span when available. Multi-file
/// documents (includes) attribute each diagnostic to its own file.
fn report(d: &Diagnostic, fallback_file: &str) {
    let file = d.file.as_deref().unwrap_or(fallback_file);
    match &d.span {
        Some(s) => eprintln!(
            "{}:{}:{}: [{:?}] {}: {}",
            file, s.line, s.column, d.severity, d.code, d.message
        ),
        None => eprintln!("[{:?}] {}: {}", d.severity, d.code, d.message),
    }
}

/// Print an evaluation failure. Schema violations (strict mode) print
/// every carried diagnostic before the summary error.
fn report_eval_error(e: &EvalError, name: &str) {
    if let EvalError::ValidationFailed { diagnostics } = e {
        for d in diagnostics {
            report(d, name);
        }
    }
    // LANG-C-009: a budget trip replaces the diagnostics the walk had
    // collected, so the progressive warnings that preceded it travel on the
    // error. Print them first: they are the operator's advance notice.
    for d in e.attached_warnings() {
        report(d, name);
    }
    let file = e.file().unwrap_or(name);
    match e.span() {
        Some(s) => eprintln!(
            "{}:{}:{}: [Error] {}: {}",
            file,
            s.line,
            s.column,
            e.code(),
            e
        ),
        None => eprintln!("[Error] {}: {}", e.code(), e),
    }
}

/// Join a worker thread, turning a panic into a coded, non-panicking
/// failure (LANG-I-012). The FFI and WASM boundaries already isolate
/// panics; the CLI must not abort with exit 101 and a bare trace. The
/// panic payload is deliberately not echoed: it can contain document
/// text or secret material.
fn join_worker<T>(handle: std::thread::JoinHandle<T>, what: &str) -> anyhow::Result<T> {
    match handle.join() {
        Ok(value) => Ok(value),
        Err(_) => Err(anyhow::anyhow!(
            "E030: {what} panicked and the run was aborted; no partial output was released \
             (report this with the input if it is shareable)"
        )),
    }
}

/// LANG-H-007: Rust ignores `SIGPIPE` at startup, so a pipeline reader
/// that closes early (`active eval ... | head -1`) turns the next stdout
/// write into an EPIPE panic (exit 101) with a backtrace instead of the
/// conventional silent death (141) every Unix shell expects.
///
/// The disposition is restored with `sigaction`, not `signal`: on macOS
/// `signal()` reports success while writes still return `EPIPE` (Apple's
/// libc keeps extra action state, so only a fully zeroed action with
/// `SA_SIGINFO` clear restores the default). See the `sigpipe` crate,
/// which exists for this exact platform difference.
#[cfg(unix)]
fn restore_default_sigpipe() {
    // SAFETY: `action` is fully initialised before use (`zeroed` is the
    // documented starting point for `sigaction`, `sigemptyset` writes the
    // mask) and a disposition change is process-global by definition.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = 0;
        libc::sigaction(libc::SIGPIPE, &action, std::ptr::null_mut());
    }
}

fn main() -> anyhow::Result<()> {
    // Before any output: the first write after an early reader close must
    // kill the filter, not panic.
    #[cfg(unix)]
    restore_default_sigpipe();
    // The derived clap command tree is large; parse on a thread with a
    // generous stack so debug builds on Windows cannot overflow while
    // building help or parsing arguments.
    let cli = join_worker(
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(Cli::parse)
            .expect("spawn argument parser"),
        "the argument parser",
    )?;
    // Resolve the global parse limits once, before any command runs
    // (LANG-C-003): every parse path then reads the same values.
    // Same one-meaning rule as the evaluation budgets (LANG-C-015): an
    // absent flag uses the default, an explicit 0 is the documented opt-out
    // (the parse sentinel), never a zero travelling as "default".
    let parse_bytes = match cli.max_input_bytes {
        None => active_core::parser::DEFAULT_MAX_INPUT_BYTES,
        Some(0) => active_core::parser::UNLIMITED_PARSE_BYTES,
        Some(n) => n,
    };
    let parse_ms = match cli.parse_wall_ms {
        None => active_core::parser::DEFAULT_MAX_PARSE_WALL_MS,
        Some(0) => active_core::parser::UNLIMITED_PARSE_WALL_MS,
        Some(n) => n,
    };
    let _ = PARSE_LIMITS.set((parse_bytes, parse_ms));

    // Parsing and evaluation are recursive; run the command on a worker
    // with a generous stack so the documented 64-level expression limit
    // stays reachable in unoptimized builds (debug CLIs, test runners).
    let worker = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || -> anyhow::Result<()> {
    match cli.command {
        Commands::Eval {
            file,
            format,
            resilient,
            allow_parent_escapes,
            trust_keys,
            lockfile,
            store,
            cache,
            profile,
            trace,
            materialise_blobs,
            path,
            audit_trail,
            envelope,
            envelope_trust,
            peer,
            sandbox,
            budget,
            reader_version,
        } => {
            let name = file.to_string_lossy().to_string();
            // LANG-I-002c: lockfile drift fails closed before evaluation.
            // L-P03: the same lock also pins the include graph by
            // spelling/resolved path; `--trust-keys` defaults to the
            // sibling `active-lock.toml`.
            let trust = load_trust(&trust_keys)?;
            let include_lock = match lockfile.as_deref() {
                Some(lock) => Some(explicit_include_lock(lock, &file, None)?),
                None if trust.is_some() => sibling_include_lock(&file)?,
                None => None,
            };
            let store_requested = store.is_some();
            // L-P29: a content-keyed cache turns a repeat run into a
            // read. Run observers (the audit trail, the envelope, the
            // trace) record THIS run, and admission flags (the L-P04
            // store, trust keys, a lockfile, envelope trust) add side
            // effects, policy, or parse-time diagnostics that are not in
            // the key; each of those bypasses the cache. Decide before
            // parsing so the warning also appears on a parse failure.
            let cache_compatible = audit_trail.is_none()
                && envelope.is_none()
                && !trace
                && !store_requested
                && trust_keys.is_none()
                && lockfile.is_none()
                && envelope_trust.is_none();
            if cache.is_some() && !cache_compatible {
                eprintln!(
                    "warning: --cache is not used with --audit-trail, --envelope, --trace, --store, --trust-keys, --lockfile, or --envelope-trust"
                );
            }
            // An empty root would resolve entry names against the
            // working directory; refuse it like the include store does.
            if cache
                .as_ref()
                .is_some_and(|path| path.as_os_str().is_empty())
            {
                anyhow::bail!("--cache requires a directory path");
            }
            let module = match parse_input_or_diags_locked(
                &file,
                resilient,
                allow_parent_escapes,
                &trust,
                include_lock,
                store,
            ) {
                Ok(module) => module,
                Err(diags) => {
                    for d in &diags {
                        report(d, &name);
                    }
                    // LANG-F-010: a parse failure leaves auditable evidence.
                    if let Some(audit_path) = audit_trail.as_ref() {
                        write_failed_parse_audit(
                            audit_path,
                            &file,
                            &diags,
                            resilient,
                            allow_parent_escapes,
                            materialise_blobs,
                            peer.as_deref(),
                        )?;
                    }
                    if let Some(envelope_path) = envelope.as_ref() {
                        let codes: Vec<String> =
                            diags.iter().map(|d| d.code.clone()).collect();
                        write_envelope_v2(
                            envelope_path,
                            "agent",
                            false,
                            serde_json::Value::Null,
                            None,
                            serde_json::json!({
                                "outcome": "failed",
                                "failure_class": "parse",
                                "diagnostics": codes,
                            }),
                            peer.as_deref(),
                        )?;
                    }
                    return Err(anyhow::anyhow!("parse failed"));
                }
            };

            // Sandbox gate (PROPOSAL §3.5): the named profile selects
            // its budget preset and refuses documents declaring
            // capabilities outside the profile's tool set — the host
            // refuses excess requests before evaluation.
            let explicit_flags = budget.has_explicit();
            let mut budget = budget.resolve()?;
            if let Some(ref sandbox) = sandbox {
                let profile_def =
                    active_core::convert::sandbox_profile(sandbox).ok_or_else(|| {
                        anyhow::anyhow!(
                            "unknown sandbox profile: {sandbox} (want strict|data|agent|trusted)"
                        )
                    })?;
                // Like --budget-profile: a named preset wins over
                // explicit per-field flags — warn only when the user
                // actually passed --max-* (not when another profile
                // already selected non-default budgets).
                if explicit_flags {
                    eprintln!("warning: --sandbox overrides the explicit --max-* flags");
                }
                budget = match profile_def.budget {
                    "strict" => Budget::strict(),
                    "interactive" => Budget::interactive(),
                    "trusted" => Budget::trusted(),
                    other => anyhow::bail!("unknown profile budget: {other}"),
                };
                let host_tools: Vec<&str> = profile_def.tools.to_vec();
                let refused =
                    active_core::convert::refused_capabilities(&module.capabilities, &host_tools);
                if !refused.is_empty() {
                    if let Some(path) = envelope.as_ref() {
                        write_envelope_v2(
                            path,
                            "agent",
                            false,
                            serde_json::Value::Null,
                            None,
                            serde_json::json!({
                                "outcome": "failed",
                                "failure_class": "validation",
                                "diagnostics": ["E034"],
                            }),
                            peer.as_deref(),
                        )?;
                    }
                    // Names are document data and may be secret-derived;
                    // the count locates the problem without echoing them.
                    anyhow::bail!(
                        "sandbox '{sandbox}' refuses {} declared capabilit{}",
                        refused.len(),
                        if refused.len() == 1 { "y" } else { "ies" }
                    );
                }
            }

            let mut eng = engine(resilient, &profile, budget, reader_version.as_deref());
            eng.materialise_blobs = materialise_blobs;
            eng.envelope_trust = load_trust(&envelope_trust)?;
            let engine = eng;
            if audit_trail.is_some() {
                validate_audit_limits(&engine.budget)?;
            }
            // The effective profile (override wins, else the document's
            // literal selector) is recorded in the audit so replay can
            // reproduce the same hash.
            let effective_profile = engine.profile_override.clone().or_else(|| {
                active_core::secret::active_profile_name(&module)
                    .filter(|name| module.profiles.contains_key(name))
            });
            // L-P29: a run whose callers need the run itself, or whose
            // admission adds side effects outside the key, was already
            // reported as uncached before parsing.
            let cache_dir = cache.filter(|_| cache_compatible);
            let cache_variant = format!(
                "source={:?};path={:?};materialise={materialise_blobs};reader={:?};resilient={resilient};capability={}",
                file.as_path(),
                path.as_deref(),
                reader_version.as_deref(),
                sandbox.as_deref().unwrap_or("agent"),
            );
            // The MAC key lives outside the cache directory: a listing
            // cannot reproduce a key or confirm a guessed environment
            // value. Without it the run proceeds uncached.
            let (cache_key, cache_owner) = if cache_dir.is_some() {
                match cache_signing_key() {
                    Ok((key, owner)) => (
                        active_core::cache::eval_key(
                            &module,
                            effective_profile.as_deref(),
                            &engine.budget,
                            module.mode,
                            &cache_variant,
                            &engine.env,
                            &key,
                        ),
                        owner,
                    ),
                    Err(e) => {
                        eprintln!("warning: --cache is not used: {e}");
                        (None, None)
                    }
                }
            } else {
                (None, None)
            };
            if let (Some(dir), Some(key)) = (cache_dir.as_ref(), cache_key.as_ref()) {
                match active_core::cache::EvalCache::new(dir)
                    .with_owner(cache_owner)
                    .get(key)
                {
                    Ok(Some(entry)) => {
                        for d in &entry.diagnostics {
                            report(d, &name);
                        }
                        match format.as_str() {
                            "json" => outln!("{}", entry.json),
                            "yaml" => match (&entry.yaml, &entry.yaml_error) {
                                (Some(text), _) => outln!("{text}"),
                                (None, Some(e)) => anyhow::bail!("yaml export failed: {e}"),
                                (None, None) => anyhow::bail!(
                                    "yaml export failed: cache entry carries no rendering"
                                ),
                            },
                            "toml" => match (&entry.toml, &entry.toml_error) {
                                (Some(text), _) => {
                                    for d in &entry.export_diagnostics {
                                        report(d, &name);
                                    }
                                    outln!("{text}");
                                }
                                (None, Some(e)) => anyhow::bail!("toml export failed: {e}"),
                                (None, None) => anyhow::bail!(
                                    "toml export failed: cache entry carries no rendering"
                                ),
                            },
                            other => anyhow::bail!("unsupported format: {other}"),
                        }
                        if had_failure(&entry.diagnostics, resilient) {
                            std::process::exit(1);
                        }
                        return Ok(());
                    }
                    Ok(None) => {}
                    // A corrupt entry is a miss with a warning: the cache
                    // is an optimization, never an input.
                    Err(e) => eprintln!("warning: ignoring cache entry: {e}"),
                }
            }
            active_core::trace::set_tracing(trace);
            let (value, diags, seeds) = match engine.evaluate_with_seeds(&module) {
                Ok(out) => {
                    dump_trail(trace);
                    out
                }
                Err(e) => {
                    dump_trail(trace);
                    // LANG-F-010: EVERY failed attempt leaves an audit when
                    // one was requested - budget trips included, not only
                    // strict validation refusals. The class names which
                    // stage refused, so a dispute has evidence.
                    if let Some(audit_path) = audit_trail.as_ref() {
                        // The audit names the refusing code, not only the
                        // warnings that preceded it.
                        let class_of = |e: &active_core::EvalError| match e {
                            active_core::EvalError::ValidationFailed { .. } => "validation",
                            active_core::EvalError::BudgetExceeded { .. } => "budget",
                            _ => "other",
                        };
                        let code_diag = active_core::Diagnostic {
                            severity: active_core::Severity::Error,
                            code: e.code().to_string(),
                            message: format!("{e}"),
                            path: None,
                            span: None,
                            repair: None,
                            confidence: 1.0,
                            file: None,
                        };
                        let audit_diags: Vec<active_core::Diagnostic> = match &e {
                            active_core::EvalError::ValidationFailed { diagnostics } => {
                                diagnostics.clone()
                            }
                            active_core::EvalError::BudgetExceeded { warnings, .. } => {
                                let mut v = warnings.clone();
                                v.push(code_diag);
                                v
                            }
                            _ => vec![code_diag],
                        };
                        let class = class_of(&e);
                        write_audit_trail(
                            audit_path,
                            &file,
                            &module,
                            None,
                            &audit_diags,
                            resilient,
                            effective_profile.as_deref(),
                            &engine.budget,
                            allow_parent_escapes,
                            materialise_blobs,
                            Some(class),
                            peer.as_deref(),
                        )?;
                    }
                    if let Some(envelope_path) = envelope.as_ref() {
                        let class = match &e {
                            active_core::EvalError::ValidationFailed { .. } => "validation",
                            active_core::EvalError::BudgetExceeded { .. } => "budget",
                            _ => "other",
                        };
                        let codes = match &e {
                            active_core::EvalError::ValidationFailed { diagnostics } => diagnostics
                                .iter()
                                .map(|d| d.code.clone())
                                .collect::<Vec<_>>(),
                            active_core::EvalError::BudgetExceeded { warnings, .. } => warnings
                                .iter()
                                .map(|d| d.code.clone())
                                .chain(std::iter::once(e.code().to_string()))
                                .collect(),
                            _ => vec![e.code().to_string()],
                        };
                        write_envelope_v2(
                            envelope_path,
                            "agent",
                            false,
                            serde_json::Value::Null,
                            None,
                            serde_json::json!({
                                "outcome": "failed",
                                "failure_class": class,
                                "diagnostics": codes,
                            }),
                            peer.as_deref(),
                        )?;
                    }
                    report_eval_error(&e, &name);
                    return Err(anyhow::anyhow!("evaluation failed"));
                }
            };
            for d in &diags {
                report(d, &name);
            }

            // Demanded cone selection (PROPOSAL §2.2): project AFTER the
            // full budgeted evaluation, so safety stays whole-document
            // while agents fetch only what they need. Structural
            // material is redacted on the FULL tree before projection,
            // so a cone keeps the document's path-relative taint.
            // Structural material is redacted on the FULL tree, then
            // projected: the cone is already redacted at document
            // paths, so it must NOT be re-rooted and redacted again.
            let profile = complete_profile(active_core::secret::SecretProfile::for_engine(
                &module, &engine, &seeds,
            ))?;
            let full_value = active_core::convert::redact_value(&profile, &value);
            let value = match path.as_deref() {
                Some(p) => active_core::convert::project_path(&full_value, p)
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                    .clone(),
                None => full_value.clone(),
            };
            // Explicit materialisation demand (spec/blob.md §5) runs on
            // the redacted cone: a tainted blob (node- or path-material)
            // is marked tainted there, so the demand fails closed with
            // B003 instead of silently dropping the payload.
            if materialise_blobs {
                if let Err(e) = active_core::convert::require_materialised(&value) {
                    if let Some(path) = envelope.as_ref() {
                        write_envelope_v2(
                            path,
                            "agent",
                            false,
                            serde_json::Value::Null,
                            None,
                            serde_json::json!({
                                "outcome": "failed",
                                "failure_class": "validation",
                                "diagnostics": ["B003"],
                            }),
                            peer.as_deref(),
                        )?;
                    }
                    report_eval_error(&e, &name);
                    return Err(anyhow::anyhow!("evaluation failed"));
                }
            }

            if let Some(envelope_path) = envelope.as_ref() {
                // Config documents are not interchange envelopes
                // (spec/agent-interchange-profile.md §2 rule 1), so a
                // tool-result envelope is refused rather than produced
                // for a table that is ordinary data (review M-07).
                if module.mode != active_core::Mode::Data {
                    anyhow::bail!(
                        "--envelope requires a Data-mode document (`active_mode = \"data\"`)"
                    );
                }
                // A material profile/schema selector must not be
                // written in cleartext (review H-04); refuse emission
                // instead.
                for (label, selector) in [
                    ("profile", effective_profile.as_deref()),
                    (
                        "schema_version",
                        active_core::envelope::document_schema_version(&module).as_deref(),
                    ),
                ] {
                    if selector.is_some_and(|text| {
                        profile.is_material_text(text)
                            || profile.redact_text(text).is_some()
                    }) {
                        anyhow::bail!(
                            "refusing to emit envelope: {label} is material and would leak in cleartext"
                        );
                    }
                }
                // The emitted envelope wraps the full redacted value, or
                // that document's own `envelope.payload` when it already
                // is an interchange document. Taint is computed from the
                // pre-redaction payload, so a plain secret never becomes
                // `tainted: false` (review M1).
                let projected = path.is_some();
                let original_payload = if projected {
                    value.clone()
                } else {
                    match &value {
                        active_core::Value::Table(root) => match root.get("envelope") {
                            Some(active_core::Value::Table(fields)) => fields
                                .get("payload")
                                .cloned()
                                .unwrap_or_else(|| value.clone()),
                            _ => value.clone(),
                        },
                        _ => value.clone(),
                    }
                };
                let computed_taint =
                    active_core::envelope::payload_tainted(&original_payload);
                let (payload, capability, claimed_tainted) = if projected {
                    (value.clone(), "agent".to_string(), computed_taint)
                } else {
                    match &full_value {
                        active_core::Value::Table(root) => match root.get("envelope") {
                            Some(active_core::Value::Table(fields)) => (
                                fields
                                    .get("payload")
                                    .cloned()
                                    .unwrap_or(active_core::Value::Null),
                                match fields.get("capability") {
                                    Some(active_core::Value::String(name)) => name.clone(),
                                    _ => "agent".to_string(),
                                },
                                matches!(
                                    fields.get("tainted"),
                                    Some(active_core::Value::Bool(true))
                                ),
                            ),
                            _ => (full_value.clone(), "agent".to_string(), false),
                        },
                        _ => (full_value.clone(), "agent".to_string(), false),
                    }
                };
                let mut tainted = claimed_tainted || computed_taint;
                let capability_used = sandbox
                    .clone()
                    .unwrap_or_else(|| "agent".to_string());
                let _ = capability;
                let bound = active_core::envelope::bound_content_hash(
                    &payload,
                    active_core::envelope::document_schema_version(&module).as_deref(),
                    effective_profile.as_deref(),
                    &engine.budget,
                    module.mode,
                    Some(&capability_used),
                    tainted,
                );
                let resolved = engine.budget.resolve();
                let mut receipt = serde_json::Map::new();
                receipt.insert("outcome".into(), serde_json::json!("clean"));
                receipt.insert("result".into(), serde_json::json!(bound.clone()));
                receipt.insert(
                    "mode".into(),
                    serde_json::json!(match module.mode {
                        active_core::Mode::Config => "config",
                        active_core::Mode::Data => "data",
                    }),
                );
                if let Some(profile) = effective_profile.as_deref() {
                    receipt.insert("profile".into(), serde_json::json!(profile));
                }
                if let Some(version) =
                    active_core::envelope::document_schema_version(&module)
                {
                    receipt.insert("schema_version".into(), serde_json::json!(version));
                }
                receipt.insert(
                    "budget".into(),
                    serde_json::json!({
                        "nodes": resolved.max_nodes,
                        "depth": resolved.max_depth,
                        "array_len": resolved.max_array_len,
                        "keys": resolved.max_keys,
                        "wall_ms": resolved.max_wall_ms,
                        "value_size": resolved.max_value_size,
                        "total_size": resolved.max_total_value_size,
                    }),
                );
                receipt.insert(
                    "diagnostics".into(),
                    serde_json::json!(diags
                        .iter()
                        .map(|d| d.code.clone())
                        .collect::<Vec<_>>()),
                );
                let mut payload_json =
                    serde_json::from_str(&active_core::convert::value_to_pretty_json(
                        &payload,
                    ))
                    .unwrap_or(serde_json::Value::Null);
                let mut result = Some(bound.clone());
                // Strict-mode warnings already make the process exit 1;
                // the envelope must agree with the audit/exit verdict
                // (review M2).
                if had_failure(&diags, resilient) {
                    payload_json = serde_json::Value::Null;
                    tainted = false;
                    result = None;
                    receipt.insert("outcome".into(), serde_json::json!("failed"));
                    receipt.insert("failure_class".into(), serde_json::json!("other"));
                    receipt.remove("result");
                }
                write_envelope_v2(
                    envelope_path,
                    &capability_used,
                    tainted,
                    payload_json,
                    result.as_deref(),
                    serde_json::Value::Object(receipt),
                    peer.as_deref(),
                )?;
            }

            // L-P29: with the cache on, render every format once so a
            // later hit serves any requested format without evaluating
            // again. Only the requested format prints, and an output
            // beyond the entry cap skips the cache before anything is
            // cloned into an entry.
            let cache_enabled = cache_key.is_some();
            let json_text = (format == "json" || cache_enabled).then(|| {
                if materialise_blobs {
                    value_to_pretty_json_materialised(&value)
                } else {
                    value_to_pretty_json(&value)
                }
            });
            let cache_json_fits = json_text.as_ref().is_none_or(|text| {
                text.len() as u64 <= active_core::cache::MAX_ENTRY_BYTES
            });
            let cache_extras = cache_enabled && cache_json_fits;
            let yaml_render = (format == "yaml" || cache_extras).then(|| {
                if materialise_blobs {
                    value_to_yaml_string_materialised(&value)
                } else {
                    value_to_yaml_string(&value)
                }
            });
            let toml_render = (format == "toml" || cache_extras).then(|| {
                if materialise_blobs {
                    value_to_toml_string_materialised(&value)
                } else {
                    value_to_toml_string(&value)
                }
            });
            if format == "json" {
                if let Some(text) = json_text.as_deref() {
                    outln!("{text}");
                }
            }
            if let Some(rendered) = yaml_render.as_ref() {
                match rendered {
                    Ok(yaml) if format == "yaml" => outln!("{yaml}"),
                    Err(e) if format == "yaml" => anyhow::bail!("yaml export failed: {e}"),
                    _ => {}
                }
            }
            if let Some(rendered) = toml_render.as_ref() {
                match rendered {
                    Ok((toml, export_diags)) if format == "toml" => {
                        for d in export_diags {
                            report(d, &name);
                        }
                        outln!("{toml}");
                    }
                    Err(e) if format == "toml" => anyhow::bail!("toml export failed: {e}"),
                    _ => {}
                }
            }
            if !matches!(format.as_str(), "json" | "yaml" | "toml") {
                anyhow::bail!("unsupported format: {}", format);
            }
            if let (Some(dir), Some(key)) = (cache_dir.as_ref(), cache_key.as_ref()) {
                let yaml_text = yaml_render
                    .as_ref()
                    .and_then(|render| render.as_ref().ok().map(String::as_str));
                let toml_text = toml_render
                    .as_ref()
                    .and_then(|render| render.as_ref().ok().map(|(text, _)| text.as_str()));
                // A material profile or schema selector must not be
                // written in cleartext (the envelope path refuses the
                // same leak); the entry is skipped entirely so neither
                // the selector nor a hash of it lands on disk.
                let selector_material = effective_profile
                    .as_deref()
                    .is_some_and(|text| {
                        profile.is_material_text(text) || profile.redact_text(text).is_some()
                    })
                    || active_core::envelope::document_schema_version(&module)
                        .as_deref()
                        .is_some_and(|text| {
                            profile.is_material_text(text) || profile.redact_text(text).is_some()
                        });
                if selector_material {
                    eprintln!(
                        "warning: cache entry not written: profile or schema selector is material"
                    );
                } else {
                    let receipt = cache_receipt(
                        &module,
                        effective_profile.as_deref(),
                        &engine.budget,
                        &value,
                        sandbox.as_deref().unwrap_or("agent"),
                        &diags,
                        resilient,
                    );
                    // Count every serialized part, not a guess: long
                    // diagnostic messages or a large receipt must not
                    // reach the clones or the writer.
                    let extra = serde_json::to_string(&diags)
                        .map(|text| text.len())
                        .unwrap_or(usize::MAX)
                        .saturating_add(
                            serde_json::to_string(&receipt)
                                .map(|text| text.len())
                                .unwrap_or(usize::MAX),
                        )
                        .saturating_add(match toml_render.as_ref() {
                            Some(Ok((_, export_diags))) => serde_json::to_string(export_diags)
                                .map(|text| text.len())
                                .unwrap_or(usize::MAX),
                            _ => 0,
                        })
                        .saturating_add(match yaml_render.as_ref() {
                            Some(Err(e)) => e.len(),
                            _ => 0,
                        })
                        .saturating_add(match toml_render.as_ref() {
                            Some(Err(e)) => e.len(),
                            _ => 0,
                        })
                        .saturating_add(256) as u64;
                    if !cache_json_fits
                        || !active_core::cache::entry_fits(
                            json_text.as_deref().unwrap_or(""),
                            yaml_text,
                            toml_text,
                            extra,
                        )
                    {
                        eprintln!("warning: result exceeds the cache entry limit; not cached");
                    } else {
                        let (yaml, yaml_error) = match yaml_render.as_ref() {
                            Some(Ok(text)) => (Some(text.clone()), None),
                            Some(Err(e)) => (None, Some(e.clone())),
                            None => (None, None),
                        };
                        let (toml, toml_error, export_diagnostics) = match toml_render.as_ref() {
                            Some(Ok((text, export_diags))) => {
                                (Some(text.clone()), None, export_diags.clone())
                            }
                            Some(Err(e)) => (None, Some(e.clone()), Vec::new()),
                            None => (None, None, Vec::new()),
                        };
                        let entry = active_core::cache::CacheEntry {
                            version: active_core::cache::CACHE_VERSION,
                            key: key.clone(),
                            json: json_text.clone().unwrap_or_default(),
                            yaml,
                            yaml_error,
                            toml,
                            toml_error,
                            export_diagnostics,
                            diagnostics: diags.clone(),
                            receipt,
                        };
                        // Best-effort: a cache that cannot write never fails
                        // an evaluation that already succeeded.
                        if let Err(e) = active_core::cache::EvalCache::new(dir)
                            .with_owner(cache_owner)
                            .put(&entry)
                        {
                            eprintln!("warning: cannot write cache entry: {e}");
                        }
                    }
                }
            }
            if let Some(audit_path) = audit_trail {
                // Canonical audit form: when the document builds blobs
                // and the run did not materialise them, hash a
                // materialised view so a payload-resident secret is
                // detected. Payloads are stripped afterwards, so the
                // result does not depend on `--materialise-blobs` and
                // replay can reproduce it with one fixed policy.
                let canonical = if materialise_blobs || !module_has_blobs(&module) {
                    full_value.clone()
                } else {
                    let mut canonical_engine = engine.clone();
                    canonical_engine.materialise_blobs = true;
                    canonical_engine.taint_probe = true;
                    // A second pass must not fail on the caller's wall
                    // clock after the first succeeded; counts and sizes
                    // still bind.
                    canonical_engine.budget =
                        active_core::eval::canonical_budget(&canonical_engine.budget);
                    let (canonical_value, _, canonical_seeds) = canonical_engine
                        .evaluate_with_seeds(&module)
                        .map_err(|e| anyhow::anyhow!("audit evaluation failed: {e}"))?;
                    let canonical_profile = complete_profile(
                        active_core::secret::SecretProfile::for_engine(
                            &module,
                            &canonical_engine,
                            &canonical_seeds,
                        ),
                    )?;
                    active_core::convert::redact_value(&canonical_profile, &canonical_value)
                };
                let audited = strip_blob_payloads(&canonical);
                write_audit_trail(
                    &audit_path,
                    &file,
                    &module,
                    Some(&audited),
                    &diags,
                    resilient,
                    effective_profile.as_deref(),
                    &engine.budget,
                    allow_parent_escapes,
                    materialise_blobs,
                    None,
                    peer.as_deref(),
                )?;
            }
            if had_failure(&diags, resilient) {
                std::process::exit(1);
            }
        }
        Commands::Check {
            file,
            resilient,
            allow_parent_escapes,
            trust_keys,
            profile,
            trace,
            budget,
            reader_version,
        } => {
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            match parse_input(&file, resilient, allow_parent_escapes, &trust) {
                Ok(module) => {
                    let engine = engine(resilient, &profile, budget.resolve()?, reader_version.as_deref());
                    active_core::trace::set_tracing(trace);
                    match engine.evaluate(&module) {
                        Ok((_, diags)) => {
                            dump_trail(trace);
                            if diags.is_empty() {
                                outln!("OK");
                            } else {
                                for d in &diags {
                                    report(d, &name);
                                }
                            }
                            if had_failure(&diags, resilient) {
                                std::process::exit(1);
                            }
                        }
                        Err(e) => {
                            dump_trail(trace);
                            report_eval_error(&e, &name);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Graph {
            file,
            format,
            command,
            resilient,
            allow_parent_escapes,
            trust_keys,
            profile,
        } => {
            if let Some(GraphQuery::Query {
                from: sub_from,
                query,
                key,
                min_size,
                format: qformat,
                profile: qprofile,
            }) = command
            {
                let resolved = sub_from.or(file).ok_or_else(|| {
                    anyhow::anyhow!("graph query needs a file: pass FILE or --from FILE")
                })?;
                // An explicit inner --format wins; otherwise the outer
                // --format applies. Trust/include/resilience/profile
                // pass through so the query path cannot bypass the
                // same gates as `active graph`.
                let effective_format = qformat.unwrap_or_else(|| format.clone());
                // The inner flag wins when both are given; the outer
                // form keeps working for scripts written before the
                // documented `graph query … --profile` grammar.
                let selected_profile = qprofile.or(profile);
                return run_graph_query(
                    &resolved,
                    &query,
                    key.as_deref(),
                    min_size,
                    &effective_format,
                    resilient,
                    allow_parent_escapes,
                    &trust_keys,
                    &selected_profile,
                );
            }
            let file = file.ok_or_else(|| {
                anyhow::anyhow!("graph needs a FILE (or use `graph query --from FILE`)")
            })?;
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&file, resilient, allow_parent_escapes, &trust)?;
            for d in &module.healing {
                report(d, &name);
            }

            match format.as_str() {
                "text" => {
                    outln!("Bindings:");
                    let mut bindings: Vec<(&String, &active_core::NodeId)> =
                        module.bindings.iter().collect();
                    bindings.sort_by(|a, b| a.0.cmp(b.0));
                    for (name, id) in bindings {
                        outln!("  let {} = NodeId({})", name, id.0);
                    }
                    outln!("\nGraph edges (dependency → dependent):");
                    for edge in module.graph.raw_edges() {
                        let from = module.graph[edge.source()];
                        let to = module.graph[edge.target()];
                        outln!(
                            "  {} → {}",
                            active_core::graph::node_label(&module, from),
                            active_core::graph::node_label(&module, to)
                        );
                    }
                }
                "mermaid" => outln!("{}", active_core::graph::to_mermaid(&module)),
                "dot" => outln!("{}", active_core::graph::to_dot(&module)),
                "json" => outln!(
                    "{}",
                    serde_json::to_string_pretty(&active_core::graph::to_graph_json(&module))
                        .map_err(|e| anyhow::anyhow!("graph serialize failed: {e}"))?
                ),
                other => anyhow::bail!("unsupported format: {}", other),
            }
            if had_failure(&module.healing, resilient) {
                std::process::exit(1);
            }
        }
        Commands::Impact {
            key,
            file,
            format,
            resilient,
            allow_parent_escapes,
            trust_keys,
        } => {
            let trust = load_trust(&trust_keys)?;
            let (module, filename) =
                parse_graph_input(&file, resilient, allow_parent_escapes, &trust)?;
            let id = active_core::graph::resolve_key(&module, &key)
                .ok_or_else(|| anyhow::anyhow!("unknown key '{key}' in {filename}"))?;
            let impact = active_core::graph::impact_paths_detailed(&module, id);
            // LANG-G-016: an empty answer caused by the container
            // exclusion says so instead of reading as "no dependents".
            let note = (impact.paths.is_empty() && impact.table_dependents_excluded)
                .then_some("no non-table dependents (table dependents excluded by design)");
            match format.as_str() {
                "text" => {
                    for path in &impact.paths {
                        outln!("{path}");
                    }
                    if let Some(note) = note {
                        outln!("note: {note}");
                    }
                }
                "json" => {
                    let mut out = serde_json::json!({"key": key, "paths": impact.paths});
                    if let Some(note) = note {
                        out["note"] = serde_json::json!(note);
                    }
                    outln!("{out}");
                }
                other => anyhow::bail!("unsupported format: {}", other),
            }
        }
        Commands::Why {
            key,
            file,
            format,
            resilient,
            allow_parent_escapes,
            trust_keys,
        } => {
            let trust = load_trust(&trust_keys)?;
            let (module, filename) =
                parse_graph_input(&file, resilient, allow_parent_escapes, &trust)?;
            let id = active_core::graph::resolve_key(&module, &key)
                .ok_or_else(|| anyhow::anyhow!("unknown key '{key}' in {filename}"))?;
            let paths = active_core::graph::why_paths(&module, id);
            match format.as_str() {
                "text" => {
                    for path in paths {
                        outln!("{path}");
                    }
                }
                "json" => outln!("{}", serde_json::json!({"key": key, "paths": paths})),
                other => anyhow::bail!("unsupported format: {}", other),
            }
        }
        Commands::Convert {
            file,
            to,
            resilient,
            allow_parent_escapes,
            trust_keys,
            profile,
            budget,
            reader_version,
        } => {
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&file, resilient, allow_parent_escapes, &trust)?;
            let engine = engine(resilient, &profile, budget.resolve()?, reader_version.as_deref());
            let (value, diags, seeds) = engine
                .evaluate_with_seeds(&module)
                .inspect_err(|e| report_eval_error(e, &name))?;
            for d in &diags {
                report(d, &name);
            }
            let profile = complete_profile(active_core::secret::SecretProfile::for_engine(
                &module, &engine, &seeds,
            ))?;
            match to.as_str() {
                "json" => outln!(
                    "{}",
                    active_core::convert::value_to_pretty_json_with(&profile, &value)
                ),
                "yaml" => match active_core::convert::value_to_yaml_string_with(&profile, &value) {
                    Ok(yaml) => outln!("{yaml}"),
                    Err(e) => anyhow::bail!("yaml export failed: {e}"),
                },
                "toml" => match active_core::convert::value_to_toml_string_with(&profile, &value) {
                    Ok((toml, export_diags)) => {
                        for d in &export_diags {
                            report(d, &name);
                        }
                        outln!("{toml}");
                    }
                    Err(e) => anyhow::bail!("toml export failed: {e}"),
                },
                other => anyhow::bail!("unsupported target: {}", other),
            }
            if had_failure(&diags, resilient) {
                std::process::exit(1);
            }
        }
        Commands::Import { file, from } => {
            let name = file.to_string_lossy().to_string();
            let text = std::fs::read_to_string(&file)
                .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", name))?;
            // Hard syntax failures exit non-zero; lossy corners are
            // diagnosed to stderr, best-effort, exit 0 (spec §5).
            let (source, notes) = match from.as_str() {
                "json" => json_text_to_active(&text).map_err(|e| anyhow::anyhow!("{e}"))?,
                "yaml" | "yml" => yaml_text_to_active(&text).map_err(|e| anyhow::anyhow!("{e}"))?,
                "toml" => toml_text_to_active(&text).map_err(|e| anyhow::anyhow!("{e}"))?,
                "jsonschema" => jsonschema_to_active(&text).map_err(|e| anyhow::anyhow!("{e}"))?,
                other => anyhow::bail!("unsupported source: {}", other),
            };
            for d in &notes {
                report(d, &name);
            }
            out!("{source}");
        }
        Commands::Generate {
            from,
            to,
            name,
            trust_keys,
        } => {
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&from, false, false, &trust)?;
            // Generators see the same authored + process-env material
            // set as planning, so a schema literal equal to a host
            // value is withheld instead of emitted.
            let profile = material_profile(&module)?;
            // Documents without a schema block exit non-zero (spec §5).
            let output = match to.as_str() {
                "jsonschema" | "json-schema" => {
                    active_core::convert::schema_to_json_schema_with_profile(&module, &profile)
                        .and_then(|schema| {
                            serde_json::to_string_pretty(&schema)
                                .map_err(|e| format!("generate serialize failed: {e}"))
                        })
                }
                "openapi" => {
                    active_core::convert::schema_to_openapi_with_profile(&module, &name, &profile)
                        .and_then(|schema| {
                            serde_json::to_string_pretty(&schema)
                                .map_err(|e| format!("generate serialize failed: {e}"))
                        })
                }
                "typescript" | "ts" => active_core::convert::schema_to_typescript_with_profile(
                    &module, &name, &profile,
                ),
                "go" => active_core::convert::schema_to_go_with_profile(&module, &name, &profile),
                "rust" | "serde" => {
                    active_core::convert::schema_to_rust_with_profile(&module, &name, &profile)
                }
                "python" | "py" => {
                    active_core::convert::schema_to_python_with_profile(&module, &name, &profile)
                }
                other => anyhow::bail!(
                    "unsupported target: {other} (want jsonschema|json-schema, \
                     openapi, typescript|ts, go, rust|serde, python|py)"
                ),
            }
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            // Emit exactly one trailing newline: generator outputs
            // already end with `\n`, and `outln!` would append a
            // second one (a blank line that `gofmt` rejects).
            outln!("{}", output.trim_end_matches('\n'));
        }
        Commands::Inspect {
            from,
            path,
            format,
            trust_keys,
            profile,
        } => {
            run_inspect(&from, path.as_deref(), &format, &trust_keys, &profile)?;
        }
        Commands::Schema { action } => {
            let SchemaAction::Inspect {
                from,
                path,
                format,
                trust_keys,
                profile,
            } = action;
            run_inspect(&from, path.as_deref(), &format, &trust_keys, &profile)?;
        }
        Commands::Mode { from, format } => {
            let module = parse_input(&from, false, false, &None)?;
            let marker = module.mode_marker;
            let mode = match module.mode {
                active_core::Mode::Data => "data",
                active_core::Mode::Config => "config",
            };
            if format == "text" {
                outln!("mode: {mode}");
                outln!("marker: {}", if marker { "present" } else { "absent" });
                if module.capabilities.is_empty() {
                    outln!("capabilities: (none)");
                } else {
                    outln!("capabilities: {}", module.capabilities.join(", "));
                }
            } else if format == "json" {
                outln!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "mode": mode,
                        "marker": marker,
                        "capabilities": module.capabilities,
                    }))
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                );
            } else {
                anyhow::bail!("unknown mode format `{format}` (want json|text)");
            }
        }
        Commands::Budget { action } => {
            let BudgetAction::Explain {
                from,
                format,
                profile,
            } = action;
            let module = parse_input(&from, false, false, &None)?;
            let mut evaluation_engine = Engine::new();
            evaluation_engine.profile_override = profile.clone();
            let (value, diagnostics, seeds) = evaluation_engine
                .evaluate_with_seeds(&module)
                .map_err(|e| anyhow::anyhow!("cannot evaluate: {e}"))?;
            let data = active_core::viz::budget_data_with_seeds(
                &module,
                &value,
                &evaluation_engine.budget,
                &diagnostics,
                profile.as_deref(),
                Some(&evaluation_engine.env),
                Some(&seeds),
            );
            if format == "text" {
                let limits = &data["limits"];
                let usage = &data["usage"];
                let headroom = &data["headroom"];
                for (label, key) in [
                    ("nodes", "nodes"),
                    ("depth", "depth"),
                    ("array length", "array_len"),
                    ("keys", "keys"),
                    ("value size", "value_size"),
                    ("total size", "total_size"),
                ] {
                    outln!(
                        "{label}: used {} of {} (headroom {})",
                        usage[key], limits[key], headroom[key]
                    );
                }
                outln!(
                    "wall clock: {} ms (host-time limit; not sampled here)",
                    limits["wall_ms"]
                );
            } else if format == "json" {
                outln!(
                    "{}",
                    serde_json::to_string_pretty(&data).map_err(|e| anyhow::anyhow!("{e}"))?
                );
            } else {
                anyhow::bail!("unknown budget format `{format}` (want json|text)");
            }
        }
        Commands::Debug {
            from,
            break_at,
            replay,
            expect,
            minimize,
            max_attempts,
            format,
            profile,
            resilient,
            allow_parent_escapes,
            budget,
            trust_keys,
            reader_version: _,
        } => {
            if minimize.is_some() && trust_keys.is_some() {
                anyhow::bail!(
                    "--trust-keys is not supported with --minimize (signed-include reduction)"
                );
            }
            let trust = load_trust(&trust_keys)?;
            if format != "json" && format != "text" {
                anyhow::bail!("unknown debug format `{format}` (want json|text)");
            }
            // Reduction works on raw source: a document with parse
            // errors is exactly the case it exists for, so handle it
            // before parsing the module.
            if let Some(code) = minimize {
                let source = std::fs::read_to_string(&from)
                    .map_err(|e| anyhow::anyhow!("cannot read source: {e}"))?;
                let resolved_budget = budget.resolve()?;
                let report = active_core::debug::minimal_case_with_options(
                    &source,
                    &code,
                    max_attempts.unwrap_or(200),
                    Some(&from),
                    active_core::debug::DebugOptions {
                        resilient,
                        allow_parent_escapes,
                        profile: profile.as_deref(),
                        budget: Some(&resolved_budget),
                    },
                );
                if format == "text" {
                    outln!(
                        "reduced {} -> {} bytes ({} attempts) keeping {}",
                        report["original_bytes"], report["reduced_bytes"], report["attempts"], code
                    );
                    out!("{}", report["reduced"].as_str().unwrap_or(""));
                } else {
                    outln!(
                        "{}",
                        serde_json::to_string_pretty(&report)
                            .map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
                if !report["target_present"].as_bool().unwrap_or(false) {
                    if format == "text" {
                        outln!("target {code} not found in the document");
                    }
                    std::process::exit(1);
                }
                return Ok(());
            }
            let module = parse_input(&from, resilient, allow_parent_escapes, &trust)?;
            if replay {
                let report = active_core::debug::replay_with_budget(
                    &module,
                    expect.as_deref(),
                    profile.as_deref(),
                    &budget.resolve()?,
                    resilient,
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                if format == "text" {
                    outln!(
                        "replay: {} hash={} steps={} steps_match={}",
                        if report["ok"].as_bool().unwrap_or(false) {
                            "OK"
                        } else {
                            "MISMATCH"
                        },
                        report["hash"].as_str().unwrap_or("?"),
                        report["steps"].as_u64().unwrap_or(0),
                        report["steps_match"].as_bool().unwrap_or(false)
                    );
                } else {
                    outln!(
                        "{}",
                        serde_json::to_string_pretty(&report)
                            .map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
                if !report["ok"].as_bool().unwrap_or(false) {
                    std::process::exit(1);
                }
                return Ok(());
            }
            let mut step_engine = Engine::new();
            step_engine.profile_override = profile.clone();
            step_engine.resilient = resilient;
            step_engine.budget = budget.resolve()?;
            let (_, _, steps) = step_engine
                .evaluate_with_steps(&module)
                .map_err(|e| anyhow::anyhow!("cannot evaluate for stepping: {e}"))?;
            let (shown, matched) = match break_at.as_deref() {
                Some(selector) => match active_core::debug::breakpoint_index_with_budget(
                    &steps,
                    selector,
                    step_engine.budget.max_nodes,
                ) {
                    Some(index) => (steps[..=index].to_vec(), Some(index)),
                    None => (Vec::new(), None),
                },
                None => (steps.clone(), None),
            };
            if format == "text" {
                out!("{}", active_core::debug::render_steps(&shown));
                if let Some(selector) = &break_at {
                    match matched {
                        Some(index) => outln!("break at #{index} ({selector})"),
                        None => outln!("breakpoint not reached ({selector})"),
                    }
                }
            } else {
                let report = serde_json::json!({
                    "breakpoint": break_at,
                    "matched": matched,
                    "total_steps": steps.len(),
                    "steps": shown,
                });
                outln!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|e| anyhow::anyhow!("{e}"))?
                );
            }
            if break_at.is_some() && matched.is_none() {
                std::process::exit(1);
            }
        }
        Commands::Viz {
            kind,
            from,
            format,
            goal,
            preserve,
            max_steps,
            allow_parent_escapes,
            trust_keys,
            profile,
        } => {
            let trust = load_trust(&trust_keys)?;
            // Reject flags that do not apply to this kind instead of
            // accepting and silently ignoring them.
            if kind != "graph" && format != "json" {
                anyhow::bail!("--format is only meaningful for `viz graph` (want json)");
            }
            if kind != "repair" && (goal != "validate" || !preserve.is_empty() || max_steps != 8) {
                anyhow::bail!("--goal/--preserve/--max-steps only apply to `viz repair`");
            }
            if profile.is_some() && kind != "taint" && kind != "budget" {
                anyhow::bail!("--profile only applies to `viz taint` and `viz budget`");
            }
            if kind == "repair" {
                if max_steps > 64 {
                    anyhow::bail!(
                        "--max-steps capped at 64 (bounded loop; larger plans split across runs)"
                    );
                }
                let parsed_goal = match goal.as_str() {
                    "validate" => active_core::diag::RepairGoal::Validate,
                    "preserve-key" => {
                        if preserve.is_empty() {
                            anyhow::bail!("preserve-key goal needs at least one --preserve <path>");
                        }
                        active_core::diag::RepairGoal::PreserveKey(preserve.clone())
                    }
                    "minimize-diff" => active_core::diag::RepairGoal::MinimizeDiff,
                    other => anyhow::bail!(
                        "unknown repair goal `{other}` (want validate|preserve-key|minimize-diff)"
                    ),
                };
                // Repair plans raw source with the file context, so an
                // unparseable document still yields the H008 outcome and
                // includes resolve exactly as `active repair` resolves
                // them (spec/viz.md §3).
                if trust.is_some() {
                    parse_input_unpinned(&from, false, allow_parent_escapes, &trust, None)?;
                }
                let source = std::fs::read_to_string(&from)
                    .map_err(|e| anyhow::anyhow!("cannot read source: {e}"))?;
                let mut engine = Engine::new();
                engine.resilient = false;
                let outcome = active_core::convert::repair::repair_with_engine_and_options(
                    &source,
                    parsed_goal,
                    &preserve,
                    max_steps,
                    &engine,
                    Some(&from),
                    allow_parent_escapes,
                );
                // The plan is display data: strip the source, which can
                // contain an authored secret literal (spec/viz.md §3).
                // Directives stay verbatim: planning already withholds a
                // fill that would copy material, and redacting the path
                // would make the plan stop applying.
                let mut data =
                    serde_json::to_value(&outcome).map_err(|e| anyhow::anyhow!("{e}"))?;
                if let Some(object) = data.as_object_mut() {
                    object.remove("repaired_source");
                }
                outln!(
                    "{}",
                    serde_json::to_string_pretty(&data).map_err(|e| anyhow::anyhow!("{e}"))?
                );
                return Ok(());
            }
            let module = parse_input(&from, false, allow_parent_escapes, &trust)?;
            match kind.as_str() {
                "graph" => match format.as_str() {
                    "json" => outln!(
                        "{}",
                        serde_json::to_string_pretty(&active_core::graph::to_graph_json(&module))
                            .map_err(|e| anyhow::anyhow!("{e}"))?
                    ),
                    "mermaid" => out!("{}", active_core::graph::to_mermaid(&module)),
                    "dot" => out!("{}", active_core::graph::to_dot(&module)),
                    other => {
                        anyhow::bail!("unknown graph format `{other}` (want json|mermaid|dot)")
                    }
                },
                "taint" => {
                    let mut evaluation_engine = Engine::new();
                    evaluation_engine.profile_override = profile.clone();
                    // Taint queries need payload bytes to see a
                    // payload-resident secret; only paths are emitted.
                    evaluation_engine.materialise_blobs = true;
                    evaluation_engine.taint_probe = true;
                    let (value, _, seeds) = evaluation_engine
                        .evaluate_with_seeds(&module)
                        .map_err(|e| anyhow::anyhow!("cannot evaluate: {e}"))?;
                    outln!(
                        "{}",
                        serde_json::to_string_pretty(&active_core::viz::taint_data_with_seeds(
                            &module,
                            &value,
                            profile.as_deref(),
                            Some(&evaluation_engine.env),
                            Some(&seeds),
                        ))
                        .map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
                "budget" => {
                    let mut evaluation_engine = Engine::new();
                    evaluation_engine.profile_override = profile.clone();
                    let (value, diagnostics, seeds) = evaluation_engine
                        .evaluate_with_seeds(&module)
                        .map_err(|e| anyhow::anyhow!("cannot evaluate: {e}"))?;
                    outln!(
                        "{}",
                        serde_json::to_string_pretty(&active_core::viz::budget_data_with_seeds(
                            &module,
                            &value,
                            &evaluation_engine.budget,
                            &diagnostics,
                            profile.as_deref(),
                            Some(&evaluation_engine.env),
                            Some(&seeds),
                        ))
                        .map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
                "schema" => {
                    let mut data = active_core::viz::schema_data(&module)
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    // Schema prose can equal a host value.
                    material_profile(&module)?.redact_json(&mut data);
                    outln!(
                        "{}",
                        serde_json::to_string_pretty(&data).map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
                "repair" => unreachable!("repair handled before parsing"),
                other => anyhow::bail!(
                    "unknown viz kind `{other}` (want graph|taint|budget|schema|repair)"
                ),
            }
        }
        Commands::Explain {
            from,
            format,
            allow_parent_escapes,
            trust_keys,
            budget,
            reader_version: _,
        } => {
            let filename = from.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&from, false, allow_parent_escapes, &trust)?;
            let (diags, hard_error) = collect_diags(&module, &filename, budget.resolve()?);
            match format {
                OutFormat::Text => {
                    for line in explain_lines(&diags) {
                        outln!("{line}");
                    }
                }
                OutFormat::Json => {
                    let items = explain_items(&diags);
                    let structured = active_core::convert::explain_structured(&items);
                    outln!(
                        "{}",
                        serde_json::to_string_pretty(&structured)
                            .map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
            }
            if hard_error {
                std::process::exit(2);
            }
        }
        Commands::Patch {
            from,
            format,
            allow_parent_escapes,
            trust_keys,
            budget,
            reader_version: _,
        } => {
            let filename = from.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&from, false, allow_parent_escapes, &trust)?;
            let (diags, hard_error) = collect_diags(&module, &filename, budget.resolve()?);
            if hard_error {
                // LANG-G-005: `active patch` still fails closed on a hard
                // error (nothing to plan); the code and message are on
                // stderr via the same diagnostic shape.
                for d in &diags {
                    report(d, &filename);
                }
                std::process::exit(2);
            }
            // Planning with the material profile: a default that copies
            // an authored or environment secret yields no directive.
            let profile = material_profile(&module)?;
            let structured = active_core::convert::suggest_patches_structured_with_profile(
                &module, &diags, &profile,
            );
            match format {
                OutFormat::Text => {
                    for suggestion in &structured {
                        outln!("{}", suggestion.directive);
                    }
                }
                OutFormat::Json => {
                    outln!(
                        "{}",
                        serde_json::to_string_pretty(&structured)
                            .map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
            }
        }
        Commands::Capability {
            require_major,
            check_doc,
            profile,
        } => {
            // The descriptor is embedded at build time and must parse:
            // a corrupt descriptor is a build bug, so fail closed
            // instead of printing an empty success.
            let descriptor: serde_json::Value = serde_json::from_str(CAPABILITY_JSON)
                .map_err(|e| anyhow::anyhow!("capability descriptor corrupt: {e}"))?;
            if let Some(want) = require_major {
                let version = descriptor
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let major: u64 = version
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(u64::MAX);
                if major != want {
                    anyhow::bail!(
                        "capability major mismatch: descriptor is {version}, agent requires {want}"
                    );
                }
            }
            let want_profile = profile.is_some();
            if let Some(name) = profile {
                match active_core::convert::sandbox_profile(&name) {
                    Some(p) => outln!(
                        "{}",
                        serde_json::json!({
                            "profile": p.name,
                            "budget": p.budget,
                            "tools": p.tools,
                        })
                    ),
                    None => anyhow::bail!(
                        "unknown sandbox profile: {name} (want strict|data|agent|trusted)"
                    ),
                }
            }
            if let Some(doc) = check_doc {
                return check_declared_against(&doc, &descriptor);
            }
            if want_profile {
                return Ok(());
            }
            outln!(
                "{}",
                serde_json::to_string_pretty(&descriptor)
                    .map_err(|e| anyhow::anyhow!("capability serialize failed: {e}"))?
            );
        }
        Commands::Compatibility { old, new, format } => {
            let old_module = parse_input(&old, false, false, &None)?;
            let new_module = parse_input(&new, false, false, &None)?;
            let mut findings = active_core::convert::compare_schemas(&old_module, &new_module)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            // Findings are value-free by contract. `compare_schemas`
            // redacts with the document selector; an env-resolved value
            // copied into an enum member or media type is material only
            // when the process environment is consulted, so sweep the
            // messages again with both documents' material profiles.
            let old_profile = material_profile(&old_module)?;
            let new_profile = material_profile(&new_module)?;
            for finding in &mut findings {
                if let Some(text) = old_profile.redact_text(&finding.message) {
                    finding.message = text;
                }
                if let Some(text) = new_profile.redact_text(&finding.message) {
                    finding.message = text;
                }
            }
            match format.as_str() {
                "json" => outln!(
                    "{}",
                    serde_json::to_string_pretty(&findings)
                        .map_err(|e| anyhow::anyhow!("compatibility serialize failed: {e}"))?
                ),
                "text" => {
                    if findings.is_empty() {
                        outln!("compatible: no schema changes");
                    }
                    for finding in &findings {
                        outln!(
                            "[{}] {} {}: {}",
                            finding.severity, finding.code, finding.path, finding.message
                        );
                    }
                }
                other => anyhow::bail!("unsupported format: {other}"),
            }
            if findings.iter().any(|finding| finding.is_breaking()) {
                anyhow::bail!("E040: breaking schema changes detected");
            }
        }
        Commands::Repair {
            from,
            goal,
            preserve,
            max_steps,
            format,
            output,
            apply_step,
            cherry_pick,
            browser,
            filter_code,
            filter_severity,
            filter_path,
            allow_parent_escapes,
            trust_keys,
            profile,
            budget,
            reader_version,
        } => {
            use active_core::diag::RepairGoal;
            if max_steps > 64 {
                anyhow::bail!(
                    "--max-steps capped at 64 (bounded loop; larger plans split across runs)"
                );
            }
            let goal_parsed = match goal.as_str() {
                "validate" => RepairGoal::Validate,
                "minimize-diff" => RepairGoal::MinimizeDiff,
                "preserve-key" => {
                    if preserve.is_empty() {
                        anyhow::bail!("preserve-key goal needs at least one --preserve <path>");
                    }
                    RepairGoal::PreserveKey(preserve.clone())
                }
                other => anyhow::bail!(
                    "unknown repair goal: {other} (want validate|preserve-key|minimize-diff)"
                ),
            };
            let source = std::fs::read_to_string(&from)
                .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", from.display()))?;
            let trust = load_trust(&trust_keys)?;
            if trust.is_some() {
                // Gate the original file through trust verification
                // before planning: untrusted includes fail here, the
                // same as every other tool path.
                parse_input_unpinned(&from, false, allow_parent_escapes, &trust, None)?;
            }
            let mut eng = engine(false, &profile, budget.resolve()?, reader_version.as_deref());
            eng.materialise_blobs = false;
            if browser {
                if apply_step.is_some() || cherry_pick.is_some() || output.is_some() {
                    anyhow::bail!(
                        "--browser emits the JSON model and cannot be combined with --apply-step, --cherry-pick, or --output"
                    );
                }
                if format != "text" && format != "json" {
                    anyhow::bail!("--browser always emits JSON (--format json|text)");
                }
                let mut model = active_core::convert::repair::browser_model_with(
                    &source,
                    goal_parsed.clone(),
                    &preserve,
                    max_steps,
                    &eng,
                    Some(&from),
                    allow_parent_escapes,
                );
                // Browser filters select the diagnostics shown. The plan
                // stays whole: filtering steps would change what
                // `--apply-step` means, and the browser must never
                // disagree with the shipped plan.
                for severity in &filter_severity {
                    let known = ["error", "warning", "info", "hint"];
                    if !known.contains(&severity.as_str()) {
                        anyhow::bail!(
                            "unknown --filter-severity `{severity}` (want error|warning|info|hint)"
                        );
                    }
                }
                if !filter_code.is_empty() || !filter_severity.is_empty() || !filter_path.is_empty()
                {
                    if let Some(items) = model["diagnostics"].as_array_mut() {
                        items.retain(|item| {
                            let code = item["code"].as_str().unwrap_or_default();
                            let severity = item["severity"].as_str().unwrap_or_default();
                            let path = item["path"].as_str().unwrap_or_default();
                            (filter_code.is_empty() || filter_code.iter().any(|c| c == code))
                                && (filter_severity.is_empty()
                                    || filter_severity.iter().any(|s| s == severity))
                                && (filter_path.is_empty()
                                    || filter_path.iter().any(|p| path.starts_with(p)))
                        });
                    }
                }
                outln!(
                    "{}",
                    serde_json::to_string_pretty(&model)
                        .map_err(|e| anyhow::anyhow!("browser serialize failed: {e}"))?
                );
                return Ok(());
            }
            let outcome = active_core::convert::repair::repair_with_engine_and_options(
                &source,
                goal_parsed,
                &preserve,
                max_steps,
                &eng,
                Some(&from),
                allow_parent_escapes,
            );
            // Partial application (REPAIR-UI.003): an explicit subset
            // applies only safe steps inside the index bound and/or
            // matching the path prefix, in plan order.
            let partial = apply_step.is_some() || cherry_pick.is_some();
            let (target_source, applied_directives) = if partial {
                active_core::convert::repair::apply_selected(
                    &source,
                    &outcome.steps,
                    apply_step,
                    cherry_pick.as_deref(),
                )
            } else {
                (
                    outcome.repaired_source.clone(),
                    outcome
                        .steps
                        .iter()
                        .filter(|step| {
                            matches!(
                                step.status,
                                active_core::convert::repair::StepStatus::Applied
                            )
                        })
                        .map(|step| step.directive.clone())
                        .collect(),
                )
            };
            // PreserveKey carries its paths inside the goal for the
            // plan record; the loop itself used the raw list.
            // A partial apply reports the APPLIED subset: unselected
            // steps are skipped and the source/hash/verdict describe
            // the document that was actually written.
            let mut display = outcome.clone();
            if partial {
                let selected: std::collections::HashSet<&str> =
                    applied_directives.iter().map(String::as_str).collect();
                display.repaired_source = target_source.clone();
                for step in &mut display.steps {
                    if matches!(
                        step.status,
                        active_core::convert::repair::StepStatus::Applied
                    ) && !selected.contains(step.directive.as_str())
                    {
                        step.status = active_core::convert::repair::StepStatus::Skipped {
                            reason: "not selected by --apply-step/--cherry-pick".to_string(),
                        };
                    }
                }
                let (clean, codes) = source_verdict(&eng, &target_source, &from);
                display.clean = clean;
                display.final_codes = codes;
            }
            match format.as_str() {
                "json" => outln!(
                    "{}",
                    serde_json::to_string_pretty(&display)
                        .map_err(|e| anyhow::anyhow!("repair serialize failed: {e}"))?
                ),
                "text" => {
                    if display.clean {
                        outln!("repair: clean");
                    }
                    for step in &display.steps {
                        match &step.status {
                            active_core::convert::repair::StepStatus::Applied => {
                                outln!("applied {} {}", step.kind, step.directive)
                            }
                            active_core::convert::repair::StepStatus::Skipped { reason } => {
                                outln!("skipped {} {} ({reason})", step.kind, step.directive)
                            }
                        }
                    }
                    for finding in &display.findings {
                        report(finding, &from.to_string_lossy());
                    }
                    if !display.clean {
                        outln!("repair: not clean ({})", display.final_codes.join(", "));
                    }
                    if partial {
                        outln!("selected: {} directives", applied_directives.len());
                        for directive in &applied_directives {
                            outln!("apply {directive}");
                        }
                    }
                }
                other => anyhow::bail!("unsupported format: {other}"),
            }
            if let Some(path) = output {
                if partial && !applied_directives.is_empty() {
                    std::fs::write(&path, &target_source)
                        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?;
                    if !outcome.clean {
                        eprintln!(
                            "warning: wrote a partial repair to {} (plan is not clean yet)",
                            path.display()
                        );
                    }
                } else if !partial && outcome.clean {
                    std::fs::write(&path, &target_source)
                        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?;
                } else {
                    eprintln!(
                        "warning: not writing {}: repair did not reach clean",
                        path.display()
                    );
                }
            }
            if partial {
                if applied_directives.is_empty() {
                    std::process::exit(1);
                }
            } else if !outcome.clean {
                std::process::exit(1);
            }
        }
        Commands::Doc {
            from,
            to,
            title,
            resilient,
            allow_parent_escapes,
            trust_keys,
            budget,
            reader_version,
        } => {
            let filename = from.to_string_lossy().to_string();
            let title = title.unwrap_or_else(|| {
                from.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Configuration".to_string())
            });
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&from, resilient, allow_parent_escapes, &trust)?;
            let engine = engine(resilient, &None, budget.resolve()?, reader_version.as_deref());
            let (value, diags, seeds) = engine
                .evaluate_with_seeds(&module)
                .map_err(|e| anyhow::anyhow!("evaluation failed: {e}"))?;
            for d in &diags {
                report(d, &filename);
            }
            let profile = complete_profile(active_core::secret::SecretProfile::for_engine(
                &module, &engine, &seeds,
            ))?;
            let items = collect_items_with_profile(
                &module,
                &active_core::convert::redact_value(&profile, &value),
                &profile,
            );
            match to.as_str() {
                "markdown" | "md" => outln!("{}", doc_to_markdown(&title, &items)),
                "html" => outln!("{}", doc_to_html(&title, &items)),
                "man" => outln!("{}", doc_to_man(&title, &items)),
                other => anyhow::bail!("unsupported target: {}", other),
            }
        }
        Commands::Lock {
            file,
            output,
            store,
            resilient,
            allow_parent_escapes,
            trust_keys,
            profile,
            budget,
            reader_version,
        } => {
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input_unpinned(&file, resilient, allow_parent_escapes, &trust, store)?;
            let engine = engine(resilient, &profile, budget.resolve()?, reader_version.as_deref());
            let (value, diags, seeds) = engine
                .evaluate_with_seeds(&module)
                .inspect_err(|e| report_eval_error(e, &name))?;
            for d in &diags {
                report(d, &name);
            }
            // The effective profile, when one is active: selection
            // changes values, so it joins the provenance (override
            // wins, else the document's own valid selector).
            let effective = profile.clone().or_else(|| {
                // Read the document's own literal selector BEFORE
                // patches: a patch must not be able to rename the
                // provenance profile.
                active_core::secret::active_profile_name(&module)
                    .filter(|name| module.profiles.contains_key(name))
            });
            // LANG-I-001: a re-lock reuses the salts stored in the
            // existing lockfile, so an unchanged document renders the same
            // bytes. stdout output has no prior file and gets a fresh salt.
            let previous_salts = output
                .as_ref()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .map(|text| active_core::provenance::env_salts_from_lock(&text));
            let provenance = active_core::provenance::collect_provenance_with_salts(
                &module,
                &engine.env,
                Some(file.as_path()),
                effective.as_deref(),
                Some(&seeds),
                previous_salts.as_ref(),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            let secret_profile = complete_profile(
                active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds),
            )?;
            // Canonical lock identity: same materialise → redact → strip
            // policy as the audit, so both hash the same document.
            let canonical = if module_has_blobs(&module) && !engine.materialise_blobs {
                let mut canonical_engine = engine.clone();
                canonical_engine.materialise_blobs = true;
                canonical_engine.taint_probe = true;
                canonical_engine.budget =
                    active_core::eval::canonical_budget(&canonical_engine.budget);
                let (canonical_value, _, canonical_seeds) = canonical_engine
                    .evaluate_with_seeds(&module)
                    .map_err(|e| anyhow::anyhow!("lock evaluation failed: {e}"))?;
                let canonical_profile = complete_profile(
                        active_core::secret::SecretProfile::for_engine(
                            &module,
                            &canonical_engine,
                            &canonical_seeds,
                        ),
                    )?;
                active_core::convert::redact_value(&canonical_profile, &canonical_value)
            } else {
                active_core::convert::redact_value(&secret_profile, &value)
            };
            let mut manifest = active_core::provenance::LockManifest::new(
                format!(
                    "active-cli {} / active-core {}",
                    env!("CARGO_PKG_VERSION"),
                    active_core::VERSION
                ),
                &strip_blob_payloads(&canonical),
                provenance,
            );
            // A lock written to a file records its inputs relative to
            // that file, so verification resolves them from the lock's
            // own directory (L-P03).
            if let Some(path) = output.as_ref() {
                let base = match path.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                    _ => std::path::PathBuf::from("."),
                };
                manifest.rebase_paths(&base);
            }
            let rendered = manifest.render();
            match output {
                Some(path) => std::fs::write(&path, &rendered)
                    .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?,
                None => out!("{rendered}"),
            }
            if had_failure(&diags, resilient) {
                std::process::exit(1);
            }
        }
        Commands::Test {
            paths,
            conformance,
            strict,
            env,
        } => {
            let mut values = std::collections::HashMap::new();
            for item in env {
                let (key, value) = item.split_once('=').ok_or_else(|| {
                    anyhow::anyhow!("--env expects KEY=VALUE, got `{item}`")
                })?;
                values.insert(key.to_string(), value.to_string());
            }
            let report = test_runner::run(&paths, conformance.as_deref(), strict, &values)?;
            for note in &report.notes {
                outln!("{note}");
            }
            for failure in &report.failures {
                eprintln!("FAIL: {failure}");
            }
            let file_failures = report.failures.len().saturating_sub(report.case_failures);
            outln!(
                "test: {} file(s), {} failure(s); {} conformance case(s), {} failure(s)",
                report.files,
                file_failures,
                report.cases,
                report.case_failures
            );
            if !report.failures.is_empty() {
                std::process::exit(1);
            }
        }
        Commands::Store { action } => {
            let resolve_root = |explicit: Option<PathBuf>| -> anyhow::Result<PathBuf> {
                explicit
                    .or_else(active_core::store::Store::default_root)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "no store directory: pass --store or set {}",
                            active_core::store::ENV_STORE_DIR
                        )
                    })
            };
            match action {
                StoreAction::Add { file, store } => {
                    let meta = std::fs::metadata(&file).map_err(|e| {
                        anyhow::anyhow!("cannot read {}: {e}", file.display())
                    })?;
                    if !meta.is_file() {
                        anyhow::bail!("{} is not a regular file", file.display());
                    }
                    if meta.len() > active_core::store::MAX_ENTRY_BYTES {
                        anyhow::bail!(
                            "{} is {} bytes; the store limit is {}",
                            file.display(),
                            meta.len(),
                            active_core::store::MAX_ENTRY_BYTES
                        );
                    }
                    let bytes = std::fs::read(&file)
                        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", file.display()))?;
                    let store = active_core::store::Store::new(resolve_root(store)?);
                    let hash = store.put(&bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
                    outln!("{hash}");
                }
                StoreAction::List { json, store } => {
                    let store = active_core::store::Store::new(resolve_root(store)?);
                    let hashes = store.hashes().map_err(|e| anyhow::anyhow!("{e}"))?;
                    if json {
                        outln!(
                            "{}",
                            serde_json::to_string_pretty(&hashes)
                                .map_err(|e| anyhow::anyhow!("{e}"))?
                        );
                    } else {
                        for hash in hashes {
                            outln!("{hash}");
                        }
                    }
                }
                StoreAction::Gc {
                    roots,
                    all,
                    dry_run,
                    store,
                } => {
                    let store = active_core::store::Store::new(resolve_root(store)?);
                    let mut keep = std::collections::HashSet::new();
                    if !all {
                        for path in roots {
                            let meta = std::fs::metadata(&path).map_err(|e| {
                                anyhow::anyhow!("cannot read root {}: {e}", path.display())
                            })?;
                            if !meta.is_file() {
                                anyhow::bail!("{} is not a regular file", path.display());
                            }
                            if meta.len() > active_core::store::MAX_ROOT_BYTES {
                                anyhow::bail!(
                                    "{} exceeds the {} byte root limit",
                                    path.display(),
                                    active_core::store::MAX_ROOT_BYTES
                                );
                            }
                            let text = std::fs::read_to_string(&path).map_err(|e| {
                                anyhow::anyhow!("cannot read root {}: {e}", path.display())
                            })?;
                            let found =
                                active_core::store::roots_from_lockfile(&text).map_err(|e| {
                                    anyhow::anyhow!("{}: {e}", path.display())
                                })?;
                            keep.extend(found);
                        }
                    }
                    let removed = store
                        .gc(&keep, dry_run)
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    for hash in &removed {
                        if dry_run {
                            outln!("would remove {hash}");
                        } else {
                            outln!("removed {hash}");
                        }
                    }
                    outln!(
                        "{} entr{} {}",
                        removed.len(),
                        if removed.len() == 1 { "y" } else { "ies" },
                        if dry_run { "would be removed" } else { "removed" }
                    );
                }
            }
        }
        Commands::Keygen { out } => {
            // Fresh Ed25519 seed from OS randomness. Both files are
            // created with their final mode in one open (LANG-H-002/H-901):
            // no write-then-chmod window, and the public half is not
            // umask-dependent.
            let key =
                active_core::sign::generate_secret_key().map_err(|e| anyhow::anyhow!("{e}"))?;
            let secret_path = out.with_extension("secret");
            let public_path = out.with_extension("public");
            write_key_file(
                &secret_path,
                &active_core::sign::secret_hex(&key),
                0o600,
            )?;
            let verifying = key.verifying_key();
            write_key_file(
                &public_path,
                &active_core::sign::public_hex(&verifying),
                0o644,
            )?;
            outln!(
                "wrote {} and {}",
                secret_path.display(),
                public_path.display()
            );
        }
        Commands::Sign {
            file,
            key,
            kid,
            identity,
            expires,
            force,
        } => {
            let label = file.to_string_lossy().to_string();
            let secret_text = std::fs::read_to_string(&key)
                .map_err(|e| anyhow::anyhow!("cannot read key {}: {e}", key.display()))?;
            let secret = active_core::sign::load_secret_key(&secret_text)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let bytes =
                std::fs::read(&file).map_err(|e| anyhow::anyhow!("cannot read {label}: {e}"))?;
            // LANG-I-002: the signature binds the identity path, so a
            // signed overlay copied elsewhere fails verification.
            let binding = active_core::sign::SignBinding {
                path: identity.as_deref().unwrap_or(&label),
                role: None,
                version: None,
                expires: expires.as_deref(),
            };
            let sidecar = active_core::sign::sign_bytes(&secret, &kid, &binding, &bytes)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let sidecar_path = active_core::sign::sidecar_path_for(&file);
            // LANG-I-017: a re-sign overwrites the only copy of the old
            // sidecar; require an explicit --force.
            if sidecar_path.exists() && !force {
                anyhow::bail!(
                    "{} already exists; pass --force to re-sign",
                    sidecar_path.display()
                );
            }
            std::fs::write(&sidecar_path, sidecar).map_err(|e| {
                anyhow::anyhow!(
                    "cannot write {}: {e}",
                    std::path::PathBuf::from(&sidecar_path).display()
                )
            })?;
            outln!("signed {label} for key '{kid}'");
        }
        Commands::Verify { file, trust_keys } => {
            let label = file.to_string_lossy().to_string();
            // Verify without a trust file is meaningless: fail closed
            // instead of panicking on the missing keys.
            let trust = load_trust(&Some(trust_keys))?
                .ok_or_else(|| anyhow::anyhow!("verify requires --trust-keys <file>"))?;
            match active_core::sign::verify_file(&file, &trust) {
                Ok(kid) => outln!("{label}: valid signature from key '{kid}'"),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Fmt {
            files,
            style,
            write,
            check,
        } => {
            let style = match style.as_str() {
                "brace" | "bracketed" => Style::Brace,
                "indent" | "indented" => Style::Indent,
                "preserve" => Style::Preserve,
                other => anyhow::bail!("unsupported style: {other} (brace|indent|preserve)"),
            };
            if write && check {
                anyhow::bail!("--write and --check are contradictory");
            }
            if files.len() != 1 && !write && !check {
                anyhow::bail!("multiple files require --write or --check");
            }
            // Formatting parses strictly with includes unresolved: the
            // composition structure survives and files needing repair
            // fail instead of being rewritten.
            let options = with_parse_limits(ParseOptions {
                resolve_includes: false,
                ..ParseOptions::default()
            });
            let mut dirty = false;
            for file in &files {
                let name = file.to_string_lossy().to_string();
                let module = parse_file(file, options.clone()).map_err(|diags| {
                    for d in &diags {
                        report(d, &name);
                    }
                    anyhow::anyhow!("cannot format a file with parse errors: {name}")
                })?;
                // Strict by contract (LANG-D-001, spec/formatter.md 2): the
                // parser records auto-applied healing instead of failing, so
                // refuse here when it had to repair anything. Formatting a
                // repaired tree would bake the repair into the source.
                if !module.healing.is_empty() {
                    for d in &module.healing {
                        report(d, &name);
                    }
                    anyhow::bail!("cannot format a file that needs repair: {name}");
                }
                let out = active_core::fmt::format(&module, style);
                let current = std::fs::read(file)?;
                let unchanged = current == out.as_bytes();
                if check {
                    if !unchanged {
                        outln!("would reformat: {name}");
                        dirty = true;
                    }
                    continue;
                }
                if write {
                    if !unchanged {
                        std::fs::write(file, &out)?;
                        outln!("formatted {name}");
                    }
                    continue;
                }
                out!("{out}");
            }
            if dirty {
                std::process::exit(1);
            }
        }
        Commands::Lint {
            files,
            disable,
            config_file,
            output_format,
            allow_parent_escapes,
        } => {
            use active_core::lint;
            let format = LintFormat::parse(&output_format)?;
            let parse_options = with_parse_limits(ParseOptions {
                allow_parent_escapes,
                ..Default::default()
            });
            // An explicit file wins; otherwise each input discovers the
            // nearest `.active-lint.toml` above it (deterministic per
            // file, so a workspace can scope rules by subtree).
            let explicit = match &config_file {
                Some(path) => Some(
                    lint::LintConfig::load(path)
                        .map_err(|e| anyhow::anyhow!("bad --config file: {e}"))?,
                ),
                None => None,
            };
            // Read-only across files: every file always runs, even
            // when an earlier file fails to parse.
            let mut dirty = false;
            let mut items: Vec<(String, Diagnostic)> = Vec::new();
            for file in &files {
                let name = file.to_string_lossy().to_string();
                let options = if let Some(config) = &explicit {
                    config.options(&disable)
                } else if let Some((_, config)) = lint::LintConfig::discover(file)
                    .map_err(|e| anyhow::anyhow!("bad .active-lint.toml: {e}"))?
                {
                    config.options(&disable)
                } else {
                    lint::LintOptions::with_disabled(&disable)
                }
                .map_err(|e| {
                    eprintln!("[Error] {e}");
                    anyhow::anyhow!("bad lint configuration")
                })?;
                let module = match parse_file(file, parse_options.clone()) {
                    Ok(module) => module,
                    Err(diags) => {
                        // Parse failures are not lint findings: the
                        // pinned JSON/SARIF schema admits `^L[0-9]{3}$`
                        // only. They still reach the user (stderr) and
                        // still fail the run.
                        for d in &diags {
                            report(d, &name);
                        }
                        dirty = true;
                        continue;
                    }
                };
                // Lint messages can quote schema prose; a deprecation
                // reason copied from a host value must redact like every
                // other prose surface. The linter itself stays hermetic
                // (no environment snapshot), so the CLI applies the
                // analyzed material set here.
                let material = material_profile(&module)?;
                for mut d in lint::lint_with(&module, &options) {
                    if let Some(text) = material.redact_text(&d.message) {
                        d.message = text;
                    }
                    if format == LintFormat::Text {
                        report(&d, &name);
                    } else {
                        items.push((name.clone(), d.clone()));
                    }
                    dirty = true;
                }
            }
            match format {
                LintFormat::Text => {}
                LintFormat::Json => {
                    outln!(
                        "{}",
                        serde_json::to_string_pretty(&lint::json_report(&items))?
                    );
                }
                LintFormat::Sarif => {
                    outln!(
                        "{}",
                        serde_json::to_string_pretty(&lint::sarif_report(
                            &items,
                            env!("CARGO_PKG_VERSION")
                        ))?
                    );
                }
            }
            if dirty {
                std::process::exit(1);
            }
        }
        Commands::Doctor { path, format } => {
            let explicit = path.is_some();
            let root = match path {
                Some(p) => p,
                None => std::env::current_dir()?,
            };
            run_doctor(&root, explicit, &format)?;
        }
    }
    Ok(())
        })
        .expect("spawn command thread");
    join_worker(worker, "the command")?
}

#[cfg(test)]
mod panic_isolation_tests {
    use super::join_worker;

    #[test]
    fn a_panicking_worker_becomes_a_coded_error() {
        // LANG-I-012: the CLI joins its workers through this helper, so a
        // panic in evaluation reports E030 and exit 1 instead of a bare
        // exit 101 with a trace.
        let handle = std::thread::spawn(|| -> anyhow::Result<()> { panic!("boom") });
        let err = join_worker(handle, "the command").expect_err("panic must not propagate");
        assert!(err.to_string().starts_with("E030"), "{err}");
        // The payload is not echoed: it can contain document text.
        assert!(!err.to_string().contains("boom"), "{err}");
    }

    #[test]
    fn a_clean_worker_result_passes_through() {
        let handle = std::thread::spawn(|| -> anyhow::Result<()> { Ok(()) });
        assert!(join_worker(handle, "the command").expect("clean").is_ok());
    }
}
