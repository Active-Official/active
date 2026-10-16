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
use crate::parser::{validate_patch_path, PathSeg};
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
    TypeError {
        message: String,
        span: Option<Span>,
        file: Option<String>,
    },
    /// A patch application failure with its own stable code (E019):
    /// rename conflicts and invalid paths discovered at apply time.
    /// Missing remove/rename targets warn (W004) instead of failing.
    #[error("patch error: {message}")]
    PatchError {
        message: String,
        span: Option<Span>,
        file: Option<String>,
    },
    #[error("secret used in unsafe context")]
    SecretLeak { file: Option<String> },
}

impl EvalError {
    /// Stable diagnostic code for this error class (see docs/diagnostics.md).
    pub fn code(&self) -> &'static str {
        match self {
            EvalError::CycleDetected => "S001",
            EvalError::MissingRoot => "S002",
            EvalError::MissingNode(_) => "S003",
            EvalError::TypeError { .. } => "T001",
            EvalError::PatchError { .. } => "E019",
            EvalError::SecretLeak { .. } => "S004",
        }
    }

    /// Source span when the error can be attributed to a node position.
    pub fn span(&self) -> Option<&Span> {
        match self {
            EvalError::TypeError { span, .. } | EvalError::PatchError { span, .. } => span.as_ref(),
            _ => None,
        }
    }

    /// Source file when the error can be attributed to a node position
    /// (multi-file documents via includes, C-025).
    pub fn file(&self) -> Option<&str> {
        match self {
            EvalError::TypeError { file, .. } | EvalError::PatchError { file, .. } => {
                file.as_deref()
            }
            EvalError::SecretLeak { file } => file.as_deref(),
            _ => None,
        }
    }

    /// Attach a source position to a type error (used by the evaluation
    /// loop, which knows the failing node's id).
    fn type_error(message: impl Into<String>, module: &Module, node: NodeId) -> Self {
        EvalError::TypeError {
            message: message.into(),
            span: module.node_spans.get(&node).cloned(),
            file: module.node_files.get(&node).cloned(),
        }
    }

    /// Attach a source position to a patch error (rename conflicts and
    /// invalid paths discovered at apply time).
    fn patch_error(message: impl Into<String>, module: &Module, node: NodeId) -> Self {
        EvalError::PatchError {
            message: message.into(),
            span: module.node_spans.get(&node).cloned(),
            file: module.node_files.get(&node).cloned(),
        }
    }

    /// Attach a source file to a secret-leak error.
    fn secret_leak(module: &Module, node: NodeId) -> Self {
        EvalError::SecretLeak {
            file: module.node_files.get(&node).cloned(),
        }
    }
}

