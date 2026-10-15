//! Evaluator — topological evaluation of the IR
//!
//! - Cycle detection
//! - Topological order
//! - Full string interpolation
//! - Unit normalization (time → ms, bytes → raw bytes)
//! - Secret redaction / taint
//! - Basic arithmetic and logic

use petgraph::algo::{is_cyclic_directed, toposort};
use std::collections::HashMap;
use thiserror::Error;

use crate::diag::Span;
use crate::diag::{Diagnostic, Severity};
use crate::ir::*;
use crate::types::Unit;

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("cycle detected in dependency graph")]
    CycleDetected,
    #[error("missing root")]
    MissingRoot,
    #[error("node {0:?} not found")]
    MissingNode(NodeId),
    #[error("type error: {message}")]
    TypeError { message: String, span: Option<Span> },
    #[error("secret used in unsafe context")]
    SecretLeak,
}

impl EvalError {
    /// Stable diagnostic code for this error class (see docs/diagnostics.md).
    pub fn code(&self) -> &'static str {
        match self {
            EvalError::CycleDetected => "S001",
            EvalError::MissingRoot => "S002",
            EvalError::MissingNode(_) => "S003",
            EvalError::TypeError { .. } => "T001",
            EvalError::SecretLeak => "S004",
        }
    }

    /// Source span when the error can be attributed to a node position.
    pub fn span(&self) -> Option<&Span> {
        match self {
            EvalError::TypeError { span, .. } => span.as_ref(),
            _ => None,
        }
    }

    /// Attach a source span to a type error (used by the evaluation loop,
    /// which knows the failing node's id).
    fn type_error(message: impl Into<String>, span: Option<Span>) -> Self {
        EvalError::TypeError {
            message: message.into(),
            span,
        }
    }
}

pub struct Engine {
    pub resilient: bool,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self { resilient: true }
    }

    pub fn evaluate(&self, module: &Module) -> Result<(Value, Vec<Diagnostic>), EvalError> {
        if is_cyclic_directed(&module.graph) {
            return Err(EvalError::CycleDetected);
        }

        let order = toposort(&module.graph, None).map_err(|_| EvalError::CycleDetected)?;

        let mut values: HashMap<NodeId, Value> = HashMap::new();
        let mut diagnostics = module.diagnostics.clone();

        for node_idx in order {
            let node_id = module.graph[node_idx];
            let node = module.get(node_id).ok_or(EvalError::MissingNode(node_id))?;

            let value = match node {
                Node::Literal(v) => v.clone(),

                Node::Env { name, default } => match std::env::var(name) {
                    Ok(s) => Value::String(s),
                    Err(_) => {
                        if let Some(def) = default {
                            values.get(def).cloned().unwrap_or(Value::Null)
                        } else {
                            Value::Null
                        }
                    }
                },

                Node::Secret { locator } => {
                    let loc = values.get(locator).cloned().unwrap_or(Value::Null);
                    match loc {
                        Value::String(s) => Value::Secret(s),
                        _ => Value::Secret(String::new()),
                    }
                }

                Node::Ref(id) => values.get(id).cloned().unwrap_or(Value::Null),

                Node::Ident(name) => {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        code: "W001".into(),
                        message: format!("unresolved identifier '{}'", name),
                        path: Some(name.clone()),
                        span: module.node_spans.get(&node_id).cloned(),
                        repair: None,
                        confidence: 1.0,
                    });
                    Value::Null
                }

                Node::Interpolate { parts } => {
                    let mut out = String::new();
                    for part in parts {
                        match part {
                            InterpPart::Literal(s) => out.push_str(s),
                            InterpPart::Expr(id) => {
                                if let Some(v) = values.get(id) {
                                    match v {
                                        Value::String(s) => out.push_str(s),
                                        Value::Int(i) => out.push_str(&i.to_string()),
                                        Value::Float(f) => out.push_str(&f.to_string()),
                                        Value::Bool(b) => out.push_str(&b.to_string()),
                                        Value::DurationMs(ms) => out.push_str(&format!("{}ms", ms)),
                                        Value::Quantity(n, unit) => {
                                            out.push_str(&format!("{}{}", n, unit.suffix()))
                                        }
                                        Value::Secret(_) => {
                                            return Err(EvalError::SecretLeak);
                                        }
                                        Value::Null => out.push_str("null"),
                                        _ => out.push_str("<?>"),
                                    }
                                } else {
                                    out.push_str("null");
                                }
                            }
                        }
                    }
                    Value::String(out)
                }

                Node::Binary { op, lhs, rhs } => {
                    let l = values.get(lhs).cloned().unwrap_or(Value::Null);
                    let r = values.get(rhs).cloned().unwrap_or(Value::Null);
                    eval_binary(*op, &l, &r).map_err(|m| {
                        EvalError::type_error(m, module.node_spans.get(&node_id).cloned())
                    })?
                }

                Node::Unary { op, expr } => {
                    let v = values.get(expr).cloned().unwrap_or(Value::Null);
                    eval_unary(*op, &v).map_err(|m| {
                        EvalError::type_error(m, module.node_spans.get(&node_id).cloned())
                    })?
                }

                Node::Array { elements } => {
                    let els: Vec<Value> = elements
                        .iter()
                        .map(|e| values.get(e).cloned().unwrap_or(Value::Null))
                        .collect();
                    Value::Array(els)
                }

                Node::Table { fields } => {
                    let mut map = indexmap::IndexMap::new();
                    for (k, id) in fields {
                        map.insert(k.clone(), values.get(id).cloned().unwrap_or(Value::Null));
                    }
                    Value::Table(map)
                }

                Node::Quantity { value, unit } => {
                    let v = values.get(value).cloned().unwrap_or(Value::Null);
                    normalize_quantity(&v, unit).map_err(|m| {
                        EvalError::type_error(m, module.node_spans.get(&node_id).cloned())
                    })?
                }

                _ => Value::Null,
            };

            values.insert(node_id, value);
        }

        let root = module.root.ok_or(EvalError::MissingRoot)?;
        let result = values.get(&root).cloned().unwrap_or(Value::Null);
        Ok((result, diagnostics))
    }
}

