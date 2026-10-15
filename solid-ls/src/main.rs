//! solid-ls — Language Server (alpha stub)
//!
//! A real implementation would speak LSP over stdio and provide:
//! - semantic tokens
//! - hover (type + origin + @intent)
//! - go-to-definition / find-references
//! - diagnostics from the evaluator
//! - completions from schema
//! - code actions for healing and style conversion

fn main() {
    eprintln!("solid-ls alpha");
    eprintln!("This is a placeholder. A full LSP server would:");
    eprintln!("  • speak JSON-RPC over stdin/stdout");
    eprintln!("  • use solid-core for parsing + evaluation");
    eprintln!("  • publish diagnostics on every change");
    eprintln!("  • answer hover / definition / completion requests");
    eprintln!();
    eprintln!("Exiting.");
}
