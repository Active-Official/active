//! Schema validation after evaluation (C-031, spec/validation.md).
//!
//! Walks the fully evaluated value tree (post-profile, post-patch)
//! against the schema tree and reports every violation. Validation never
//! mutates the tree and never echoes evaluated values into messages:
//! kinds and schema-authored literals only, so secrets cannot leak.

use std::collections::HashMap;

use crate::diag::{Diagnostic, Severity, Span};
use crate::eval::{value_kind_name, Engine, EvalError};
use crate::ir::{Module, Node, NodeId, SchemaField, Value};
use crate::parser::PathSeg;
use crate::types::SolidType;

impl Engine {
    /// Validate the evaluated result against the document schema.
    /// Runs only when the document declares one; otherwise a no-op.
    /// Strict mode collects every violation and fails; resilient mode
    /// appends them to `diagnostics` and succeeds with the tree as-is.
    pub(crate) fn validate_schema(
        &self,
        module: &Module,
        values: &HashMap<NodeId, Value>,
        result: &mut Value,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(), EvalError> {
        let Some(schema_id) = module.schema else {
            return Ok(());
        };
        let mut validator = Validator {
            module,
            values,
            diagnostics: Vec::new(),
            regexes: HashMap::new(),
            schema_bugs: std::collections::HashSet::new(),
            resilient: self.resilient,
            fills: Vec::new(),
        };
        validator.validate_node(schema_id, module.root, result, &mut Vec::new());
        if self.resilient {
            validator.apply_fills(result);
        }
        if !validator.diagnostics.is_empty() && !self.resilient {
            return Err(EvalError::ValidationFailed {
                diagnostics: validator.diagnostics,
            });
        }
        diagnostics.extend(validator.diagnostics);
        Ok(())
    }
}

/// One resilient default insertion: the structured path of the missing
/// key (table keys and array indices — array elements can hold
/// defaulted fields too), the field that declares it, and the
/// evaluated default expression plus its span for W005.
struct FillRequest {
    segments: Vec<PathSeg>,
    field_id: NodeId,
    default_id: NodeId,
    span: Option<Span>,
    file: Option<String>,
}

struct Validator<'a> {
    module: &'a Module,
    values: &'a HashMap<NodeId, Value>,
    diagnostics: Vec<Diagnostic>,
    /// Compiled patterns by pattern-argument node: `None` means invalid
    /// (reported once at insert). Avoids recompiling per element and
    /// repeating schema-bug diagnostics across array elements.
    regexes: HashMap<NodeId, Option<regex::Regex>>,
    /// Schema-bug diagnostics already reported, keyed by (field node,
    /// argument): incomparable bounds report once per field even when
    /// array elements re-trigger them (spec/validation.md §4).
    schema_bugs: std::collections::HashSet<(NodeId, String)>,
    resilient: bool,
    /// Resilient default insertions collected during the walk, applied
    /// after it (spec/validation.md §9).
    fills: Vec<FillRequest>,
}

impl<'a> Validator<'a> {
    /// Validate one value against one schema node. `value_id` is the
    /// arena node for the value when the result tree still lines up
    /// with it (always true except under profile merges and patches);
    /// it feeds span attribution only, never semantics. `segments` is
    /// the structured path (table keys and array indices) used for
    /// messages, diagnostic paths, and default fill locations.
    fn validate_node(
        &mut self,
        schema_id: NodeId,
        value_id: Option<NodeId>,
        value: &Value,
        segments: &mut Vec<PathSeg>,
    ) {
        match self.module.get(schema_id).cloned() {
            Some(Node::SchemaField { field }) => {
                self.validate_field(schema_id, &field, value_id, value, segments);
            }
            Some(Node::SchemaTable { fields, closed }) => {
                self.validate_table(schema_id, &fields, closed, value_id, value, segments);
            }
            Some(Node::SchemaArray { element }) => {
                self.validate_array(schema_id, element, value_id, value, segments);
            }
            // A non-schema node is not a contract: nothing to check.
            _ => {}
        }
    }

