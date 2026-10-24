use active_core::convert::{
    collect_items, doc_to_html, doc_to_man, doc_to_markdown, json_text_to_active,
    jsonschema_to_active, schema_inspect, schema_to_go, schema_to_json_schema, schema_to_openapi,
    schema_to_python, schema_to_rust, schema_to_typescript, toml_text_to_active,
    value_to_pretty_json, value_to_toml_string, value_to_yaml_string, yaml_text_to_active,
};
use active_core::diag::{Diagnostic, Severity};
use active_core::{parse_file, Budget, Engine, EvalError, ParseOptions, Style};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "active",
    version,
    about = "Active configuration language toolchain"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Evaluator resource limits (spec/budgets.md, C-037). Each defaults
/// to the shipped profile; `0` disables that limit.
#[derive(Args, Clone)]
struct BudgetArgs {
    /// Maximum IR nodes visited during evaluation
    #[arg(long, default_value_t = Budget::default().max_nodes)]
    max_nodes: u64,
    /// Maximum value nesting depth
    #[arg(long, default_value_t = Budget::default().max_depth)]
    max_depth: u64,
    /// Longest single array
    #[arg(long, default_value_t = Budget::default().max_array_len)]
    max_array_len: u64,
    /// Total table keys across the document
    #[arg(long, default_value_t = Budget::default().max_keys)]
    max_keys: u64,
    /// Wall-clock milliseconds for the whole evaluation
    #[arg(long, default_value_t = Budget::default().max_wall_ms)]
    max_wall_ms: u64,
    /// Maximum materialized value size (reference expansion counted)
    #[arg(long, default_value_t = Budget::default().max_value_size)]
    max_value_size: u64,
    /// Maximum cumulative materialized size across evaluation
    #[arg(long, default_value_t = Budget::default().max_total_value_size)]
    max_total_value_size: u64,
    /// Named budget profile: strict (CI/serverless), interactive
    /// (REPL/playground), or trusted (vendored/generated input).
    /// A profile replaces every per-field limit above.
    #[arg(long, value_enum)]
    budget_profile: Option<BudgetProfile>,
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
    /// True when the user passed any explicit --max-* flag (vs. all
    /// clap defaults): used to warn when a named profile silently
    /// overrides them.
    fn has_explicit(&self) -> bool {
        self.budget() != Budget::default()
    }

    fn budget(&self) -> Budget {
        Budget {
            max_nodes: self.max_nodes,
            max_depth: self.max_depth,
            max_array_len: self.max_array_len,
            max_keys: self.max_keys,
            max_wall_ms: self.max_wall_ms,
            max_value_size: self.max_value_size,
            max_total_value_size: self.max_total_value_size,
        }
    }

