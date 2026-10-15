//! solid-fmt — dual-syntax formatter (alpha)
//!
//! In production this will parse to AST/IR and re-emit in either
//! human-indentation style or fully-bracketed AI style.

use clap::Parser;
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "solid-fmt", about = "Format SOLID files")]
struct Cli {
    file: PathBuf,
    /// Emit the AI-friendly fully bracketed form
    #[arg(long)]
    bracketed: bool,
    /// Write result back to the file
    #[arg(short, long)]
    write: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let src = fs::read_to_string(&cli.file)?;

    // TODO(solid C-040): real dual-style formatter replaces this echo
    // (indentation style, bracketed style, layout-preserving mode).
    let style = if cli.bracketed {
        "bracketed"
    } else {
        "indented"
    };
    let out = format!("# formatted by solid-fmt (style={})\n{}", style, src);

    if cli.write {
        fs::write(&cli.file, out)?;
        println!("formatted {}", cli.file.display());
    } else {
        print!("{}", out);
    }
    Ok(())
}