pub struct Engine {
    pub resilient: bool,
    /// CLI `--profile` override: when set, selects this profile instead
    /// of the document's `active_profile` key (spec §3.5).
    pub profile_override: Option<String>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            resilient: true,
            profile_override: None,
        }
    }

    pub fn evaluate(&self, module: &Module) -> Result<(Value, Vec<Diagnostic>), EvalError> {
        if is_cyclic_directed(&module.graph) {
            return Err(EvalError::CycleDetected);
        }

        let order = toposort(&module.graph, None).map_err(|_| EvalError::CycleDetected)?;

        let mut values: HashMap<NodeId, Value> = HashMap::new();
        let mut diagnostics = module.diagnostics.clone();
        diagnostics.extend(module.healing.iter().cloned());

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

                // Semantic tags are tool treatment only: transparent to the value.
                Node::Semantic { inner, .. } => values.get(inner).cloned().unwrap_or(Value::Null),

                // Schema nodes carry types, not values: they evaluate to
                // Null and never export (extracted from value fields at
                // parse). Their constraint arguments evaluate normally;
                // C-031 reads those values during validation.
                Node::SchemaField { .. } | Node::SchemaTable { .. } | Node::SchemaArray { .. } => {
                    Value::Null
                }
                Node::Ident(name) => {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        code: "W001".into(),
                        message: format!("unresolved identifier '{}'", name),
                        path: Some(name.clone()),
                        span: module.node_spans.get(&node_id).cloned(),
                        repair: None,
                        confidence: 1.0,
                        file: module.node_files.get(&node_id).cloned(),
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
                                            return Err(EvalError::secret_leak(module, node_id));
                                        }
                                        Value::Null => out.push_str("null"),
                                        // Structured values have no string
                                        // form: interpolating them is a type
                                        // error, never a placeholder.
                                        Value::Array(_) | Value::Table(_) => {
                                            return Err(EvalError::type_error(
                                                format!(
                                                    "cannot interpolate a {} into a string",
                                                    match v {
                                                        Value::Array(_) => "array",
                                                        _ => "table",
                                                    }
                                                ),
                                                module,
                                                node_id,
                                            ));
                                        }
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
                    eval_binary(*op, &l, &r)
                        .map_err(|m| EvalError::type_error(m, module, node_id))?
                }

                Node::Unary { op, expr } => {
                    let v = values.get(expr).cloned().unwrap_or(Value::Null);
                    eval_unary(*op, &v).map_err(|m| EvalError::type_error(m, module, node_id))?
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
                    normalize_quantity(&v, unit)
                        .map_err(|m| EvalError::type_error(m, module, node_id))?
                }

                _ => Value::Null,
            };

            values.insert(node_id, value);
        }

        let root = module.root.ok_or(EvalError::MissingRoot)?;
        let result = values.get(&root).cloned().unwrap_or(Value::Null);
        let result = self.apply_profile(module, &values, result, &mut diagnostics);
        let mut result = result;
        self.apply_patches(module, &values, &mut result, &mut diagnostics)?;
        Ok((result, diagnostics))
    }

    /// Apply structural patch directives in source order (spec §2.9):
    /// after evaluation (and profile merge), before schema validation.
    /// Values were computed with the base tree intact, so a patch may
    /// reference the very key it overwrites. Missing remove/rename
    /// targets warn (W004); an existing rename target is E019.
    fn apply_patches(
        &self,
        module: &Module,
        values: &HashMap<NodeId, Value>,
        result: &mut Value,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(), EvalError> {
        use crate::ir::PatchOp;

        for &patch_id in &module.patches {
            let (op, span, file) = match module.get(patch_id) {
                Some(Node::PatchDirective { op }) => (
                    op.clone(),
                    module.node_spans.get(&patch_id).cloned(),
                    module.node_files.get(&patch_id).cloned(),
                ),
                _ => continue,
            };
            let mut warn = |code: &str, message: String| {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: code.into(),
                    message,
                    path: None,
                    span: span.clone(),
                    repair: None,
                    confidence: 1.0,
                    file: file.clone(),
                });
            };
            match op {
                PatchOp::Set { path, value } => {
                    let segs = validate_patch_path(&path).map_err(|why| {
                        EvalError::patch_error(
                            format!("invalid patch path `{path}`: {why}"),
                            module,
                            patch_id,
                        )
                    })?;
                    let v = values.get(&value).cloned().unwrap_or(Value::Null);
                    // Failed sets roll back: navigation may have
                    // materialized intermediate tables before hitting a
                    // blocked hop, and a failed patch must leave no trace.
                    let snapshot = result.clone();
                    if !patch_set(result, &segs, v) {
                        *result = snapshot;
                        warn(
                            "W004",
                            format!("patch target `{path}` is blocked or out of bounds"),
                        );
                    }
                }
                PatchOp::Remove { path } => {
                    let segs = validate_patch_path(&path).map_err(|why| {
                        EvalError::patch_error(
                            format!("invalid patch path `{path}`: {why}"),
                            module,
                            patch_id,
                        )
                    })?;
                    if !patch_remove(result, &segs) {
                        warn("W004", format!("remove target `{path}` does not exist"));
                    }
                }
                PatchOp::Rename { from, to } => {
                    let segs = validate_patch_path(&from).map_err(|why| {
                        EvalError::patch_error(
                            format!("invalid patch path `{from}`: {why}"),
                            module,
                            patch_id,
                        )
                    })?;
                    match patch_rename(result, &segs, &to) {
                        RenameOutcome::Ok => {}
                        RenameOutcome::Missing => {
                            warn("W004", format!("rename source `{from}` does not exist"));
                        }
                        RenameOutcome::Exists => {
                            return Err(EvalError::patch_error(
                                format!("rename target `{to}` already exists"),
                                module,
                                patch_id,
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Select and merge the active profile (spec §3.5). The `--profile`
    /// override wins; otherwise the root value's `active_profile` field
    /// decides. Missing key → base only. Unknown name or non-string →
    /// W002. Type changes on override → W003. Every profile evaluates
    /// whether selected or not (strictness catches rot in inactive
    /// environments); selection reads the BASE key only.
    fn apply_profile(
        &self,
        module: &Module,
        values: &HashMap<NodeId, Value>,
        result: Value,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Value {
        if module.profiles.is_empty() {
            // No profiles defined: the effective selector (override first,
            // then the key) still validates, then base stands.
            let selected: Option<String> = match &self.profile_override {
                Some(name) => Some(name.clone()),
                None => match &result {
                    Value::Table(map) => match map.get("active_profile") {
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(_) => {
                            diagnostics.push(Diagnostic {
                                severity: Severity::Warning,
                                code: "W002".into(),
                                message: "active_profile must be a string".into(),
                                path: Some("active_profile".into()),
                                span: if self.profile_override.is_some() {
                                    None
                                } else {
                                    active_key_span(module, &result)
                                },
                                repair: None,
                                confidence: 1.0,
                                file: if self.profile_override.is_some() {
                                    None
                                } else {
                                    active_key_file(module, &result)
                                },
                            });
                            return result;
                        }
                        None => None,
                    },
                    _ => None,
                },
            };
            if let Some(name) = selected {
                // With no profiles defined, any effective selector is unknown.
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "W002".into(),
                    message: format!("unknown profile '{name}'"),
                    path: Some("active_profile".into()),
                    span: if self.profile_override.is_some() {
                        None
                    } else {
                        active_key_span(module, &result)
                    },
                    repair: None,
                    confidence: 1.0,
                    file: if self.profile_override.is_some() {
                        None
                    } else {
                        active_key_file(module, &result)
                    },
                });
            }
            return result;
        }
        let base_table = match &result {
            Value::Table(map) => map.clone(),
            _ => return result,
        };
        let selected: Option<String> = match &self.profile_override {
            Some(name) => Some(name.clone()),
            None => match base_table.get("active_profile") {
                None => return result,
                Some(Value::String(s)) => Some(s.clone()),
                Some(other) => {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        code: "W002".into(),
                        message: format!(
                            "active_profile must be a string, got {}",
                            value_kind_name(other)
                        ),
                        path: Some("active_profile".into()),
                        span: if self.profile_override.is_some() {
                            None
                        } else {
                            active_key_span(module, &result)
                        },
                        repair: None,
                        confidence: 1.0,
                        file: if self.profile_override.is_some() {
                            None
                        } else {
                            active_key_file(module, &result)
                        },
                    });
                    return result;
                }
            },
        };
        let name = match selected {
            Some(n) => n,
            None => return result,
        };
        let Some(&profile_id) = module.profiles.get(&name) else {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                code: "W002".into(),
                message: format!("unknown profile '{name}'"),
                path: Some("active_profile".into()),
                span: if self.profile_override.is_some() {
                    None
                } else {
                    active_key_span(module, &result)
                },
                repair: None,
                confidence: 1.0,
                file: if self.profile_override.is_some() {
                    None
                } else {
                    active_key_file(module, &result)
                },
            });
            return result;
        };
        let profile_value = values.get(&profile_id).cloned().unwrap_or(Value::Null);
        let mut merged = base_table;
        merge_profile_value(
            &mut merged,
            profile_value,
            &name,
            String::new(),
            module.node_spans.get(&profile_id).cloned(),
            module.node_files.get(&profile_id).cloned(),
            diagnostics,
        );
        Value::Table(merged)
    }
}

/// Span of the root `active_profile` key's node, when it exists.
fn active_key_span(module: &Module, result: &Value) -> Option<crate::diag::Span> {
    let Value::Table(_) = result else {
        return None;
    };
    let root = module.root?;
    let Node::Table { fields } = module.get(root)? else {
        return None;
    };
    let id = fields.get("active_profile")?;
    module.node_spans.get(id).cloned()
}

/// File of the root `active_profile` key's node, when recorded.
fn active_key_file(module: &Module, result: &Value) -> Option<String> {
    let Value::Table(_) = result else {
        return None;
    };
    let root = module.root?;
    let Node::Table { fields } = module.get(root)? else {
        return None;
    };
    let id = fields.get("active_profile")?;
    module.node_files.get(id).cloned()
}

/// Short kind name for diagnostics (never includes values, so secrets
/// cannot leak through it). Numbers share a kind; quantities carry
/// their unit so different dimensions compare unequal.
fn value_kind_name(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "boolean".into(),
        Value::Int(_) | Value::Float(_) => "number".into(),
        Value::String(_) => "string".into(),
        Value::DurationMs(_) => "duration".into(),
        Value::Quantity(_, unit) => format!("quantity:{}", unit.suffix()),
        Value::Array(_) => "array".into(),
        Value::Table(_) => "table".into(),
        Value::Secret(_) => "secret".into(),
    }
}

