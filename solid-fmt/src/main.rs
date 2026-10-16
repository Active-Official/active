//! solid-fmt — dual-syntax formatter.
//!
//! Re-emits a SOLID document in brace style, indentation style, or the
//! style the source used, preserving comments and canonical blank lines
//! (spec/dual-syntax.md §8).

use clap::Parser;
use solid_core::fmt::format;
use solid_core::{parse_file, ParseOptions, Style};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "solid-fmt", about = "Format SOLID files")]
struct Cli {
    file: PathBuf,
    /// Emit the fully bracketed (brace) style
    #[arg(long, conflicts_with = "indented")]
    bracketed: bool,
    /// Emit the indentation style (the default per P-040)
    #[arg(long, conflicts_with = "bracketed")]
    indented: bool,
    /// Write the result back to the file
    #[arg(short, long)]
    write: bool,
    /// CI mode: report whether the file needs formatting, write nothing
    #[arg(long, conflicts_with = "write")]
    check: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let name = cli.file.to_string_lossy().to_string();

    let style = match (cli.bracketed, cli.indented) {
        (true, _) => Style::Brace,
        // Indentation is the default style (P-040 human-indented
        // default); --indented selects it explicitly.
        _ => Style::Indent,
    };

    // Formatting preserves include statements alongside local keys: the
    // merge is skipped (resolve_includes = false) so the output keeps
    // the same composition structure as the input.
    let options = ParseOptions {
        resolve_includes: false,
        ..ParseOptions::default()
    };
    let module = parse_file(&cli.file, options).map_err(|diags| {
        for d in &diags {
            match &d.span {
                Some(s) => eprintln!(
                    "{}:{}:{}: [Error] {}: {}",
                    name, s.line, s.column, d.code, d.message
                ),
                None => eprintln!("[Error] {}: {}", d.code, d.message),
            }
        }
        anyhow::anyhow!("cannot format a file with parse errors")
    })?;

    let out = format(&module, style);
    if cli.check {
        let current = fs::read_to_string(&cli.file)?;
        if current != out {
            println!("would reformat: {}", name);
            std::process::exit(1);
        }
        return Ok(());
    }
    if cli.write {
        fs::write(&cli.file, &out)?;
        println!("formatted {}", cli.file.display());
    } else {
        print!("{out}");
    }
    Ok(())
}