    fn validate_table(
        &mut self,
        schema_id: NodeId,
        fields: &indexmap::IndexMap<String, NodeId>,
        closed: bool,
        value_id: Option<NodeId>,
        value: &Value,
        segments: &mut Vec<PathSeg>,
    ) {
        let Value::Table(map) = value else {
            let path = render_path(segments);
            self.push(
                "T002",
                format!(
                    "schema violation at '{}': expected Table, got {}",
                    display_path(&path),
                    value_kind_name(value)
                ),
                &path,
                self.spans(value_id, schema_id),
            );
            return;
        };
        // The arena child ids behind this value table, for spans. Any
        // key the arena does not know (patched or merged in) validates
        // normally with a schema-side span.
        let arena_children = match value_id.and_then(|id| self.module.get(id)) {
            Some(Node::Table { fields }) => Some(fields.clone()),
            _ => None,
        };
        for (name, field_id) in fields {
            segments.push(PathSeg::Key(name.clone()));
            match map.get(name) {
                None => self.check_missing(*field_id, segments),
                Some(v) => {
                    let child_id = arena_children
                        .as_ref()
                        .and_then(|children| children.get(name).copied());
                    self.validate_node(*field_id, child_id, v, segments);
                }
            }
            segments.pop();
        }
        // Extra value keys are ignored in open tables; closed tables
        // reject them (spec/validation.md §10). Iteration follows value
        // order, so diagnostics stay deterministic.
        if closed {
            for (key, _) in map {
                if fields.contains_key(key) {
                    continue;
                }
                segments.push(PathSeg::Key(key.clone()));
                let key_path = render_path(segments);
                let key_id = arena_children
                    .as_ref()
                    .and_then(|children| children.get(key).copied());
                self.push(
                    "T005",
                    format!(
                        "schema violation at '{}': unexpected key '{key}' in closed table",
                        display_path(&key_path)
                    ),
                    &key_path,
                    self.spans(key_id, schema_id),
                );
                segments.pop();
            }
        }
    }

    fn validate_array(
        &mut self,
        schema_id: NodeId,
        element: NodeId,
        value_id: Option<NodeId>,
        value: &Value,
        segments: &mut Vec<PathSeg>,
    ) {
        let Value::Array(elements) = value else {
            let path = render_path(segments);
            self.push(
                "T002",
                format!(
                    "schema violation at '{}': expected Array, got {}",
                    display_path(&path),
                    value_kind_name(value)
                ),
                &path,
                self.spans(value_id, schema_id),
            );
            return;
        };
        let arena_elements: Option<Vec<NodeId>> = match value_id.and_then(|id| self.module.get(id))
        {
            // Positional mapping is only valid while the lengths line
            // up: a patch that adds or removes elements breaks the
            // correspondence, so the whole array falls back to schema
            // spans (spec/validation.md §8).
            Some(Node::Array { elements: arena }) if arena.len() == elements.len() => {
                Some(arena.clone())
            }
            _ => None,
        };
        for (i, v) in elements.iter().enumerate() {
            // Positional resolution against the arena array: nested
            // arrays and tables attribute to their own level while the
            // lengths line up; patched length changes fall back to
            // schema spans (spec/validation.md §8).
            let elem_id = arena_elements.as_ref().and_then(|els| els.get(i).copied());
            segments.push(PathSeg::Index(i));
            self.validate_node(element, elem_id, v, segments);
            segments.pop();
        }
    }

    fn validate_field(
        &mut self,
        schema_id: NodeId,
        field: &SchemaField,
        value_id: Option<NodeId>,
        value: &Value,
        segments: &mut [PathSeg],
    ) {
        let path = render_path(segments);
        if !type_accepts(&field.typ, value) {
            self.push(
                "T002",
                format!(
                    "schema violation at '{}': expected {}, got {}",
                    display_path(&path),
                    field.typ.name(),
                    value_kind_name(value)
                ),
                &path,
                self.spans(value_id, schema_id),
            );
            return;
        }
        self.check_constraints(schema_id, field, value, &path, value_id);
    }
    /// A schema field with no value (spec/validation.md §5 and §9).
    /// A declared default fills under resilient mode (recorded for
    /// later insertion) but is T004 under strict mode (explicitness) —
    /// the default takes precedence over the Null-absence rule so
    /// `Null(default = …)` behaves like every other defaulted field.
    /// Anything else is T004, except Null-typed absence (quiet).
    fn check_missing(&mut self, field_id: NodeId, segments: &[PathSeg]) {
        let (default_arg, is_null, field_span) = match self.module.get(field_id) {
            Some(Node::SchemaField { field }) => (
                field.default,
                matches!(field.typ, SolidType::Null),
                field.span.clone(),
            ),
            _ => (None, false, None),
        };
        let path = render_path(segments);
        let (span, file) =
            match field_span.or_else(|| self.module.node_spans.get(&field_id).cloned()) {
                Some(s) => (Some(s), self.module.node_files.get(&field_id).cloned()),
                None => (None, None),
            };
        if let Some(default_id) = default_arg {
            if self.resilient {
                self.fills.push(FillRequest {
                    segments: segments.to_vec(),
                    field_id,
                    default_id,
                    span,
                    file,
                });
                return;
            }
            self.push(
                "T004",
                format!(
                    "schema violation at '{}': missing required key (strict mode requires explicit values despite a schema default)",
                    display_path(&path)
                ),
                &path,
                (span, file),
            );
            return;
        }
        if is_null {
            return;
        }
        self.push(
            "T004",
            format!(
                "schema violation at '{}': missing required key (no default)",
                display_path(&path)
            ),
            &path,
            (span, file),
        );
    }

