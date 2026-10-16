//! SOLID Core — Parser, IR, Evaluator, Types, Diagnostics
//!
//! This is the alpha production foundation of the SOLID language.

pub mod ast;
pub mod convert;
pub mod diag;
pub mod eval;
pub mod fmt;
pub mod graph;
pub mod heal;
pub mod ir;
pub mod lint;
pub mod parser;
pub mod types;
pub mod validate;

pub use diag::{Diagnostic, Repair, RepairKind, Severity};
pub use eval::{Budget, BudgetKind, Engine, EvalError};
pub use ir::{
    Annotation, AnnotationKind, Attachment, BinOp, DependsOn, IncludeStmt, InterpPart, Module,
    Node, NodeId, PatchOp, SchemaField, SemanticTag, Style, TriviaComment, UnOp, Value,
};
pub use lint::{lint, lint_with, LintOptions};
pub use parser::{
    parse, parse_file, parse_recovering, parse_text_recovering_with_file, parse_text_with_file,
    parse_with, Lexed, ParseOptions,
};
pub use types::{Constraint, Unit};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
