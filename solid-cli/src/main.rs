use clap::{Args, Parser, Subcommand};
use solid_core::convert::{
    json_text_to_solid, toml_text_to_solid, value_to_pretty_json, value_to_toml_string,
    value_to_yaml_string, yaml_text_to_solid,
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
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// Show dependency graph in simple text form
    Graph {
        file: PathBuf,
        /// Enable medium-confidence healing repairs (resilient mode)
        #[arg(long)]
        resilient: bool,
        /// Allow includes to use absolute paths or escape the directory
        #[arg(long)]
        allow_parent_escapes: bool,
        /// Select a profile by name, overriding the active_profile key
        #[arg(long)]
        profile: Option<String>,
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

fn engine(resilient: bool, profile: &Option<String>, budget: Budget) -> Engine {
    let mut engine = Engine::new();
    // The CLI is strict by default: only --resilient tolerates
    // violations. Engine::new stays resilient for library use.
    engine.resilient = resilient;
    engine.profile_override = profile.clone();
    engine.budget = budget;
    engine
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
            profile,
            budget,
        } => {
            let name = file.to_string_lossy().to_string();
            let module =
                parse_file(&file, options(resilient, allow_parent_escapes)).map_err(|diags| {
                    for d in &diags {
                        report(d, &name);
                    }
                    anyhow::anyhow!("parse failed")
                })?;

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
            profile,
            budget,
        } => {
            let name = file.to_string_lossy().to_string();
            match parse_file(&file, options(resilient, allow_parent_escapes)) {
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
                Err(diags) => {
                    for d in diags {
                        report(&d, &name);
                    }
                    std::process::exit(1);
                }
            }
        }
        Commands::Graph {
            file,
            resilient,
            allow_parent_escapes,
            profile: _,
        } => {
            let name = file.to_string_lossy().to_string();
            let module =
                parse_file(&file, options(resilient, allow_parent_escapes)).map_err(|diags| {
                    for d in &diags {
                        report(d, &name);
                    }
                    anyhow::anyhow!("parse failed")
                })?;
            for d in &module.healing {
                report(d, &name);
            }

            println!("Bindings:");
            for (name, id) in &module.bindings {
                println!("  let {} = NodeId({})", name, id.0);
            }
            println!("\nGraph edges (dependency → dependent):");
            for edge in module.graph.raw_edges() {
                let from = module.graph[edge.source()];
                let to = module.graph[edge.target()];
                println!("  NodeId({}) → NodeId({})", from.0, to.0);
            }
            if had_failure(&module.healing, resilient) {
                std::process::exit(1);
            }
        }
        Commands::Convert {
            file,
            to,
            resilient,
            allow_parent_escapes,
            profile,
            budget,
        } => {
            let name = file.to_string_lossy().to_string();
            let module =
                parse_file(&file, options(resilient, allow_parent_escapes)).map_err(|diags| {
                    for d in &diags {
                        report(d, &name);
                    }
                    anyhow::anyhow!("parse failed")
                })?;
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