    /// Named budget profile (`strict`, `interactive`, `trusted`)
    /// or explicit per-field limits. A profile wins entirely when
    /// set; per-field flags apply only without a profile. Passing
    /// both prints a warning so a stale `--max-*` flag cannot
    /// silently do nothing.
    fn resolve(&self) -> anyhow::Result<Budget> {
        let profile = match self.budget_profile {
            None => return Ok(self.budget()),
            Some(BudgetProfile::Strict) => Budget::strict(),
            Some(BudgetProfile::Interactive) => Budget::interactive(),
            Some(BudgetProfile::Trusted) => Budget::trusted(),
        };
        if self.budget() != Budget::default() {
            eprintln!("warning: --budget-profile overrides the explicit --max-* flags");
        }
        Ok(profile)
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
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
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
        /// whole graph; the path selects what gets printed.
        /// Dotted/index path (`server.host`, `ports[0]`).
        #[arg(long)]
        path: Option<String>,
        /// Write a secret-redacted audit document of this evaluation
        /// (PROPOSAL §5.1): counts, diagnostic codes, content hash —
        /// itself a valid Active document, never raw values.
        #[arg(long)]
        audit_trail: Option<PathBuf>,
        /// Named sandbox profile (strict|data|agent|trusted): selects
        /// the profile's budget preset and refuses to evaluate
        /// documents declaring capabilities outside the profile
        #[arg(long)]
        sandbox: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Check a file for errors / cycles / unresolved names
    Check {
        file: PathBuf,
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
        /// Emit opt-in evaluation trace (counts and diagnostic codes
        /// only, never values, sources, or secrets) as JSONL on stderr
        #[arg(long)]
        trace: bool,
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
        /// Require sibling signatures on includes (C-052)
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
        /// Require sibling signatures on includes (C-052)
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
        /// Require sibling signatures on includes (C-052)
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
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
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
        /// Output target
        #[arg(short, long)]
        to: String,
        /// Root type name
        #[arg(long, default_value = "Config")]
        name: String,
        /// Require sibling signatures on includes (C-052)
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
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
    },
    /// Step, breakpoint, replay, or reduce a document (debugger)
    Debug {
        /// Input .active file
        #[arg(short, long)]
        from: PathBuf,
        /// Stop at the first matching breakpoint: taint, path:<prefix>,
        /// code:<CODE>, or node:<n>
        #[arg(long = "break", conflicts_with_all = ["replay", "minimize"])]
        break_at: Option<String>,
        /// Deterministic replay: evaluate twice and compare hashes,
        /// codes, and step traces
        #[arg(long, conflicts_with = "minimize")]
        replay: bool,
        /// Expected content hash for --replay (sha256:...)
        #[arg(long, requires = "replay")]
        expect: Option<String>,
        /// Reduce the document while the given diagnostic code survives
        #[arg(long)]
        minimize: Option<String>,
        /// Maximum reduction attempts
        #[arg(long, default_value_t = 200)]
        max_attempts: usize,
        /// Output format: json (default) or text
        #[arg(long, default_value = "json")]
        format: String,
        /// Require sibling signatures on includes (C-052)
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
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
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
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
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
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
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
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
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
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
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
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Generate an Ed25519 key pair for overlay signing
    Keygen {
        /// Output prefix: writes <prefix>.secret (0600) and <prefix>.public
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Sign a file into a detached `<file>.sig` sidecar
    Sign {
        /// File to sign
        file: PathBuf,
        /// Secret key file (64 hex chars)
        #[arg(long)]
        key: PathBuf,
        /// Key id recorded in the sidecar
        #[arg(long)]
        kid: String,
    },
    /// Verify a file against its sidecar and a trusted-keys file
    Verify {
        /// File to verify
        file: PathBuf,
        /// Trusted keys TOML
        #[arg(long)]
        trust_keys: PathBuf,
    },
    /// Format files in a canonical style (spec/formatter.md)
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
        /// Disable a rule (repeatable): unused-lets, depends-on,
        /// complexity, hardcoded-secrets, deprecated
        #[arg(long = "disable-lint")]
        disable: Vec<String>,
        /// Output format: text (human), json (stable schema), sarif 2.1.0
        #[arg(long = "format", default_value = "text")]
        output_format: String,
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
        /// Require sibling signatures on includes (C-052)
        #[arg(long)]
        trust_keys: Option<PathBuf>,
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
fn run_inspect(
    from: &Path,
    path: Option<&str>,
    format: &str,
    trust_keys: &Option<PathBuf>,
) -> anyhow::Result<()> {
    let trust = load_trust(trust_keys)?;
    let module = parse_input(from, false, false, &trust)?;
    match path {
        None => {
            if format != "json" {
                anyhow::bail!("schema inspection supports --format json only");
            }
            let inspected = schema_inspect(&module).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&inspected).map_err(|e| anyhow::anyhow!("{e}"))?
            );
        }
        Some(path) => {
            let (value, _) = Engine::new()
                .evaluate(&module)
                .map_err(|e| anyhow::anyhow!("cannot evaluate for inspection: {e}"))?;
            let report = active_core::convert::inspect_path(&module, &value, path)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            match format {
                "json" => println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|e| anyhow::anyhow!("{e}"))?
                ),
                "text" => print!("{}", active_core::convert::render_inspection_text(&report)),
                other => anyhow::bail!("unknown inspect format `{other}` (want json|text)"),
            }
        }
    }
    Ok(())
}