/// Merge a profile table into the base table in place (spec §3.5):
/// tables recurse, scalars last-wins (profile wins). Additions are
/// silent; a profile value that overrides a base value of a different
/// kind wins with a W003 warning naming the dotted path. `span`/`file`
/// locate the profile block for the warning.
fn merge_profile_value(
    base: &mut indexmap::IndexMap<String, Value>,
    overlay: Value,
    profile: &str,
    path: String,
    span: Option<crate::diag::Span>,
    file: Option<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Value::Table(fields) = overlay else {
        return;
    };
    for (k, v) in fields {
        let child = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        match base.get(&k) {
            None => {
                base.insert(k, v);
            }
            Some(existing) => match (existing, &v) {
                (Value::Table(_), Value::Table(_)) => {
                    if let Some(Value::Table(base_sub)) = base.get_mut(&k) {
                        merge_profile_value(
                            base_sub,
                            v,
                            profile,
                            child,
                            span.clone(),
                            file.clone(),
                            diagnostics,
                        );
                    }
                }
                _ => {
                    if value_kind_name(existing) != value_kind_name(&v) {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            code: "W003".into(),
                            message: format!(
                                "profile '{profile}' overrides '{child}' with a different type"
                            ),
                            path: Some(child),
                            span: span.clone(),
                            repair: None,
                            confidence: 1.0,
                            file: file.clone(),
                        });
                    }
                    base.insert(k, v);
                }
            },
        }
    }
}