    /// Apply resilient default insertions collected during the walk
    /// (spec/validation.md §9): insert the evaluated default, record
    /// W005, then validate the filled value like a written one so a
    /// mistyped default surfaces as T002/T003 instead of corrupting
    /// the tree silently. Requests run in schema (walk) order. The
    /// filled value attributes to the default expression's span, which
    /// is more actionable than the schema field span.
    fn apply_fills(&mut self, result: &mut Value) {
        let fills = std::mem::take(&mut self.fills);
        for mut fill in fills {
            // Both skips are defensive (the walk verified the parents
            // and evaluation fills the node map), but a missing key
            // must never go silent: report T004 when the default
            // cannot be applied.
            let Some(default_value) = self.values.get(&fill.default_id).cloned() else {
                self.push(
                    "T004",
                    format!(
                        "schema violation at '{}': missing required key (schema default could not be applied)",
                        display_path(&render_path(&fill.segments))
                    ),
                    &render_path(&fill.segments),
                    (fill.span.clone(), fill.file.clone()),
                );
                continue;
            };
            if !insert_at(result, &fill.segments, default_value.clone()) {
                self.push(
                    "T004",
                    format!(
                        "schema violation at '{}': missing required key (schema default could not be applied)",
                        display_path(&render_path(&fill.segments))
                    ),
                    &render_path(&fill.segments),
                    (fill.span.clone(), fill.file.clone()),
                );
                continue;
            };
            let path = render_path(&fill.segments);
            self.diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                code: "W005".into(),
                message: format!(
                    "filled missing key '{}' from schema default",
                    display_path(&path)
                ),
                path: Some(path),
                span: fill.span,
                repair: None,
                confidence: 1.0,
                file: fill.file,
            });
            self.validate_node(
                fill.field_id,
                Some(fill.default_id),
                &default_value,
                &mut fill.segments,
            );
        }
    }

    fn check_constraints(
        &mut self,
        schema_id: NodeId,
        field: &SchemaField,
        value: &Value,
        path: &str,
        value_id: Option<NodeId>,
    ) {
        let spans = self.spans(value_id, schema_id);
        match (&field.typ, value) {
            (SolidType::Int, Value::Int(n)) => {
                self.check_int_bounds(field, schema_id, *n, path, spans, true);
            }
            (SolidType::Float, Value::Float(f)) => {
                self.check_float_bounds(field, schema_id, *f, path, spans);
            }
            (SolidType::Float, Value::Int(n)) => {
                self.check_float_bounds(field, schema_id, *n as f64, path, spans);
            }
            (SolidType::Duration, Value::DurationMs(ms)) => {
                self.check_duration_bounds(field, schema_id, *ms, path, spans);
            }
            // Bytes normalize to integers at evaluation (known
            // limitation: plain integers are indistinguishable from
            // byte counts, spec/validation.md §3). Unlike Int fields,
            // Bytes bounds are integers only (spec §4).
            (SolidType::Bytes, Value::Int(n)) => {
                self.check_int_bounds(field, schema_id, *n, path, spans, false);
            }
            (SolidType::String, Value::String(s)) => {
                self.check_string_bounds(field, schema_id, s, path, spans);
            }
            (SolidType::Enum(members), Value::String(s)) if !members.iter().any(|m| m == s) => {
                let quoted: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
                self.push(
                    "T003",
                    format!(
                        "schema violation at '{}': not a member of Enum({})",
                        display_path(path),
                        quoted.join(", ")
                    ),
                    path,
                    spans,
                );
            }
            _ => {}
        }
    }

    fn check_int_bounds(
        &mut self,
        field: &SchemaField,
        field_id: NodeId,
        n: i64,
        path: &str,
        spans: (Option<Span>, Option<String>),
        allow_float_bounds: bool,
    ) {
        for (arg, name) in [(field.min, "min"), (field.max, "max")] {
            let Some(arg_id) = arg else { continue };
            let Some(bound) = self.values.get(&arg_id) else {
                continue;
            };
            let (numerically_less, numerically_greater) = match bound {
                Value::Int(b) => (n < *b, n > *b),
                Value::Float(b) if allow_float_bounds => ((n as f64) < *b, (n as f64) > *b),
                _ => {
                    // A broken bound reports once per field even when
                    // array elements re-trigger it (spec §4).
                    if self.bug_once(field_id, name) {
                        self.violation(
                            path,
                            &format!("incomparable {name} bound for {} fields", field.typ.name()),
                            spans.clone(),
                        );
                    }
                    continue;
                }
            };
            if (name == "min" && numerically_less) || (name == "max" && numerically_greater) {
                self.violation(
                    path,
                    if name == "min" {
                        "below minimum"
                    } else {
                        "above maximum"
                    },
                    spans.clone(),
                );
            }
        }
    }

    /// Schema-bug diagnostics report once per (field, argument): the walk
    /// may reach the same broken constraint through many array elements.
    /// Returns true on first occurrence (caller reports), false after.
    fn bug_once(&mut self, field_id: NodeId, arg: &str) -> bool {
        self.schema_bugs.insert((field_id, arg.to_string()))
    }

    fn check_float_bounds(
        &mut self,
        field: &SchemaField,
        field_id: NodeId,
        f: f64,
        path: &str,
        spans: (Option<Span>, Option<String>),
    ) {
        for (arg, name) in [(field.min, "min"), (field.max, "max")] {
            let Some(arg_id) = arg else { continue };
            let Some(bound) = self.values.get(&arg_id) else {
                continue;
            };
            let bound = match bound {
                Value::Int(n) => *n as f64,
                Value::Float(x) => *x,
                _ => {
                    if self.bug_once(field_id, name) {
                        self.violation(
                            path,
                            &format!("incomparable {name} bound for Float fields"),
                            spans.clone(),
                        );
                    }
                    continue;
                }
            };
            if (name == "min" && f < bound) || (name == "max" && f > bound) {
                self.violation(
                    path,
                    if name == "min" {
                        "below minimum"
                    } else {
                        "above maximum"
                    },
                    spans.clone(),
                );
            }
        }
    }

    fn check_duration_bounds(
        &mut self,
        field: &SchemaField,
        field_id: NodeId,
        ms: u64,
        path: &str,
        spans: (Option<Span>, Option<String>),
    ) {
        for (arg, name) in [(field.min, "min"), (field.max, "max")] {
            let Some(arg_id) = arg else { continue };
            let Some(bound) = self.values.get(&arg_id) else {
                continue;
            };
            let bound = match bound {
                Value::DurationMs(b) => *b,
                // Plain integers read as milliseconds.
                Value::Int(b) if *b >= 0 => *b as u64,
                _ => {
                    if self.bug_once(field_id, name) {
                        self.violation(
                            path,
                            &format!("incomparable {name} bound for Duration fields"),
                            spans.clone(),
                        );
                    }
                    continue;
                }
            };
            if (name == "min" && ms < bound) || (name == "max" && ms > bound) {
                self.violation(
                    path,
                    if name == "min" {
                        "below minimum"
                    } else {
                        "above maximum"
                    },
                    spans.clone(),
                );
            }
        }
    }

    fn check_string_bounds(
        &mut self,
        field: &SchemaField,
        field_id: NodeId,
        s: &str,
        path: &str,
        spans: (Option<Span>, Option<String>),
    ) {
        let len = s.chars().count();
        for (arg, name, detail) in [
            (field.min_len, "min_len", "too short"),
            (field.max_len, "max_len", "too long"),
        ] {
            let Some(arg_id) = arg else { continue };
            let Some(bound) = self.values.get(&arg_id) else {
                continue;
            };
            match bound {
                Value::Int(n) => {
                    // A negative limit is a nonsense schema, not a
                    // violation: clamp it so every string passes.
                    let limit = (*n).max(0) as usize;
                    if (name == "min_len" && len < limit) || (name == "max_len" && len > limit) {
                        self.violation(path, detail, spans.clone());
                    }
                }
                _ => {
                    if self.bug_once(field_id, name) {
                        self.violation(
                            path,
                            &format!("incomparable {name} bound for String fields"),
                            spans.clone(),
                        );
                    }
                }
            }
        }
        if let Some(pattern_id) = field.pattern {
            let owned_pattern: Option<String> = match self.values.get(&pattern_id) {
                Some(Value::String(pattern)) => Some(pattern.clone()),
                Some(_) => {
                    if self.bug_once(field_id, "pattern") {
                        self.violation(
                            path,
                            "incomparable pattern bound for String fields",
                            spans.clone(),
                        );
                    }
                    None
                }
                None => None,
            };
            if let Some(pattern) = owned_pattern {
                // Compile once per pattern node: array elements share
                // the result, and an invalid pattern reports once.
                let cached = self.regexes.get(&pattern_id).cloned();
                let compiled = match cached {
                    Some(result) => result,
                    None => {
                        let result = regex::Regex::new(&pattern).ok();
                        if result.is_none() && self.bug_once(field_id, "pattern") {
                            self.violation(path, "invalid pattern in schema", spans.clone());
                        }
                        self.regexes.insert(pattern_id, result.clone());
                        result
                    }
                };
                if let Some(re) = compiled {
                    if !re.is_match(s) {
                        self.violation(path, "does not match pattern", spans.clone());
                    }
                }
            }
        }
    }

    /// Value span when the result tree still lines up with the arena
    /// (the common case), else the schema span, else none — each with
    /// its file (spec/validation.md §8).
    fn spans(&self, value_id: Option<NodeId>, schema_id: NodeId) -> (Option<Span>, Option<String>) {
        if let Some(id) = value_id {
            if let Some(span) = self.module.node_spans.get(&id).cloned() {
                return (Some(span), self.module.node_files.get(&id).cloned());
            }
        }
        (
            self.module.node_spans.get(&schema_id).cloned(),
            self.module.node_files.get(&schema_id).cloned(),
        )
    }

    fn violation(&mut self, path: &str, detail: &str, spans: (Option<Span>, Option<String>)) {
        self.push(
            "T003",
            format!("schema violation at '{}': {detail}", display_path(path)),
            path,
            spans,
        );
    }

    fn push(
        &mut self,
        code: &str,
        message: String,
        path: &str,
        spans: (Option<Span>, Option<String>),
    ) {
        let (span, file) = spans;
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            message,
            path: Some(path.to_string()),
            span,
            repair: None,
            confidence: 1.0,
            file,
        });
    }
}

