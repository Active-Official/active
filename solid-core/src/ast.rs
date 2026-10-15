//! Concrete Syntax Tree (placeholder for future expansion)
//! TODO(solid C-022): the dual-syntax parser may re-introduce a concrete
//! syntax tree for lossless layout round-tripping; the alpha lowers directly
//! to IR. The parser currently builds the IR directly.
//! The alpha parser currently builds the IR directly.

#![allow(dead_code)]

#[derive(Debug, Clone)]
pub struct Document {
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone)]
pub enum Statement {
    Let { name: String, expr: Expr },
    Table { name: String, body: Vec<Statement> },
    KeyValue { key: String, value: Expr },
}

#[derive(Debug, Clone)]
pub enum Expr {
    Null,
    Bool(bool),
    Int(i64),
    String(String),
    Ident(String),
    Env {
        name: String,
        default: Option<Box<Expr>>,
    },
    Array(Vec<Expr>),
    Table(Vec<(String, Expr)>),
}