fn normalize_quantity(v: &Value, unit: &Unit) -> Result<Value, String> {
    let num = match v {
        Value::Int(n) => *n as f64,
        Value::Float(f) => *f,
        _ => {
            return Err(format!("quantity requires numeric value, got {:?}", v));
        }
    };

    if let Some(ms) = unit.normalize_time_to_ms(num) {
        return Ok(Value::DurationMs(ms));
    }
    if let Some(bytes) = unit.normalize_to_bytes(num) {
        // Represent bytes as Int for simplicity in the alpha
        return Ok(Value::Int(bytes as i64));
    }
    // Rates and custom units retain their dimension (e.g. 100/req/s).
    Ok(Value::Quantity(num, unit.clone()))
}

fn eval_binary(op: BinOp, lhs: &Value, rhs: &Value) -> Result<Value, String> {
    use BinOp::*;
    match (op, lhs, rhs) {
        (Add, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
        (Sub, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
        (Mul, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
        (Div, Value::Int(a), Value::Int(b)) if *b != 0 => Ok(Value::Int(a / b)),

        (Add, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
        (Sub, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
        (Mul, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
        (Div, Value::Float(a), Value::Float(b)) if *b != 0.0 => Ok(Value::Float(a / b)),

        // Mixed int/float
        (Add, Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
        (Add, Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + *b as f64)),
        (Mul, Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 * b)),
        (Mul, Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * *b as f64)),

        (Eq, a, b) => Ok(Value::Bool(value_eq(a, b))),
        (Ne, a, b) => Ok(Value::Bool(!value_eq(a, b))),

        (Lt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
        (Le, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
        (Gt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
        (Ge, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),

        // Duration arithmetic (time is the only dimension with a dedicated
        // Value; bytes normalize to Int). Scalar mul/div, same-dimension
        // add/sub and ordering. Mismatched dimensions (e.g. DurationMs + Int)
        // fall through to the type error below.
        (Add, Value::DurationMs(a), Value::DurationMs(b)) => {
            Ok(Value::DurationMs(a.saturating_add(*b)))
        }
        (Sub, Value::DurationMs(a), Value::DurationMs(b)) => {
            Ok(Value::DurationMs(a.saturating_sub(*b)))
        }
        (Mul, Value::DurationMs(a), Value::Int(b)) => {
            Ok(Value::DurationMs(scale_dur(*a, *b as f64)))
        }
        (Mul, Value::Int(a), Value::DurationMs(b)) => {
            Ok(Value::DurationMs(scale_dur(*b, *a as f64)))
        }
        (Mul, Value::DurationMs(a), Value::Float(b)) => Ok(Value::DurationMs(scale_dur(*a, *b))),
        (Mul, Value::Float(a), Value::DurationMs(b)) => Ok(Value::DurationMs(scale_dur(*b, *a))),
        (Div, Value::DurationMs(a), Value::Int(b)) if *b != 0 => {
            Ok(Value::DurationMs(scale_dur(*a, 1.0 / *b as f64)))
        }
        (Div, Value::DurationMs(a), Value::Float(b)) if *b != 0.0 => {
            Ok(Value::DurationMs(scale_dur(*a, 1.0 / *b)))
        }
        (Div, Value::DurationMs(a), Value::DurationMs(b)) if *b != 0 => {
            Ok(Value::Int((*a / *b) as i64))
        }
        (Lt, Value::DurationMs(a), Value::DurationMs(b)) => Ok(Value::Bool(a < b)),
        (Le, Value::DurationMs(a), Value::DurationMs(b)) => Ok(Value::Bool(a <= b)),
        (Gt, Value::DurationMs(a), Value::DurationMs(b)) => Ok(Value::Bool(a > b)),
        (Ge, Value::DurationMs(a), Value::DurationMs(b)) => Ok(Value::Bool(a >= b)),

        (And, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(*a && *b)),
        (Or, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(*a || *b)),

        _ => Err(format!(
            "unsupported binary op {:?} on {:?} and {:?}",
            op, lhs, rhs
        )),
    }
}

/// Scale a duration in milliseconds by a factor, clamped at zero.
fn scale_dur(ms: u64, factor: f64) -> u64 {
    (ms as f64 * factor).round().max(0.0) as u64
}

fn eval_unary(op: UnOp, v: &Value) -> Result<Value, String> {
    match (op, v) {
        (UnOp::Neg, Value::Int(n)) => Ok(Value::Int(-n)),
        (UnOp::Neg, Value::Float(f)) => Ok(Value::Float(-f)),
        (UnOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        _ => Err(format!("unsupported unary op {:?} on {:?}", op, v)),
    }
}

fn value_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => (x - y).abs() < f64::EPSILON,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::DurationMs(x), Value::DurationMs(y)) => x == y,
        (Value::Quantity(x, ux), Value::Quantity(y, uy)) => x == y && ux == uy,
        _ => false,
    }
}
