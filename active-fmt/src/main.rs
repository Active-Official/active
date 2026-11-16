//! active-fmt - dual-syntax formatter.
//!
//! Re-emits an Active document in brace style, indentation style, or the
//! style the source used, preserving comments and canonical blank lines
//! (spec/dual-syntax.md §8).
//!
//! Strict by contract (spec/formatter.md §2): a file that needs repair is
//! never rewritten. The parser records auto-applied healing (H001/H002/H003,
//! W011) instead of failing, so this binary checks the healing list after a
//! successful parse and refuses with the diagnostics on stderr and exit 1.
//! That keeps `active-fmt`/`active fmt` usable as a CI gate: a clean exit
//! means the file was already canonical, not that it was quietly repaired.

use active_core::fmt::format;
use active_core::{parse_file, Module, ParseOptions, Style};
use clap::{Parser, ValueEnum};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, ValueEnum)]
enum StyleArg {
    Brace,
    Indent,
    Preserve,
}

#[derive(Parser)]
#[command(name = "active-fmt", about = "Format Active files")]
struct Cli {
    file: PathBuf,
    /// Output style: brace, indent, or preserve (re-emit in the source's
    /// predominant style). Defaults to indent.
    #[arg(long, value_enum, conflicts_with_all = ["bracketed", "indented"])]
    style: Option<StyleArg>,
    /// Emit the fully bracketed (brace) style (alias for --style brace)
    #[arg(long, conflicts_with = "indented")]
    bracketed: bool,
    /// Emit the indentation style (the default per P-040; alias for
    /// --style indent)
    #[arg(long, conflicts_with = "bracketed")]
    indented: bool,
    /// Write the result back to the file (skipped, with no "formatted"
    /// report, when the file is already canonical)
    #[arg(short, long)]
    write: bool,
    /// CI mode: report whether the file needs formatting, write nothing
    #[arg(long, conflicts_with = "write")]
    check: bool,
}

/// Refuse a document the parser had to repair: formatting it would bake the
/// repair into the source (spec/formatter.md §2).
fn refuse_healing(module: &Module, name: &str) -> anyhow::Result<()> {
    if module.healing.is_empty() {
        return Ok(());
    }
    for d in &module.healing {
        match &d.span {
            Some(s) => eprintln!(
                "{}:{}:{}: [Warning] {}: {}",
                name, s.line, s.column, d.code, d.message
            ),
            None => eprintln!("[Warning] {}: {}", d.code, d.message),
        }
    }
    anyhow::bail!("cannot format a file that needs repair: {name}")
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let name = cli.file.to_string_lossy().to_string();

    let style = match cli.style {
        Some(StyleArg::Brace) => Style::Brace,
        Some(StyleArg::Indent) => Style::Indent,
        Some(StyleArg::Preserve) => Style::Preserve,
        None => match (cli.bracketed, cli.indented) {
            (true, _) => Style::Brace,
            // Indentation is the default style (P-040 human-indented
            // default); --indented selects it explicitly.
            _ => Style::Indent,
        },
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
    refuse_healing(&module, &name)?;

    let out = format(&module, style);
    // Compare BYTES (D-005): a stray encoding mismatch must surface as the
    // same "needs formatting" verdict, never as an I/O error above it, and
    // the same comparison drives --write's skip (LANG-D-901).
    let current = fs::read(&cli.file)?;
    let unchanged = current == out.as_bytes();
    if cli.check {
        if !unchanged {
            println!("would reformat: {}", name);
            std::process::exit(1);
        }
        return Ok(());
    }
    if cli.write {
        if !unchanged {
            fs::write(&cli.file, &out)?;
            println!("formatted {}", cli.file.display());
        }
        return Ok(());
    }
    print!("{out}");
    Ok(())
}
