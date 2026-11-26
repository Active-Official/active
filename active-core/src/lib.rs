//! Active Core — Parser, IR, Evaluator, Types, Diagnostics
//!
//! This is the alpha production foundation of the Active language.

pub mod ast;
pub mod authority;
pub mod blob;
pub mod cache;
pub mod convert;
pub mod debug;
pub mod diag;
pub mod diff;
pub mod envelope;
pub mod eval;
pub mod fmt;
pub mod graph;
pub mod heal;
pub mod ir;
pub mod layers;
pub mod lint;
pub mod parser;
pub mod provenance;
pub mod sandbox;
pub mod secret;
pub mod sign;
pub mod store;
pub mod trace;
pub mod types;
pub mod unify;
pub mod validate;
pub mod viz;

pub use authority::{Capability, CapabilitySet};
pub use diag::{Diagnostic, Repair, RepairKind, Severity};
pub use envelope::{EnvelopeGate, SUPPORTED_ENVELOPE_MAJORS};
pub use eval::{
    Budget, BudgetKind, ChargeReport, Demand, DemandOutcome, Engine, EnvLookup, EvalError,
    IncrementalMemo, StepEvent,
};
pub use ir::{
    Annotation, AnnotationKind, Attachment, BinOp, DependsOn, IncludeStmt, InterpPart, Mode,
    Module, Node, NodeId, PatchOp, SchemaField, SchemaMeta, SemanticTag, Style, TriviaComment,
    UnOp, Value,
};
pub use lint::{lint, lint_with, LintOptions};
pub use parser::{
    parse, parse_file, parse_file_with_trust, parse_recovering, parse_text_recovering_with_file,
    parse_text_with_file, parse_text_with_trust, parse_with, validate_patch_path, IncludeLoader,
    Lexed, ParseOptions, PathSeg,
};

pub use sandbox::{ClosureError, IncludeFile, InputClosure, Sandbox, SandboxedModule};
pub use types::{Constraint, Unit};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
