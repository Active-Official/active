use clap::{Args, Parser, Subcommand};
use solid_core::convert::{
    collect_items, doc_to_html, doc_to_man, doc_to_markdown, json_text_to_solid, schema_to_go,
    schema_to_json_schema, schema_to_openapi, schema_to_rust, schema_to_typescript,
    toml_text_to_solid, value_to_pretty_json, value_to_toml_string, value_to_yaml_string,
    yaml_text_to_solid,
};
use solid_core::diag::{Diagnostic, Severity};
use solid_core::{parse_file, Budget, Engine, EvalError, ParseOptions, Style};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "solid",
    version,
    about = "SOLID configuration language toolchain"
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
}

impl BudgetArgs {
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
}

#[derive(Subcommand)]
enum Commands {
    /// Parse and evaluate a .solid file, print as JSON
    Eval {
        /// Input .solid file
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
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Show dependency graph (text, Mermaid, DOT, or JSON)
    Graph {
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
    /// Import JSON, YAML, or TOML as SOLID source (best-effort)
    Import {
        /// Input file
        file: PathBuf,
        /// Source format
        #[arg(short, long, default_value = "json")]
        from: String,
    },
    /// Generate typed bindings from a SOLID schema
    Generate {
        /// Input .solid file (must contain a schema block)
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
    /// Render schema, intent, and comments as documentation
    Doc {
        /// Input .solid file
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
    },
    /// Emit a reproducible lockfile (content hash + provenance)
    Lock {
        /// Input .solid file
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
        /// Input .solid files (at least one)
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
        /// Input .solid files (at least one)
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Disable a rule (repeatable): unused-lets, depends-on,
        /// complexity, hardcoded-secrets, deprecated
        #[arg(long = "disable-lint")]
        disable: Vec<String>,
    },
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
fn load_trust(path: &Option<PathBuf>) -> anyhow::Result<Option<solid_core::sign::TrustedKeys>> {
    match path {
        None => Ok(None),
        Some(file) => {
            let text = std::fs::read_to_string(file)
                .map_err(|e| anyhow::anyhow!("cannot read trust file {}: {e}", file.display()))?;
            solid_core::sign::load_trusted_keys(&text)
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
    trust: &Option<solid_core::sign::TrustedKeys>,
) -> anyhow::Result<solid_core::Module> {
    let name = file.to_string_lossy().to_string();
    let result = match trust {
        Some(keys) => {
            solid_core::parse_file_with_trust(file, options(resilient, allow_parent_escapes), keys)
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

fn engine(resilient: bool, profile: &Option<String>, budget: Budget) -> Engine {
    let mut engine = Engine::new();
    // The CLI is strict by default: only --resilient tolerates
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
    trust: &Option<solid_core::sign::TrustedKeys>,
) -> anyhow::Result<(solid_core::Module, String)> {
    let name = file.to_string_lossy().to_string();
    let module = parse_input(file, resilient, allow_parent_escapes, trust)?;
    Ok((module, name))
}

/// True when the exit code must be non-zero: strict mode fails on any
/// Warning-or-Error; resilient mode fails only on Error.
fn had_failure(diags: &[Diagnostic], resilient: bool) -> bool {
    diags
        .iter()
        .any(|d| d.severity == Severity::Error || (!resilient && d.severity == Severity::Warning))
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
    let cli = Cli::parse();

    match cli.command {
        Commands::Eval {
            file,
            format,
            resilient,
            allow_parent_escapes,
            trust_keys,
            profile,
            budget,
        } => {
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&file, resilient, allow_parent_escapes, &trust)?;

            let engine = engine(resilient, &profile, budget.budget());
            let (value, diags) = engine
                .evaluate(&module)
                .inspect_err(|e| report_eval_error(e, &name))?;
            for d in &diags {
                report(d, &name);
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
            budget,
        } => {
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            match parse_input(&file, resilient, allow_parent_escapes, &trust) {
                Ok(module) => {
                    let engine = engine(resilient, &profile, budget.budget());
                    match engine.evaluate(&module) {
                        Ok((_, diags)) => {
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
            resilient,
            allow_parent_escapes,
            trust_keys,
            profile: _,
        } => {
            let name = file.to_string_lossy().to_string();
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&file, resilient, allow_parent_escapes, &trust)?;
            for d in &module.healing {
                report(d, &name);
            }

            match format.as_str() {
                "text" => {
                    println!("Bindings:");
                    let mut bindings: Vec<(&String, &solid_core::NodeId)> =
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
                            solid_core::graph::node_label(&module, from),
                            solid_core::graph::node_label(&module, to)
                        );
                    }
                }
                "mermaid" => println!("{}", solid_core::graph::to_mermaid(&module)),
                "dot" => println!("{}", solid_core::graph::to_dot(&module)),
                "json" => println!(
                    "{}",
                    serde_json::to_string_pretty(&solid_core::graph::to_graph_json(&module))
                        .unwrap_or_default()
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
            let id = solid_core::graph::resolve_key(&module, &key)
                .ok_or_else(|| anyhow::anyhow!("unknown key '{key}' in {filename}"))?;
            let paths = solid_core::graph::impact_paths(&module, id);
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
            let id = solid_core::graph::resolve_key(&module, &key)
                .ok_or_else(|| anyhow::anyhow!("unknown key '{key}' in {filename}"))?;
            let paths = solid_core::graph::why_paths(&module, id);
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
            let engine = engine(resilient, &profile, budget.budget());
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
                "json" => json_text_to_solid(&text).map_err(|e| anyhow::anyhow!("{e}"))?,
                "yaml" | "yml" => yaml_text_to_solid(&text).map_err(|e| anyhow::anyhow!("{e}"))?,
                "toml" => toml_text_to_solid(&text).map_err(|e| anyhow::anyhow!("{e}"))?,
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
                "jsonschema" => schema_to_json_schema(&module)
                    .map(|schema| serde_json::to_string_pretty(&schema).unwrap_or_default()),
                "openapi" => schema_to_openapi(&module, &name)
                    .map(|schema| serde_json::to_string_pretty(&schema).unwrap_or_default()),
                "typescript" | "ts" => schema_to_typescript(&module, &name),
                "go" => schema_to_go(&module, &name),
                "rust" | "serde" => schema_to_rust(&module, &name),
                other => anyhow::bail!("unsupported target: {}", other),
            }
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{output}");
        }
        Commands::Doc {
            from,
            to,
            title,
            resilient,
            allow_parent_escapes,
            trust_keys,
        } => {
            let filename = from.to_string_lossy().to_string();
            let title = title.unwrap_or_else(|| {
                from.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Configuration".to_string())
            });
            let trust = load_trust(&trust_keys)?;
            let module = parse_input(&from, resilient, allow_parent_escapes, &trust)?;
            let mut engine = Engine::new();
            engine.resilient = resilient;
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
            let engine = engine(resilient, &profile, budget.budget());
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
                solid_core::Value::Table(map) => match map.get("active_profile") {
                    Some(solid_core::Value::String(name)) if module.profiles.contains_key(name) => {
                        Some(name.clone())
                    }
                    _ => None,
                },
                _ => None,
            });
            let provenance = solid_core::provenance::collect_provenance(
                &module,
                &engine.env,
                Some(file.as_path()),
                effective.as_deref(),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            let manifest = solid_core::provenance::LockManifest::new(
                format!(
                    "solid-cli {} / solid-lang-core {}",
                    env!("CARGO_PKG_VERSION"),
                    solid_core::VERSION
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
                solid_core::sign::generate_secret_key().map_err(|e| anyhow::anyhow!("{e}"))?;
            let secret_path = out.with_extension("secret");
            let public_path = out.with_extension("public");
            std::fs::write(&secret_path, solid_core::sign::secret_hex(&key))
                .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", secret_path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| anyhow::anyhow!("cannot chmod {}: {e}", secret_path.display()))?;
            }
            let verifying = key.verifying_key();
            std::fs::write(&public_path, solid_core::sign::public_hex(&verifying))
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
            let secret = solid_core::sign::load_secret_key(&secret_text)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let bytes =
                std::fs::read(&file).map_err(|e| anyhow::anyhow!("cannot read {label}: {e}"))?;
            let sidecar = solid_core::sign::sign_bytes(&secret, &kid, &bytes)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let sidecar_path = solid_core::sign::sidecar_path_for(&file);
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
            let trust = load_trust(&Some(trust_keys))?.expect("trust file given");
            match solid_core::sign::verify_file(&file, &trust) {
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
                let module = parse_file(file, options).map_err(|diags| {
                    for d in &diags {
                        report(d, &name);
                    }
                    anyhow::anyhow!("cannot format a file with parse errors: {name}")
                })?;
                let out = solid_core::fmt::format(&module, style);
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
        Commands::Lint { files, disable } => {
            use solid_core::lint;
            let options = lint::LintOptions::with_disabled(&disable).map_err(|e| {
                eprintln!("[Error] {e}");
                anyhow::anyhow!("bad --disable-lint flag")
            })?;
            // Read-only across files: every file always runs, even
            // when an earlier file fails to parse.
            let mut dirty = false;
            for file in &files {
                let name = file.to_string_lossy().to_string();
                let module = match parse_file(file, ParseOptions::default()) {
                    Ok(module) => module,
                    Err(diags) => {
                        for d in &diags {
                            report(d, &name);
                        }
                        dirty = true;
                        continue;
                    }
                };
                for d in lint::lint_with(&module, &options) {
                    report(&d, &name);
                    dirty = true;
                }
            }
            if dirty {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
