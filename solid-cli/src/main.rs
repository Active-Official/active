use clap::{Parser, Subcommand};
use solid_core::convert::{value_to_pretty_json, value_to_simple_toml};
use solid_core::{parse, Engine};
use std::fs;
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
    },
    /// Check a file for errors / cycles / unresolved names
    Check { file: PathBuf },
    /// Show dependency graph in simple text form
    Graph { file: PathBuf },
    /// Convert evaluated result to JSON or simple TOML
    Convert {
        file: PathBuf,
        #[arg(short, long, default_value = "json")]
        to: String,
    },
}

use solid_core::diag::Diagnostic;

/// Print a diagnostic with its span (path:line:col) when available.
fn report(d: &Diagnostic, file: &str) {
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
        Commands::Eval { file, format } => {
            let src = fs::read_to_string(&file)?;
            let name = file.to_string_lossy().to_string();
            let module = parse(&src, &name).map_err(|diags| {
                for d in &diags {
                    report(d, &name);
                }
                anyhow::anyhow!("parse failed")
            })?;

            let engine = Engine::new();
            let (value, diags) = engine.evaluate(&module).map_err(|e| {
                match e.span() {
                    Some(s) => eprintln!(
                        "{}:{}:{}: [Error] {}: {}",
                        name,
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
        }
        Commands::Check { file } => {
            let src = fs::read_to_string(&file)?;
            let name = file.to_string_lossy().to_string();
            match parse(&src, &name) {
                Ok(module) => {
                    let engine = Engine::new();
                    match engine.evaluate(&module) {
                        Ok((_, diags)) => {
                            if diags.is_empty() {
                                println!("OK");
                            } else {
                                for d in diags {
                                    report(&d, &name);
                                }
                            }
                        }
                        Err(e) => {
                            match e.span() {
                                Some(s) => eprintln!(
                                    "{}:{}:{}: [Error] {}: {}",
                                    name,
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
        Commands::Graph { file } => {
            let src = fs::read_to_string(&file)?;
            let module = parse(&src, file.to_string_lossy().as_ref())
                .map_err(|d| anyhow::anyhow!("parse failed: {} diagnostics", d.len()))?;

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
        }
        Commands::Convert { file, to } => {
            let src = fs::read_to_string(&file)?;
            let module = parse(&src, file.to_string_lossy().as_ref())
                .map_err(|_| anyhow::anyhow!("parse failed"))?;
            let engine = Engine::new();
            let (value, _) = engine.evaluate(&module)?;
            match to.as_str() {
                "json" => println!("{}", value_to_pretty_json(&value)),
                "toml" => println!("{}", value_to_simple_toml(&value, "")),
                other => anyhow::bail!("unsupported target: {}", other),
            }
        }
    }

    Ok(())
}
