//! SOLID Core — Parser, IR, Evaluator, Types, Diagnostics
//!
//! This is the alpha production foundation of the SOLID language.

pub mod ast;
pub mod convert;
pub mod diag;
pub mod eval;
pub mod fmt;
pub mod heal;
pub mod ir;
pub mod parser;
pub mod types;

pub use diag::{Diagnostic, Repair, RepairKind, Severity};
pub use eval::Engine;
pub use ir::{
    Attachment, BinOp, IncludeStmt, Module, Node, NodeId, PatchOp, SemanticTag, Style,
    TriviaComment, UnOp, Value,
};
pub use parser::{parse, parse_file, parse_recovering, parse_with, ParseOptions};
pub use types::{Constraint, Unit};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