fn options(resilient: bool, allow_parent_escapes: bool) -> ParseOptions {
    ParseOptions {
        resilient,
        allow_parent_escapes,
        ..ParseOptions::default()
    }
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

/// Parse a file, enforcing overlay signatures when trust is set.
fn parse_input(
    file: &std::path::Path,
    resilient: bool,
    allow_parent_escapes: bool,
    trust: &Option<active_core::sign::TrustedKeys>,
) -> anyhow::Result<active_core::Module> {
    let name = file.to_string_lossy().to_string();
    let result = match trust {
        Some(keys) => {
            active_core::parse_file_with_trust(file, options(resilient, allow_parent_escapes), keys)
        }
        None => parse_file(file, options(resilient, allow_parent_escapes)),
    };
    result.map_err(|diags| {
        for d in &diags {
            report(d, &name);
        }
        anyhow::anyhow!("parse failed")
    })
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
            println!("capabilities ok (none declared)");
        } else {
            println!("capabilities ok: {}", module.capabilities.join(", "));
        }
        let _ = name;
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "host refuses capabilities: {} (document {name} declares them, this host does not offer them)",
            refused.join(", ")
        ))
    }
}

/// Secret-redacted audit document (PROPOSAL §5.1, CHECKLIST AUD.001):
/// counts, diagnostic codes, mode, and the content hash of the
/// REDACTED result — itself a valid Active document, never raw
/// values or secrets. Written on success and on validation failure
/// alike (the outcome field tells them apart); fatal engine errors
/// return before any value exists, so they carry no audit trail.
fn write_audit_trail(
    path: &std::path::Path,
    file: &std::path::Path,
    module: &active_core::Module,
    value: &active_core::Value,
    diags: &[active_core::Diagnostic],
    resilient: bool,
) -> anyhow::Result<()> {
    fn quote(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
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
    // warning; resilient tolerates warnings but never errors.
    let failed = had_failure(diags, resilient);
    let doc = format!(
        "audit = {{\n  tool = \"active\",\n  version = {},\n  source = {},\n  mode = \"{}\",\n  nodes = {},\n  diagnostics = [{}],\n  result = \"{}\",\n  outcome = \"{}\"\n}}\n",
        quote(env!("CARGO_PKG_VERSION")),
        quote(&file.to_string_lossy()),
        match module.mode {
            active_core::Mode::Data => "data",
            active_core::Mode::Config => "config",
        },
        module.arena.len(),
        codes_list,
        active_core::provenance::content_hash(value),
        if failed { "failed" } else { "clean" },
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
    let evaluation_engine = engine(resilient, profile, Budget::default());
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
                    println!("{p}");
                }
                Ok(())
            }
            "json" => {
                println!("{}", serde_json::json!({"query": title, "paths": paths}));
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
            let (value, diags) = evaluation_engine
                .evaluate(&module)
                .map_err(|e| anyhow::anyhow!("evaluation failed: {e}"))?;
            for d in &diags {
                report(d, &name);
            }
            if query.starts_with("secret") {
                print_paths(query, &active_core::graph::secret_tainted_paths(&value))
            } else {
                let blobs = active_core::graph::blobs_above_size(&value, min_size);
                match format {
                    "text" => {
                        for (path, size, hash) in &blobs {
                            println!("{path} size={size} blake3:{hash}");
                        }
                        Ok(())
                    }
                    "json" => {
                        println!(
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

fn engine(resilient: bool, profile: &Option<String>, budget: Budget) -> Engine {
    let mut engine = Engine::new(); // The CLI is strict by default: only --resilient tolerates
                                    // violations. Engine::new stays resilient for library use.
    engine.resilient = resilient;
    engine.profile_override = profile.clone();
    engine.budget = budget;
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

/// Strict-evaluation diagnostics for the explain/patch surfaces: parse
/// errors abort in parse_input; validation diagnostics are the working
/// set; any other engine failure is fatal and reported.
fn collect_diags(module: &active_core::Module, filename: &str, budget: Budget) -> Vec<Diagnostic> {
    match engine(false, &None, budget).evaluate(module) {
        Ok((_, diags)) => diags,
        Err(EvalError::ValidationFailed { diagnostics }) => diagnostics,
        Err(e) => {
            report_eval_error(&e, filename);
            std::process::exit(2);
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

fn main() -> anyhow::Result<()> {
    // The derived clap command tree is large; parse on a thread with a
    // generous stack so debug builds on Windows cannot overflow while
    // building help or parsing arguments.
    let cli = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(Cli::parse)
        .expect("spawn argument parser")
        .join()
        .expect("argument parser thread");

    match cli.command {
        Commands::Eval {
            file,
            format,
            resilient,
            allow_parent_escapes,
            trust_keys,
            profile,
            trace,
            materialise_blobs,
            path,
            audit_trail,
            sandbox,
            budget,
        } => {
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&file, resilient, allow_parent_escapes, &trust)?;

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
                    anyhow::bail!(
                        "sandbox '{sandbox}' refuses capabilities: {}",
                        refused.join(", ")
                    );
                }
            }

            let mut eng = engine(resilient, &profile, budget);
            eng.materialise_blobs = materialise_blobs;
            let engine = eng;
            active_core::trace::set_tracing(trace);
            let (value, diags) = match engine.evaluate(&module) {
                Ok(out) => {
                    dump_trail(trace);
                    out
                }
                Err(e) => {
                    dump_trail(trace);
                    report_eval_error(&e, &name);
                    return Err(anyhow::anyhow!("evaluation failed"));
                }
            };
            for d in &diags {
                report(d, &name);
            }

            // Demanded cone selection (PROPOSAL §2.2): project AFTER the
            // full budgeted evaluation, so safety stays whole-document
            // while agents fetch only what they need. The cone is owned
            // (cloned out) so downstream borrows stay single-level.
            // Materialisation demand applies to the cone, not the whole
            // tree: undemanded tainted blobs cannot fail a cone fetch
            // (budgets still guarded the full walk).
            let value = match path.as_deref() {
                Some(p) => active_core::convert::project_path(&value, p)
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                    .clone(),
                None => value,
            };

            // Explicit materialisation demand (spec/blob.md §5): fail
            // closed on tainted (B003) or payload-less (B005) blobs
            // inside the emitted cone instead of silently emitting
            // identity.
            if materialise_blobs {
                active_core::convert::require_materialised(&value).map_err(|e| {
                    report_eval_error(&e, &name);
                    anyhow::anyhow!("evaluation failed")
                })?;
            }

            match format.as_str() {
                "json" => println!("{}", value_to_pretty_json(&value)),
                "yaml" => match value_to_yaml_string(&value) {
                    Ok(yaml) => println!("{yaml}"),
                    Err(e) => anyhow::bail!("yaml export failed: {e}"),
                },
                "toml" => match value_to_toml_string(&value) {
                    Ok((toml, export_diags)) => {
                        for d in &export_diags {
                            report(d, &name);
                        }
                        println!("{toml}");
                    }
                    Err(e) => anyhow::bail!("toml export failed: {e}"),
                },
                other => anyhow::bail!("unsupported format: {}", other),
            }
            if let Some(audit_path) = audit_trail {
                write_audit_trail(&audit_path, &file, &module, &value, &diags, resilient)?;
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
        } => {
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            match parse_input(&file, resilient, allow_parent_escapes, &trust) {
                Ok(module) => {
                    let engine = engine(resilient, &profile, budget.resolve()?);
                    active_core::trace::set_tracing(trace);
                    match engine.evaluate(&module) {
                        Ok((_, diags)) => {
                            dump_trail(trace);
                            if diags.is_empty() {
                                println!("OK");
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
                return run_graph_query(
                    &resolved,
                    &query,
                    key.as_deref(),
                    min_size,
                    &effective_format,
                    resilient,
                    allow_parent_escapes,
                    &trust_keys,
                    &profile,
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
                    println!("Bindings:");
                    let mut bindings: Vec<(&String, &active_core::NodeId)> =
                        module.bindings.iter().collect();
                    bindings.sort_by(|a, b| a.0.cmp(b.0));
                    for (name, id) in bindings {
                        println!("  let {} = NodeId({})", name, id.0);
                    }
                    println!("\nGraph edges (dependency → dependent):");
                    for edge in module.graph.raw_edges() {
                        let from = module.graph[edge.source()];
                        let to = module.graph[edge.target()];
                        println!(
                            "  {} → {}",
                            active_core::graph::node_label(&module, from),
                            active_core::graph::node_label(&module, to)
                        );
                    }
                }
                "mermaid" => println!("{}", active_core::graph::to_mermaid(&module)),
                "dot" => println!("{}", active_core::graph::to_dot(&module)),
                "json" => println!(
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
            let paths = active_core::graph::impact_paths(&module, id);
            match format.as_str() {
                "text" => {
                    for path in paths {
                        println!("{path}");
                    }
                }
                "json" => println!("{}", serde_json::json!({"key": key, "paths": paths})),
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
                        println!("{path}");
                    }
                }
                "json" => println!("{}", serde_json::json!({"key": key, "paths": paths})),
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
        } => {
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&file, resilient, allow_parent_escapes, &trust)?;
            let engine = engine(resilient, &profile, budget.resolve()?);
            let (value, diags) = engine
                .evaluate(&module)
                .inspect_err(|e| report_eval_error(e, &name))?;
            for d in &diags {
                report(d, &name);
            }
            match to.as_str() {
                "json" => println!("{}", value_to_pretty_json(&value)),
                "yaml" => match value_to_yaml_string(&value) {
                    Ok(yaml) => println!("{yaml}"),
                    Err(e) => anyhow::bail!("yaml export failed: {e}"),
                },
                "toml" => match value_to_toml_string(&value) {
                    Ok((toml, export_diags)) => {
                        for d in &export_diags {
                            report(d, &name);
                        }
                        println!("{toml}");
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
            print!("{source}");
        }
        Commands::Generate {
            from,
            to,
            name,
            trust_keys,
        } => {
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&from, false, false, &trust)?;
            // Documents without a schema block exit non-zero (spec §5).
            let output = match to.as_str() {
                "jsonschema" => schema_to_json_schema(&module).and_then(|schema| {
                    serde_json::to_string_pretty(&schema)
                        .map_err(|e| format!("generate serialize failed: {e}"))
                }),
                "openapi" => schema_to_openapi(&module, &name).and_then(|schema| {
                    serde_json::to_string_pretty(&schema)
                        .map_err(|e| format!("generate serialize failed: {e}"))
                }),
                "typescript" | "ts" => schema_to_typescript(&module, &name),
                "go" => schema_to_go(&module, &name),
                "rust" | "serde" => schema_to_rust(&module, &name),
                "python" | "py" => schema_to_python(&module, &name),
                other => anyhow::bail!("unsupported target: {}", other),
            }
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            // Emit exactly one trailing newline: generator outputs
            // already end with `\n`, and `println!` would append a
            // second one (a blank line that `gofmt` rejects).
            println!("{}", output.trim_end_matches('\n'));
        }
        Commands::Inspect {
            from,
            path,
            format,
            trust_keys,
        } => {
            run_inspect(&from, path.as_deref(), &format, &trust_keys)?;
        }
        Commands::Schema { action } => {
            let SchemaAction::Inspect {
                from,
                path,
                format,
                trust_keys,
            } = action;
            run_inspect(&from, path.as_deref(), &format, &trust_keys)?;
        }
        Commands::Mode { from, format } => {
            let module = parse_input(&from, false, false, &None)?;
            let marker = module.mode_marker;
            let mode = match module.mode {
                active_core::Mode::Data => "data",
                active_core::Mode::Config => "config",
            };
            if format == "text" {
                println!("mode: {mode}");
                println!("marker: {}", if marker { "present" } else { "absent" });
                if module.capabilities.is_empty() {
                    println!("capabilities: (none)");
                } else {
                    println!("capabilities: {}", module.capabilities.join(", "));
                }
            } else if format == "json" {
                println!(
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
            let BudgetAction::Explain { from, format } = action;
            let module = parse_input(&from, false, false, &None)?;
            let engine = Engine::new();
            let (value, diagnostics) = engine
                .evaluate(&module)
                .map_err(|e| anyhow::anyhow!("cannot evaluate: {e}"))?;
            let data = active_core::viz::budget_data(&module, &value, &engine.budget, &diagnostics);
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
                    println!(
                        "{label}: used {} of {} (headroom {})",
                        usage[key], limits[key], headroom[key]
                    );
                }
                println!(
                    "wall clock: {} ms (host-time limit; not sampled here)",
                    limits["wall_ms"]
                );
            } else if format == "json" {
                println!(
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
            trust_keys,
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
                let report = active_core::debug::minimal_case_with(
                    &source,
                    &code,
                    max_attempts,
                    Some(&from),
                );
                if format == "text" {
                    println!(
                        "reduced {} -> {} bytes ({} attempts) keeping {}",
                        report["original_bytes"], report["reduced_bytes"], report["attempts"], code
                    );
                    print!("{}", report["reduced"].as_str().unwrap_or(""));
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report)
                            .map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
                if !report["target_present"].as_bool().unwrap_or(false) {
                    if format == "text" {
                        println!("target {code} not found in the document");
                    }
                    std::process::exit(1);
                }
                return Ok(());
            }
            let module = parse_input(&from, false, false, &trust)?;
            if replay {
                let report = active_core::debug::replay(&module, expect.as_deref())
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                if format == "text" {
                    println!(
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
                    println!(
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
            let (_, _, steps) = Engine::new()
                .evaluate_with_steps(&module)
                .map_err(|e| anyhow::anyhow!("cannot evaluate for stepping: {e}"))?;
            let (shown, matched) = match break_at.as_deref() {
                Some(selector) => match active_core::debug::breakpoint_index(&steps, selector) {
                    Some(index) => (steps[..=index].to_vec(), Some(index)),
                    None => (Vec::new(), None),
                },
                None => (steps.clone(), None),
            };
            if format == "text" {
                print!("{}", active_core::debug::render_steps(&shown));
                if let Some(selector) = &break_at {
                    match matched {
                        Some(index) => println!("break at #{index} ({selector})"),
                        None => println!("breakpoint not reached ({selector})"),
                    }
                }
            } else {
                let report = serde_json::json!({
                    "breakpoint": break_at,
                    "matched": matched,
                    "total_steps": steps.len(),
                    "steps": shown,
                });
                println!(
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
                    parse_input(&from, false, allow_parent_escapes, &trust)?;
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
                let mut data =
                    serde_json::to_value(&outcome).map_err(|e| anyhow::anyhow!("{e}"))?;
                if let Some(object) = data.as_object_mut() {
                    object.remove("repaired_source");
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&data).map_err(|e| anyhow::anyhow!("{e}"))?
                );
                return Ok(());
            }
            let module = parse_input(&from, false, allow_parent_escapes, &trust)?;
            match kind.as_str() {
                "graph" => match format.as_str() {
                    "json" => println!(
                        "{}",
                        serde_json::to_string_pretty(&active_core::graph::to_graph_json(&module))
                            .map_err(|e| anyhow::anyhow!("{e}"))?
                    ),
                    "mermaid" => print!("{}", active_core::graph::to_mermaid(&module)),
                    "dot" => print!("{}", active_core::graph::to_dot(&module)),
                    other => {
                        anyhow::bail!("unknown graph format `{other}` (want json|mermaid|dot)")
                    }
                },
                "taint" => {
                    let (value, _) = Engine::new()
                        .evaluate(&module)
                        .map_err(|e| anyhow::anyhow!("cannot evaluate: {e}"))?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&active_core::viz::taint_data(
                            &module, &value
                        ))
                        .map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
                "budget" => {
                    let engine = Engine::new();
                    let (value, diagnostics) = engine
                        .evaluate(&module)
                        .map_err(|e| anyhow::anyhow!("cannot evaluate: {e}"))?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&active_core::viz::budget_data(
                            &module,
                            &value,
                            &engine.budget,
                            &diagnostics
                        ))
                        .map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
                "schema" => {
                    let data = active_core::viz::schema_data(&module)
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    println!(
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
        } => {
            let filename = from.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&from, false, allow_parent_escapes, &trust)?;
            let diags = collect_diags(&module, &filename, budget.resolve()?);
            match format {
                OutFormat::Text => {
                    for line in explain_lines(&diags) {
                        println!("{line}");
                    }
                }
                OutFormat::Json => {
                    let items = explain_items(&diags);
                    let structured = active_core::convert::explain_structured(&items);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&structured)
                            .map_err(|e| anyhow::anyhow!("{e}"))?
                    );
                }
            }
        }
        Commands::Patch {
            from,
            format,
            allow_parent_escapes,
            trust_keys,
            budget,
        } => {
            let filename = from.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&from, false, allow_parent_escapes, &trust)?;
            let diags = collect_diags(&module, &filename, budget.resolve()?);
            match format {
                OutFormat::Text => {
                    for directive in active_core::convert::suggest_patches(&module, &diags) {
                        println!("{directive}");
                    }
                }
                OutFormat::Json => {
                    let structured =
                        active_core::convert::suggest_patches_structured(&module, &diags);
                    println!(
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
                    Some(p) => println!(
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
            println!(
                "{}",
                serde_json::to_string_pretty(&descriptor)
                    .map_err(|e| anyhow::anyhow!("capability serialize failed: {e}"))?
            );
        }
        Commands::Compatibility { old, new, format } => {
            let old_module = parse_input(&old, false, false, &None)?;
            let new_module = parse_input(&new, false, false, &None)?;
            let findings = active_core::convert::compare_schemas(&old_module, &new_module)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            match format.as_str() {
                "json" => println!(
                    "{}",
                    serde_json::to_string_pretty(&findings)
                        .map_err(|e| anyhow::anyhow!("compatibility serialize failed: {e}"))?
                ),
                "text" => {
                    if findings.is_empty() {
                        println!("compatible: no schema changes");
                    }
                    for finding in &findings {
                        println!(
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
            allow_parent_escapes,
            trust_keys,
            profile,
            budget,
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
                parse_input(&from, false, allow_parent_escapes, &trust)?;
            }
            let mut eng = engine(false, &profile, budget.resolve()?);
            eng.materialise_blobs = false;
            if browser {
                let model = active_core::convert::repair::browser_model_with(
                    &source,
                    goal_parsed.clone(),
                    &preserve,
                    max_steps,
                    &eng,
                    Some(&from),
                    allow_parent_escapes,
                );
                println!(
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
            match format.as_str() {
                "json" => println!(
                    "{}",
                    serde_json::to_string_pretty(&outcome)
                        .map_err(|e| anyhow::anyhow!("repair serialize failed: {e}"))?
                ),
                "text" => {
                    if outcome.clean {
                        println!("repair: clean");
                    }
                    for step in &outcome.steps {
                        match &step.status {
                            active_core::convert::repair::StepStatus::Applied => {
                                println!("applied {} {}", step.kind, step.directive)
                            }
                            active_core::convert::repair::StepStatus::Skipped { reason } => {
                                println!("skipped {} {} ({reason})", step.kind, step.directive)
                            }
                        }
                    }
                    for finding in &outcome.findings {
                        report(finding, &from.to_string_lossy());
                    }
                    if !outcome.clean {
                        println!("repair: not clean ({})", outcome.final_codes.join(", "));
                    }
                    if partial {
                        println!("selected: {} directives", applied_directives.len());
                        for directive in &applied_directives {
                            println!("apply {directive}");
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
        } => {
            let filename = from.to_string_lossy().to_string();
            let title = title.unwrap_or_else(|| {
                from.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Configuration".to_string())
            });
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&from, resilient, allow_parent_escapes, &trust)?;
            let engine = engine(resilient, &None, budget.resolve()?);
            let (value, diags) = engine
                .evaluate(&module)
                .map_err(|e| anyhow::anyhow!("evaluation failed: {e}"))?;
            for d in &diags {
                report(d, &filename);
            }
            let items = collect_items(&module, &value);
            match to.as_str() {
                "markdown" | "md" => println!("{}", doc_to_markdown(&title, &items)),
                "html" => println!("{}", doc_to_html(&title, &items)),
                "man" => println!("{}", doc_to_man(&title, &items)),
                other => anyhow::bail!("unsupported target: {}", other),
            }
        }
        Commands::Lock {
            file,
            output,
            resilient,
            allow_parent_escapes,
            trust_keys,
            profile,
            budget,
        } => {
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&file, resilient, allow_parent_escapes, &trust)?;
            let engine = engine(resilient, &profile, budget.resolve()?);
            let (value, diags) = engine
                .evaluate(&module)
                .inspect_err(|e| report_eval_error(e, &name))?;
            for d in &diags {
                report(d, &name);
            }
            // The effective profile, when one is active: selection
            // changes values, so it joins the provenance (override
            // wins, else the document's own valid selector).
            let effective = profile.clone().or_else(|| match &value {
                active_core::Value::Table(map) => match map.get("active_profile") {
                    Some(active_core::Value::String(name))
                        if module.profiles.contains_key(name) =>
                    {
                        Some(name.clone())
                    }
                    _ => None,
                },
                _ => None,
            });
            let provenance = active_core::provenance::collect_provenance(
                &module,
                &engine.env,
                Some(file.as_path()),
                effective.as_deref(),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            let manifest = active_core::provenance::LockManifest::new(
                format!(
                    "active-cli {} / active-core {}",
                    env!("CARGO_PKG_VERSION"),
                    active_core::VERSION
                ),
                &value,
                provenance,
            );
            let rendered = manifest.render();
            match output {
                Some(path) => std::fs::write(&path, &rendered)
                    .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?,
                None => print!("{rendered}"),
            }
            if had_failure(&diags, resilient) {
                std::process::exit(1);
            }
        }
        Commands::Keygen { out } => {
            // Fresh Ed25519 seed from OS randomness; the secret file
            // is created owner-only (0600 on Unix).
            let key =
                active_core::sign::generate_secret_key().map_err(|e| anyhow::anyhow!("{e}"))?;
            let secret_path = out.with_extension("secret");
            let public_path = out.with_extension("public");
            std::fs::write(&secret_path, active_core::sign::secret_hex(&key))
                .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", secret_path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| anyhow::anyhow!("cannot chmod {}: {e}", secret_path.display()))?;
            }
            let verifying = key.verifying_key();
            std::fs::write(&public_path, active_core::sign::public_hex(&verifying))
                .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", public_path.display()))?;
            println!(
                "wrote {} and {}",
                secret_path.display(),
                public_path.display()
            );
        }
        Commands::Sign { file, key, kid } => {
            let label = file.to_string_lossy().to_string();
            let secret_text = std::fs::read_to_string(&key)
                .map_err(|e| anyhow::anyhow!("cannot read key {}: {e}", key.display()))?;
            let secret = active_core::sign::load_secret_key(&secret_text)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let bytes =
                std::fs::read(&file).map_err(|e| anyhow::anyhow!("cannot read {label}: {e}"))?;
            let sidecar = active_core::sign::sign_bytes(&secret, &kid, &bytes)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let sidecar_path = active_core::sign::sidecar_path_for(&file);
            std::fs::write(&sidecar_path, sidecar).map_err(|e| {
                anyhow::anyhow!(
                    "cannot write {}: {e}",
                    std::path::PathBuf::from(&sidecar_path).display()
                )
            })?;
            println!("signed {label} for key '{kid}'");
        }
        Commands::Verify { file, trust_keys } => {
            let label = file.to_string_lossy().to_string();
            // Verify without a trust file is meaningless: fail closed
            // instead of panicking on the missing keys.
            let trust = load_trust(&Some(trust_keys))?
                .ok_or_else(|| anyhow::anyhow!("verify requires --trust-keys <file>"))?;
            match active_core::sign::verify_file(&file, &trust) {
                Ok(kid) => println!("{label}: valid signature from key '{kid}'"),
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
            let options = ParseOptions {
                resolve_includes: false,
                ..ParseOptions::default()
            };
            let mut dirty = false;
            for file in &files {
                let name = file.to_string_lossy().to_string();
                let module = parse_file(file, options.clone()).map_err(|diags| {
                    for d in &diags {
                        report(d, &name);
                    }
                    anyhow::anyhow!("cannot format a file with parse errors: {name}")
                })?;
                let out = active_core::fmt::format(&module, style);
                if check {
                    let current = std::fs::read_to_string(file)?;
                    if current != out {
                        println!("would reformat: {name}");
                        dirty = true;
                    }
                    continue;
                }
                if write {
                    let current = std::fs::read_to_string(file)?;
                    if current != out {
                        std::fs::write(file, &out)?;
                        println!("formatted {name}");
                    }
                    continue;
                }
                print!("{out}");
            }
            if dirty {
                std::process::exit(1);
            }
        }
        Commands::Lint {
            files,
            disable,
            output_format,
        } => {
            use active_core::lint;
            let format = LintFormat::parse(&output_format)?;
            let options = lint::LintOptions::with_disabled(&disable).map_err(|e| {
                eprintln!("[Error] {e}");
                anyhow::anyhow!("bad --disable-lint flag")
            })?;
            // Read-only across files: every file always runs, even
            // when an earlier file fails to parse.
            let mut dirty = false;
            let mut items: Vec<(String, Diagnostic)> = Vec::new();
            for file in &files {
                let name = file.to_string_lossy().to_string();
                let module = match parse_file(file, ParseOptions::default()) {
                    Ok(module) => module,
                    Err(diags) => {
                        for d in &diags {
                            if format == LintFormat::Text {
                                report(d, &name);
                            } else {
                                items.push((name.clone(), d.clone()));
                            }
                        }
                        dirty = true;
                        continue;
                    }
                };
                for d in lint::lint_with(&module, &options) {
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
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&lint::json_report(&items))?
                    );
                }
                LintFormat::Sarif => {
                    println!(
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
    }

    Ok(())
}