/// Outcome of a rename application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameOutcome {
    Ok,
    Missing,
    Exists,
}

/// Navigate to the parent of a path's final segment. With `create`,
/// missing intermediate keys materialize as tables (or arrays when the
/// following segment indexes); without it, navigation is read-only and
/// anything missing returns None. A scalar blocking the way, or an
/// array index that does not exist, always returns None (no sparse
/// extension, no clobbering). Read-only navigation keeps failed
/// remove/rename operations side-effect free (spec §2.9).
fn patch_parent<'a>(root: &'a mut Value, segs: &[PathSeg], create: bool) -> Option<&'a mut Value> {
    let mut cur = root;
    let last = segs.len().checked_sub(1)?;
    let mut i = 0;
    while i < last {
        match &segs[i] {
            PathSeg::Key(k) => {
                let need_array = matches!(segs.get(i + 1), Some(PathSeg::Index(_)));
                cur = match cur {
                    Value::Table(map) => {
                        if !create && !map.contains_key(k) {
                            return None;
                        }
                        map.entry(k.clone()).or_insert_with(|| {
                            if need_array {
                                Value::Array(Vec::new())
                            } else {
                                Value::Table(indexmap::IndexMap::new())
                            }
                        })
                    }
                    _ => return None,
                };
            }
            PathSeg::Index(idx) => {
                cur = match cur {
                    Value::Array(items) => items.get_mut(*idx)?,
                    _ => return None,
                };
            }
        }
        i += 1;
    }
    Some(cur)
}

