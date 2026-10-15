//! solid-fmt — dual-syntax formatter.
//!
//! Re-emits a SOLID document in brace style, indentation style, or the
//! style the source used, preserving comments and canonical blank lines
//! (spec/dual-syntax.md §8).

use clap::Parser;
use solid_core::fmt::format;
use solid_core::{parse, Style};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "solid-fmt", about = "Format SOLID files")]
struct Cli {
    file: PathBuf,
    /// Emit the fully bracketed (brace) style
    #[arg(long, conflicts_with = "indented")]
    bracketed: bool,
    /// Emit the indentation style
    #[arg(long, conflicts_with = "bracketed")]
    indented: bool,
    /// Write the result back to the file
    #[arg(short, long)]
    write: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let src = fs::read_to_string(&cli.file)?;
    let name = cli.file.to_string_lossy().to_string();

    let style = if cli.bracketed {
        Style::Brace
    } else if cli.indented {
        Style::Indent
    } else {
        Style::Preserve
    };

    let module = parse(&src, &name).map_err(|diags| {
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
    if cli.write {
        fs::write(&cli.file, &out)?;
        println!("formatted {}", cli.file.display());
    } else {
        print!("{out}");
    }
    Ok(())
}