/// Root-level violations would render as `at ''`; name the root instead.
fn display_path(path: &str) -> &str {
    if path.is_empty() {
        "<root>"
    } else {
        path
    }
}

/// Render structured walk segments in patch-path format (`a.b[0].c`),
/// the same shape everywhere: messages, diagnostic paths, and fills.
fn render_path(segments: &[PathSeg]) -> String {
    let mut out = String::new();
    for segment in segments {
        match segment {
            PathSeg::Key(key) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(key);
            }
            PathSeg::Index(i) => {
                out.push_str(&format!("[{i}]"));
            }
        }
    }
    out
}

/// Insert a filled default at a structured path, descending tables by
/// key and arrays by index. Only existing containers are traversed:
/// the walk verified every parent, so a miss is defensive (patches
/// share no guarantees). The final segment is always a table key —
/// array slots are positional, never "missing" — anything else fails.
fn insert_at(result: &mut Value, segments: &[PathSeg], value: Value) -> bool {
    let Some((first, rest)) = segments.split_first() else {
        return false;
    };
    if rest.is_empty() {
        if let (PathSeg::Key(key), Value::Table(map)) = (first, result) {
            map.insert(key.clone(), value);
            return true;
        }
        return false;
    }
    match (first, result) {
        (PathSeg::Key(key), Value::Table(map)) => match map.get_mut(key) {
            Some(next) => insert_at(next, rest, value),
            None => false,
        },
        (PathSeg::Index(i), Value::Array(elements)) => match elements.get_mut(*i) {
            Some(next) => insert_at(next, rest, value),
            None => false,
        },
        _ => false,
    }
}

/// The type side of T002 acceptance (spec/validation.md §3). `Int`
/// widens to `Float`; every other pair is exact. Bytes arrive as
/// integers (evaluation erases byte-ness); non-string Enums fail here
/// while wrong-member strings fail as T003.
fn type_accepts(typ: &SolidType, value: &Value) -> bool {
    use SolidType::*;
    use Value::Table as Tab;
    matches!(
        (typ, value),
        (Null, Value::Null)
            | (Bool, Value::Bool(_))
            | (Int, Value::Int(_))
            | (Float, Value::Float(_))
            | (Float, Value::Int(_))
            | (String, Value::String(_))
            | (Duration, Value::DurationMs(_))
            | (Bytes, Value::Int(_))
            | (Secret, Value::Secret(_))
            | (Any, _)
            | (Table { .. }, Tab(_))
            | (Enum(_), Value::String(_))
    )
}