/// Set the value at `path`, creating missing intermediate tables.
/// Returns false when the path is blocked (scalar in the way) or an
/// array index does not exist.
fn patch_set(root: &mut Value, segs: &[PathSeg], value: Value) -> bool {
    let Some(last) = segs.last() else {
        return false;
    };
    let Some(parent) = patch_parent(root, segs, true) else {
        return false;
    };
    match (parent, last) {
        (Value::Table(map), PathSeg::Key(k)) => {
            map.insert(k.clone(), value);
            true
        }
        (Value::Array(items), PathSeg::Index(i)) => {
            if let Some(slot) = items.get_mut(*i) {
                *slot = value;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Delete the value at `path`. Array removals shift later elements.
/// Returns false when the target does not exist.
fn patch_remove(root: &mut Value, segs: &[PathSeg]) -> bool {
    let Some(last) = segs.last() else {
        return false;
    };
    let Some(parent) = patch_parent(root, segs, false) else {
        return false;
    };
    match (parent, last) {
        (Value::Table(map), PathSeg::Key(k)) => map.shift_remove(k).is_some(),
        (Value::Array(items), PathSeg::Index(i)) if *i < items.len() => {
            items.remove(*i);
            true
        }
        _ => false,
    }
}

/// Rename a key within its parent table. Returns Missing when the source
/// path does not resolve (checked before the target, so a missing
/// source warns W004 even when the target exists), Exists when the new
/// name is taken.
fn patch_rename(root: &mut Value, segs: &[PathSeg], new_name: &str) -> RenameOutcome {
    let Some(last) = segs.last() else {
        return RenameOutcome::Missing;
    };
    // Renaming an array element is meaningless: only plain keys rename.
    if !matches!(last, PathSeg::Key(_)) {
        return RenameOutcome::Missing;
    }
    let Some(parent) = patch_parent(root, segs, false) else {
        return RenameOutcome::Missing;
    };
    let Value::Table(map) = parent else {
        return RenameOutcome::Missing;
    };
    let PathSeg::Key(old) = last else {
        return RenameOutcome::Missing;
    };
    if !map.contains_key(old) {
        return RenameOutcome::Missing;
    }
    if map.contains_key(new_name) {
        return RenameOutcome::Exists;
    }
    match map.shift_remove(old) {
        Some(v) => {
            map.insert(new_name.to_string(), v);
            RenameOutcome::Ok
        }
        None => RenameOutcome::Missing,
    }
}
/// Redaction is recursive — a secret nested inside an array or table is
/// just as sensitive as a top-level one.
fn redacted(v: &Value) -> String {
    match v {
        Value::Secret(_) => "Secret(***)".to_string(),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(redacted).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Table(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, val)| format!("{:?}: {}", k, redacted(val)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        other => format!("{:?}", other),
    }
}

fn normalize_quantity(v: &Value, unit: &Unit) -> Result<Value, String> {
    let num = match v {
        Value::Int(n) => *n as f64,
        Value::Float(f) => *f,
        _ => {
            return Err(format!(
                "quantity requires numeric value, got {}",
                redacted(v)
            ));
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
            "unsupported binary op {:?} on {} and {}",
            op,
            redacted(lhs),
            redacted(rhs)
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
        _ => Err(format!("unsupported unary op {:?} on {}", op, redacted(v))),
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
