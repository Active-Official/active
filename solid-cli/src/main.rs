use clap::{Parser, Subcommand};
use solid_core::convert::{value_to_pretty_json, value_to_simple_toml};
use solid_core::diag::{Diagnostic, Severity};
use solid_core::{parse_file, Engine, ParseOptions};
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
    /// Convert evaluated result to JSON or simple TOML
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
    },
}

fn options(resilient: bool, allow_parent_escapes: bool) -> ParseOptions {
    ParseOptions {
        resilient,
        allow_parent_escapes,
        ..ParseOptions::default()
    }
}

fn engine(profile: &Option<String>) -> Engine {
    let mut engine = Engine::new();
    engine.profile_override = profile.clone();
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Eval {
            file,
            format,
            resilient,
            allow_parent_escapes,
            profile,
        } => {
            let name = file.to_string_lossy().to_string();
            let module =
                parse_file(&file, options(resilient, allow_parent_escapes)).map_err(|diags| {
                    for d in &diags {
                        report(d, &name);
                    }
                    anyhow::anyhow!("parse failed")
                })?;

            let engine = engine(&profile);
            let (value, diags) = engine.evaluate(&module).map_err(|e| {
                let file = e.file().unwrap_or(&name);
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
                e
            })?;
            for d in &diags {
                report(d, &name);
            }

            match format.as_str() {
                "json" => println!("{}", value_to_pretty_json(&value)),
                "toml" => println!("{}", value_to_simple_toml(&value, "")),
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
        } => {
            let name = file.to_string_lossy().to_string();
            match parse_file(&file, options(resilient, allow_parent_escapes)) {
                Ok(module) => {
                    let engine = engine(&profile);
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
                            let file = e.file().unwrap_or(&name);
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
        } => {
            let name = file.to_string_lossy().to_string();
            let module =
                parse_file(&file, options(resilient, allow_parent_escapes)).map_err(|diags| {
                    for d in &diags {
                        report(d, &name);
                    }
                    anyhow::anyhow!("parse failed")
                })?;
            let engine = engine(&profile);
            let (value, diags) = engine.evaluate(&module).map_err(|e| {
                let file = e.file().unwrap_or(&name);
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
                e
            })?;
            for d in &diags {
                report(d, &name);
            }
            match to.as_str() {
                "json" => println!("{}", value_to_pretty_json(&value)),
                "toml" => println!("{}", value_to_simple_toml(&value, "")),
                other => anyhow::bail!("unsupported target: {}", other),
            }
            if had_failure(&diags, resilient) {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
