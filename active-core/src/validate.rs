//! Schema validation after evaluation (C-031, spec/validation.md).
//!
//! Walks the fully evaluated value tree (post-profile, post-patch)
//! against the schema tree and reports every violation. Validation never
//! echoes evaluated values into messages: kinds and schema-authored
//! literals only, so secrets cannot leak. Strict mode never mutates the
//! tree; resilient mode applies schema-driven repairs after the walk —
//! default fills (W005), key renames (H005), string coercions (H007) —
//! and revalidates each repaired subtree like a written one.

use std::collections::HashMap;

use crate::diag::{thresholds, Diagnostic, Repair, RepairKind, Severity, Span};
use crate::eval::{
    eval_binary, eval_unary, measure_value, value_kind_name, Accountant, BudgetCx, BudgetKind,
    Engine, EvalError,
};
use crate::heal::{best_key_match, coerce_string_to, normalized_levenshtein, sentinel_meaning};
use crate::ir::{AnnotationKind, Module, Node, NodeId, SchemaField, Value};
use crate::parser::PathSeg;
use crate::types::ActiveType;

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
        cx: BudgetCx<'_>,
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
            renames: Vec::new(),
            coercions: Vec::new(),
            deadline: cx.deadline,
            wall_limit: cx.wall_limit,
            ticks: 0,
            account: cx.account,
        };
        validator.validate_node(schema_id, module.root, result, &mut Vec::new())?;
        if self.resilient {
            // Fixpoint: renames expose nested typos, coercions, and
            // missing keys; fills insert values that need validating;
            // coercions replace values that need re-checking. Each
            // repair strictly reduces outstanding work, so the queues
            // drain (wall-clock ticks inside revalidation bound
            // pathological shapes first). The counter is a backstop:
            // leftovers degrade to explicit diagnostics below, never
            // silence.
            for _ in 0..1024 {
                if validator.renames.is_empty()
                    && validator.coercions.is_empty()
                    && validator.fills.is_empty()
                {
                    break;
                }
                validator.apply_renames(result)?;
                validator.apply_coercions(result)?;
                validator.apply_fills(result)?;
            }
            for req in std::mem::take(&mut validator.renames) {
                validator.rename_fallback(&req);
            }
            for req in std::mem::take(&mut validator.coercions) {
                validator.coercion_fallback(&req);
            }
            for fill in std::mem::take(&mut validator.fills) {
                let path = render_path(&fill.segments);
                validator.push(
                    "T004",
                    format!(
                        "schema violation at '{}': missing required key (schema default could not be applied)",
                        display_path(&path)
                    ),
                    &path,
                    (fill.span, fill.file),
                );
            }
        }
        if !validator.diagnostics.is_empty() && !self.resilient {
            // An expired deadline takes precedence over collected
            // violations: the budget error must not be masked by
            // validation output produced past the limit.
            if let Some(end) = cx.deadline {
                if std::time::Instant::now() >= end {
                    return Err(EvalError::budget_exceeded(
                        BudgetKind::WallClock,
                        cx.wall_limit,
                        module,
                        None,
                    ));
                }
            }
            return Err(EvalError::ValidationFailed {
                diagnostics: validator.diagnostics,
            });
        }
        diagnostics.extend(validator.diagnostics);
        Ok(())
    }

    /// Enforce `@constraint` annotations after schema validation
    /// (spec/annotations.md §3, C-034). Each predicate is checked
    /// against the final tree value at its path — `let` bindings fall
    /// back to their evaluated node value (they never export) — in
    /// path order with node-id tie-breaks (HashMap iteration is
    /// random, so ordering is made explicit for determinism). The
    /// predicate itself is re-evaluated against post-transform values
    /// (spec §3): graph results predate patches, profiles, and fills.
    /// Strict fails with everything collected; resilient collects
    /// alongside the tree.
    pub(crate) fn validate_annotations(
        &self,
        module: &Module,
        values: &HashMap<NodeId, Value>,
        result: &Value,
        diagnostics: &mut Vec<Diagnostic>,
        deadline: Option<std::time::Instant>,
        wall_limit: u64,
    ) -> Result<(), EvalError> {
        let mut items: Vec<(NodeId, &crate::ir::Annotation)> = module
            .annotations
            .iter()
            .flat_map(|(id, anns)| anns.iter().map(|a| (*id, a)))
            .collect();
        items.sort_by(|a, b| a.1.path.cmp(&b.1.path).then_with(|| a.0 .0.cmp(&b.0 .0)));
        // `let` bindings never export: only their node values can feed
        // constraints. Key constraints with no final-tree value are
        // skipped (removed keys must not fail on stale values).
        let bindings: std::collections::HashSet<NodeId> =
            module.bindings.values().copied().collect();
        // Post-transform overlay: base values overwritten by walking
        // the final tree alongside the arena (Table by key, Array by
        // index while lengths line up). Predicates evaluate against
        // this, never against pre-patch graph results.
        let mut overlay = values.clone();
        if let Some(root) = module.root {
            overlay_values(module, root, result, &mut overlay);
        }
        let mut local: Vec<Diagnostic> = Vec::new();
        // Material is only needed when a constraint actually fails, so
        // the analysis is lazy (the common case pays nothing).
        let mut material: Option<(crate::secret::SecretProfile, crate::secret::SecretRedactor)> =
            None;
        for (node_id, ann) in items {
            // Wall-clock sampling for long annotation lists.
            if let Some(end) = deadline {
                if std::time::Instant::now() >= end {
                    return Err(EvalError::budget_exceeded(
                        BudgetKind::WallClock,
                        wall_limit,
                        module,
                        Some(node_id),
                    ));
                }
            }
            let AnnotationKind::Constraint { source, expr } = &ann.kind else {
                continue;
            };
            let value = lookup_result_path(result, &ann.path).or_else(|| {
                bindings
                    .contains(&node_id)
                    .then(|| values.get(&node_id))
                    .flatten()
            });
            let Some(value) = value else {
                continue;
            };
            let span = module
                .node_spans
                .get(&node_id)
                .cloned()
                .or(Some(ann.span.clone()));
            let file = module.node_files.get(&node_id).cloned();
            let shown_source = render_source(module, source, *expr);
            match predicate_value(module, &overlay, values, *expr) {
                Some(Value::Bool(true)) => {}
                Some(Value::Bool(false)) => {
                    let (profile, redactor) = material.get_or_insert_with(|| {
                        let profile = crate::secret::SecretProfile::analyze_with_profile(
                            module,
                            self.profile_override.as_deref(),
                        );
                        let redactor = runtime_secret_redactor(values, &profile);
                        (profile, redactor)
                    });
                    // A material actual of any kind renders the marker;
                    // a string that merely contains a known secret is
                    // redacted in place.
                    let actual = if profile.node_is_secret(node_id)
                        || profile.path_is_secret(&ann.path)
                    {
                        crate::secret::MARKER.to_string()
                    } else {
                        render_constrained(value, redactor)
                    };
                    local.push(Diagnostic {
                        severity: Severity::Error,
                        code: "T006".into(),
                        message: format!(
                            "constraint violation at '{}': {shown_source} does not hold (actual: {actual})",
                            display_path(&ann.path),
                        ),
                        path: Some(ann.path.clone()),
                        span,
                        repair: None,
                        confidence: 1.0,
                        file,
                    });
                }
                Some(other) => local.push(Diagnostic {
                    severity: Severity::Error,
                    code: "T006".into(),
                    message: format!(
                        "constraint violation at '{}': {shown_source} must evaluate to a boolean, got {}",
                        display_path(&ann.path),
                        value_kind_name(&other),
                    ),
                    path: Some(ann.path.clone()),
                    span,
                    repair: None,
                    confidence: 1.0,
                    file,
                }),
                // Defensive: exotic predicate roots fall back to the
                // graph result; a missing one cannot happen on
                // cycle-checked graphs.
                None => {}
            }
        }
        if !local.is_empty() && !self.resilient {
            if let Some(end) = deadline {
                if std::time::Instant::now() >= end {
                    return Err(EvalError::budget_exceeded(
                        BudgetKind::WallClock,
                        wall_limit,
                        module,
                        None,
                    ));
                }
            }
            return Err(EvalError::ValidationFailed { diagnostics: local });
        }
        diagnostics.extend(local);
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

/// One resilient key rename: an unexpected closed-table key whose
/// similarity to a schema field clears the auto-apply band, whose
/// value type-checks against that field, and whose target is absent.
/// Applied after the walk (spec/validation.md §10); the renamed
/// subtree is then validated like a written one.
struct RenameRequest {
    segments: Vec<PathSeg>,
    from: String,
    to: String,
    field_id: NodeId,
    value_id: Option<NodeId>,
    span: Option<Span>,
    file: Option<String>,
    confidence: f64,
}

/// One resilient string coercion: a T002 mismatch where the string
/// parses as the target type. Applied after the walk; the coerced
/// value is then constraint-checked like a written one.
struct CoerceRequest {
    segments: Vec<PathSeg>,
    field_id: NodeId,
    value_id: Option<NodeId>,
    coerced: Value,
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
    /// Resilient key renames and string coercions collected during
    /// the walk (and during rename revalidation), applied after it
    /// (spec/validation.md §10). Strict mode records none: it only
    /// attaches suggestions to the violations it reports.
    renames: Vec<RenameRequest>,
    coercions: Vec<CoerceRequest>,
    /// Evaluation deadline for the wall-clock backstop (spec/budgets.md
    /// §2); sampled every 64 validated nodes. `wall_limit` is the
    /// configured milliseconds, carried for the error message.
    deadline: Option<std::time::Instant>,
    wall_limit: u64,
    ticks: u64,
    /// Cumulative materialization account: each default fill charges
    /// its measured size before insertion (many filled defaults of one
    /// large value trip like many aliases do).
    account: &'a mut Accountant,
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
    ) -> Result<(), EvalError> {
        self.tick(schema_id)?;
        match self.module.get(schema_id).cloned() {
            Some(Node::SchemaField { field }) => {
                self.validate_field(schema_id, &field, value_id, value, segments);
            }
            Some(Node::SchemaTable { fields, closed, .. }) => {
                self.validate_table(schema_id, &fields, closed, value_id, value, segments)?;
            }
            Some(Node::SchemaArray { element }) => {
                self.validate_array(schema_id, element, value_id, value, segments)?;
            }
            // A non-schema node is not a contract: nothing to check.
            _ => {}
        }
        Ok(())
    }

    /// Wall-clock sampling hook (spec/budgets.md §2): every 64th
    /// validated node compares against the evaluation deadline.
    fn tick(&mut self, node: NodeId) -> Result<(), EvalError> {
        self.ticks += 1;
        if self.ticks % 64 != 0 {
            return Ok(());
        }
        if let Some(end) = self.deadline {
            if std::time::Instant::now() >= end {
                return Err(EvalError::budget_exceeded(
                    BudgetKind::WallClock,
                    self.wall_limit,
                    self.module,
                    Some(node),
                ));
            }
        }
        Ok(())
    }

    fn validate_table(
        &mut self,
        schema_id: NodeId,
        fields: &indexmap::IndexMap<String, NodeId>,
        closed: bool,
        value_id: Option<NodeId>,
        value: &Value,
        segments: &mut Vec<PathSeg>,
    ) -> Result<(), EvalError> {
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
            return Ok(());
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
                None => self.check_missing(*field_id, segments, map),
                Some(v) => {
                    let child_id = arena_children
                        .as_ref()
                        .and_then(|children| children.get(name).copied());
                    self.validate_node(*field_id, child_id, v, segments)?;
                }
            }
            segments.pop();
        }
        // Extra value keys are ignored in open tables; closed tables
        // reject them (spec/validation.md §10), with phase-2 key
        // repair (C-016): the typo is matched against the schema
        // field names, and under resilient mode a confident,
        // type-checking, collision-free match renames the key instead
        // of failing. Iteration follows value order, so diagnostics
        // stay deterministic.
        if closed {
            let legal: Vec<String> = fields.keys().cloned().collect();
            for (key, value) in map {
                if fields.contains_key(key) {
                    continue;
                }
                // The mode marker is metadata, not data (Phase M): at the
                // document root it never counts as an unexpected key.
                // Nested tables keep the strict rule — a nested
                // `active_mode` is an ordinary key there.
                if key == "active_mode" && segments.is_empty() {
                    continue;
                }
                segments.push(PathSeg::Key(key.clone()));
                let key_path = render_path(segments);
                let key_id = arena_children
                    .as_ref()
                    .and_then(|children| children.get(key).copied());
                let spans = self.spans(key_id, schema_id);
                // Suggestions clear the medium band in both modes
                // (informational only); renames additionally require
                // the high band (C-016 item threshold), checked
                // explicitly below.
                let suggestion = best_key_match(key, &legal, thresholds::MEDIUM);
                // Resilient auto-rename guards (C-016 confidence
                // policy): the match must clear the high band, the
                // target must be absent (never clobber a written key),
                // and the value must already type-check against the
                // suggested field — applied renames are revalidated
                // for constraints afterwards. Shared borrows end here;
                // the push below needs `&mut self`.
                let candidate: Option<(NodeId, bool)> = match &suggestion {
                    Some((suggested, score))
                        if *score >= thresholds::HIGH && map.get(suggested).is_none() =>
                    {
                        match fields.get(suggested) {
                            Some(field_id) => match self.module.get(*field_id) {
                                // Every schema-node kind participates:
                                // fields check their declared type,
                                // tables/arrays check the value shape
                                // (constraints re-run post-rename).
                                Some(Node::SchemaField { field }) => {
                                    Some((*field_id, type_accepts(&field.typ, value)))
                                }
                                Some(Node::SchemaTable { .. }) => {
                                    Some((*field_id, matches!(value, Value::Table(_))))
                                }
                                Some(Node::SchemaArray { .. }) => {
                                    Some((*field_id, matches!(value, Value::Array(_))))
                                }
                                _ => None,
                            },
                            None => None,
                        }
                    }
                    _ => None,
                };
                let mut renamed = false;
                if self.resilient {
                    if let (Some((field_id, true)), Some((suggested, score))) =
                        (candidate, &suggestion)
                    {
                        self.renames.push(RenameRequest {
                            segments: segments.clone(),
                            from: key.clone(),
                            to: suggested.clone(),
                            field_id,
                            value_id: key_id,
                            span: spans.0.clone(),
                            file: spans.1.clone(),
                            confidence: *score,
                        });
                        renamed = true;
                    }
                }
                if !renamed {
                    let mut message = format!(
                        "schema violation at '{}': unexpected key '{key}' in closed table",
                        display_path(&key_path)
                    );
                    let mut repair = None;
                    // Suggest-only confidence is the match score, never
                    // 1.0: tooling auto-applies on confidence, and a
                    // 0.70 suggestion must not look certain.
                    let mut confidence = 1.0;
                    if let Some((suggested, score)) = &suggestion {
                        message.push_str(&format!("; did you mean '{suggested}'?"));
                        // Canonical safe signal (heal.rs): near-identical
                        // edit distance, not the composite score.
                        repair = Some(Repair {
                            original: key.clone(),
                            suggested: suggested.clone(),
                            kind: RepairKind::KeyTypo,
                            safe: normalized_levenshtein(key, suggested) >= thresholds::AUTO_APPLY,
                            side_effects: vec![key.clone(), suggested.clone()],
                            goal: None,
                            steps: Vec::new(),
                        });
                        confidence = *score;
                    }
                    self.push_diag(
                        "T005",
                        message,
                        &key_path,
                        spans,
                        DiagMeta {
                            severity: Severity::Error,
                            repair,
                            confidence,
                        },
                    );
                }
                segments.pop();
            }
        }
        Ok(())
    }

    fn validate_array(
        &mut self,
        schema_id: NodeId,
        element: NodeId,
        value_id: Option<NodeId>,
        value: &Value,
        segments: &mut Vec<PathSeg>,
    ) -> Result<(), EvalError> {
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
            return Ok(());
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
            self.validate_node(element, elem_id, v, segments)?;
            segments.pop();
        }
        Ok(())
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
            // Phase-3 wiring (C-016): a string that parses as the
            // target type is coercible. Strict reports the mismatch
            // with the coercion noted; resilient applies it and
            // constraint-checks the coerced value like a written one.
            // Sentinel words are recognized but never mapped: no
            // schema syntax designates their values, so guessing
            // would invent data (spec/validation.md §10).
            if let Value::String(text) = value {
                if let Some(coerced) = coerce_string_to(text, &field.typ) {
                    let spans = self.spans(value_id, schema_id);
                    if self.resilient {
                        self.coercions.push(CoerceRequest {
                            segments: segments.to_vec(),
                            field_id: schema_id,
                            value_id,
                            coerced,
                            span: spans.0,
                            file: spans.1,
                        });
                    } else {
                        self.push(
                            "T002",
                            format!(
                                "schema violation at '{}': expected {}, got {} (coercible under --resilient)",
                                display_path(&path),
                                field.typ.name(),
                                value_kind_name(value)
                            ),
                            &path,
                            spans,
                        );
                    }
                    return;
                }
                if let Some(intent) = sentinel_meaning(text) {
                    // Echoing the word is safe: `sentinel_meaning`
                    // matched, so the lowercase form is one of a
                    // dozen fixed dictionary words — never user data.
                    self.push(
                        "T002",
                        format!(
                            "schema violation at '{}': expected {}, got {} (sentinel '{}' signals {} but the schema designates no value — set an explicit value or a schema default)",
                            display_path(&path),
                            field.typ.name(),
                            value_kind_name(value),
                            text.to_lowercase(),
                            intent,
                        ),
                        &path,
                        self.spans(value_id, schema_id),
                    );
                    return;
                }
            }
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
    /// A schema field with no value (spec/validation.md section 5 and 9,
    /// spec/schema-extensions.md section 4).
    /// A declared default fills under resilient mode (recorded for
    /// later insertion) but is T004 under strict mode (explicitness) —
    /// the default takes precedence over the Null-absence rule so
    /// `Null(default = …)` behaves like every other defaulted field.
    /// A `requires` target that is present forces T004 naming both paths
    /// unless a resilient default fill satisfies presence.
    /// Anything else is T004, except Null-typed absence (quiet).
    fn check_missing(
        &mut self,
        field_id: NodeId,
        segments: &[PathSeg],
        table: &indexmap::IndexMap<String, Value>,
    ) {
        let (default_arg, is_null, field_span, requires_arg, is_deprecated) =
            match self.module.get(field_id) {
                Some(Node::SchemaField { field }) => (
                    field.default,
                    matches!(field.typ, ActiveType::Null),
                    field.span.clone(),
                    field.requires,
                    field.deprecated.is_some(),
                ),
                _ => (None, false, None, None, false),
            };
        let path = render_path(segments);
        let (span, file) =
            match field_span.or_else(|| self.module.node_spans.get(&field_id).cloned()) {
                Some(s) => (Some(s), self.module.node_files.get(&field_id).cloned()),
                None => (None, None),
            };
        if let Some(requires_id) = requires_arg {
            let target: Option<String> = match self.values.get(&requires_id) {
                Some(Value::String(t)) => Some(t.clone()),
                _ => None,
            };
            if let Some(target) = target {
                let filled = default_arg.is_some() && self.resilient;
                if !filled && table_has(table, &target) {
                    // A computed `requires` target may be material;
                    // echo only a literal spelling.
                    let message = if matches!(
                        self.module.get(requires_id),
                        Some(Node::Literal(Value::String(_)))
                    ) {
                        format!(
                            "schema violation at '{}': missing required key (required by present '{}')",
                            display_path(&path),
                            target,
                        )
                    } else {
                        format!(
                            "schema violation at '{}': missing required key (required by a computed requires target)",
                            display_path(&path),
                        )
                    };
                    self.push("T004", message, &path, (span, file));
                    return;
                }
            }
        }
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
            // Deprecated fields are exempt from strict explicitness:
            // omitting a deprecated key is the migration path, so it is
            // not a violation (resilient still fills the default with
            // W005; explicit use still warns via L005). All other
            // defaulted fields stay strict-explicit (spec/validation.md
            // §9): every value in the tree is written by someone.
            if !is_deprecated {
                self.push(
                    "T004",
                    format!(
                        "schema violation at '{}': missing required key (strict mode requires explicit values despite a schema default)",
                        display_path(&path)
                    ),
                    &path,
                    (span, file),
                );
            }
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
    fn apply_fills(&mut self, result: &mut Value) -> Result<(), EvalError> {
        let fills = std::mem::take(&mut self.fills);
        for mut fill in fills {
            // A rename may have healed this path after the walk queued
            // the fill (or a renamed subtree may already carry the
            // key): present keys are never overwritten by defaults.
            if value_at(result, &fill.segments).is_some() {
                continue;
            }
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
            // Charge the clone before inserting: many fills of one
            // large default must trip like many aliases do.
            let fill_size = measure_value(&default_value);
            self.account.charge(self.module, None, fill_size)?;
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
            )?;
        }
        Ok(())
    }

    /// Apply resilient key renames collected during the walk
    /// (spec/validation.md §10): move each typo'd value onto its
    /// schema field, record H005, and revalidate the renamed subtree
    /// like a written one so constraints still bite. Revalidation can
    /// discover nested typos (a renamed table's own unexpected keys),
    /// which queue further renames; every pass eliminates one
    /// unexpected key and only schema-known targets are introduced,
    /// so the queue drains. The pass counter is a backstop against
    /// pathological shapes, not expected to fire: on overflow,
    /// pending renames degrade to suggest-only T005s so nothing goes
    /// silent.
    fn apply_renames(&mut self, result: &mut Value) -> Result<(), EvalError> {
        let mut passes = 0u32;
        while !self.renames.is_empty() {
            passes += 1;
            if passes > 1024 {
                let pending = std::mem::take(&mut self.renames);
                for req in pending {
                    let RenameRequest {
                        segments,
                        from,
                        to,
                        span,
                        file,
                        confidence,
                        ..
                    } = req;
                    let path = render_path(&segments);
                    let safe = normalized_levenshtein(&from, &to) >= thresholds::AUTO_APPLY;
                    self.push_diag(
                        "T005",
                        format!(
                            "schema violation at '{}': unexpected key '{}' in closed table; did you mean '{}'?",
                            display_path(&path),
                            from,
                            to
                        ),
                        &path,
                        (span, file),
                        DiagMeta {
                            severity: Severity::Error,
                            repair: Some(Repair {
                                original: from.clone(),
                                suggested: to.clone(),
                                kind: RepairKind::KeyTypo,
                                safe,
                                side_effects: vec![from, to],
                                goal: None,
                                steps: Vec::new(),
                            }),
                            confidence,
                        },
                    );
                }
                break;
            }
            let pending = std::mem::take(&mut self.renames);
            for req in pending {
                let Some((last, parent_segs)) = req.segments.split_last() else {
                    continue;
                };
                if !matches!(last, PathSeg::Key(key) if key == &req.from) {
                    continue;
                }
                let Some(table) = value_at_mut(result, parent_segs) else {
                    self.rename_fallback(&req);
                    continue;
                };
                let Value::Table(map) = table else {
                    self.rename_fallback(&req);
                    continue;
                };
                // Recheck at apply time: a sibling rename may have
                // taken the target since the walk (two typos, one
                // field) — first wins, the other degrades.
                if map.contains_key(&req.to) {
                    self.rename_fallback(&req);
                    continue;
                }
                let Some(moved) = map.shift_remove(&req.from) else {
                    self.rename_fallback(&req);
                    continue;
                };
                map.insert(req.to.clone(), moved);
                let mut fixed: Vec<PathSeg> = parent_segs
                    .iter()
                    .cloned()
                    .chain(std::iter::once(PathSeg::Key(req.to.clone())))
                    .collect();
                let fixed_path = render_path(&fixed);
                // The rename satisfies the absence the walk already
                // reported: retract the stale missing-key T004 for
                // this exact path, and cancel any queued default fill
                // for it — the healed value wins over the default
                // (C1: fills applied afterwards must not clobber
                // renames). Other paths are untouched.
                self.diagnostics.retain(|d| {
                    !(d.code == "T004" && d.path.as_deref() == Some(fixed_path.as_str()))
                });
                self.fills
                    .retain(|f| render_path(&f.segments) != fixed_path);
                self.push_diag(
                    "H005",
                    format!(
                        "renamed key '{}' to '{}' (confidence {:.2})",
                        req.from, req.to, req.confidence
                    ),
                    &fixed_path,
                    (req.span.clone(), req.file.clone()),
                    DiagMeta {
                        severity: Severity::Warning,
                        repair: Some(Repair {
                            original: req.from.clone(),
                            suggested: req.to.clone(),
                            kind: RepairKind::KeyTypo,
                            safe: normalized_levenshtein(&req.from, &req.to)
                                >= thresholds::AUTO_APPLY,
                            side_effects: vec![req.from.clone(), req.to.clone()],
                            goal: None,
                            steps: Vec::new(),
                        }),
                        confidence: req.confidence,
                    },
                );
                if let Some(current) = value_at(result, &fixed).cloned() {
                    self.validate_node(req.field_id, req.value_id, &current, &mut fixed)?;
                }
            }
        }
        Ok(())
    }

    /// Suggest-only T005 for a rename that cannot apply (target taken
    /// or subtree vanished): the violation must still be reported,
    /// carrying the original match confidence.
    fn rename_fallback(&mut self, req: &RenameRequest) {
        let path = render_path(&req.segments);
        self.push_diag(
            "T005",
            format!(
                "schema violation at '{}': unexpected key '{}' in closed table; did you mean '{}'?",
                display_path(&path),
                req.from,
                req.to
            ),
            &path,
            (req.span.clone(), req.file.clone()),
            DiagMeta {
                severity: Severity::Error,
                repair: Some(Repair {
                    original: req.from.clone(),
                    suggested: req.to.clone(),
                    kind: RepairKind::KeyTypo,
                    safe: normalized_levenshtein(&req.from, &req.to) >= thresholds::AUTO_APPLY,
                    side_effects: vec![req.from.clone(), req.to.clone()],
                    goal: None,
                    steps: Vec::new(),
                }),
                confidence: req.confidence,
            },
        );
    }

    /// Suggest-only T002 for a coercion that cannot apply (the
    /// subtree vanished): the violation must still be reported.
    fn coercion_fallback(&mut self, req: &CoerceRequest) {
        let path = render_path(&req.segments);
        let expected = match self.module.get(req.field_id) {
            Some(Node::SchemaField { field }) => field.typ.name().to_string(),
            _ => "schema type".to_string(),
        };
        self.push(
            "T002",
            format!(
                "schema violation at '{}': expected {}, got string (coercion could not be applied)",
                display_path(&path),
                expected
            ),
            &path,
            (req.span.clone(), req.file.clone()),
        );
    }

    /// Apply resilient string coercions collected during the walk:
    /// swap in the parsed value, record H007 (value-free, per the
    /// no-echo policy), and revalidate constraints on the coerced
    /// value like a written one. Coerced values are scalars, so no
    /// materialization charge is needed (unlike default fills).
    fn apply_coercions(&mut self, result: &mut Value) -> Result<(), EvalError> {
        let pending = std::mem::take(&mut self.coercions);
        for req in pending {
            if !insert_at(result, &req.segments, req.coerced.clone()) {
                // Defensive (the walk verified the path): re-report
                // as a plain T002 so nothing goes silent.
                self.coercion_fallback(&req);
                continue;
            }
            let path = render_path(&req.segments);
            self.push_diag(
                "H007",
                format!(
                    "resilient: coerced value at '{}' from string to {}",
                    display_path(&path),
                    value_kind_name(&req.coerced)
                ),
                &path,
                (req.span.clone(), req.file.clone()),
                DiagMeta {
                    severity: Severity::Warning,
                    repair: None,
                    confidence: 0.90,
                },
            );
            if let Some(current) = value_at(result, &req.segments).cloned() {
                let mut segs = req.segments.clone();
                self.validate_node(req.field_id, req.value_id, &current, &mut segs)?;
            }
        }
        Ok(())
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
        if let ActiveType::Enum(members) = &field.typ {
            self.check_enum_descriptions(field, schema_id, members, path, spans.clone());
        }
        match (&field.typ, value) {
            (ActiveType::Int, Value::Int(n)) => {
                self.check_int_bounds(field, schema_id, *n, path, spans, true);
            }
            (ActiveType::Float, Value::Float(f)) => {
                self.check_float_bounds(field, schema_id, *f, path, spans);
            }
            (ActiveType::Float, Value::Int(n)) => {
                self.check_float_bounds(field, schema_id, *n as f64, path, spans);
            }
            (ActiveType::Duration, Value::DurationMs(ms)) => {
                self.check_duration_bounds(field, schema_id, *ms, path, spans);
            }
            // Bytes normalize to integers at evaluation (known
            // limitation: plain integers are indistinguishable from
            // byte counts, spec/validation.md §3). Unlike Int fields,
            // Bytes bounds are integers only (spec §4).
            (ActiveType::Bytes, Value::Int(n)) => {
                self.check_int_bounds(field, schema_id, *n, path, spans, false);
            }
            (ActiveType::String, Value::String(s)) => {
                self.check_string_bounds(field, schema_id, s, path, spans);
            }
            (ActiveType::Enum(members), Value::String(s)) if !members.iter().any(|m| m == s) => {
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
            (
                ActiveType::Blob,
                Value::Blob {
                    size, media_type, ..
                },
            ) => {
                self.check_blob_bounds(field, schema_id, *size, media_type, path, spans);
            }
            _ => {}
        }
    }

    /// Blob constraints (spec/blob.md §4, §8): `max_size` breach is
    /// B002, `media_type` mismatch is B004. The payload size is
    /// metadata; a schema bound or media expectation may be an
    /// expression, so each is echoed only when spelled as a literal.
    fn check_blob_bounds(
        &mut self,
        field: &SchemaField,
        schema_id: NodeId,
        size: u64,
        media_type: &str,
        path: &str,
        spans: (Option<Span>, Option<String>),
    ) {
        if let Some(max_id) = field.blob_max_size {
            match self.values.get(&max_id) {
                Some(Value::Int(max)) if *max >= 0 => {
                    if size > *max as u64 {
                        // Echo the bound only when the schema spells a
                        // literal (a computed bound may be material).
                        let message = if matches!(
                            self.module.get(max_id),
                            Some(Node::Literal(Value::Int(_)))
                        ) {
                            format!(
                                "schema violation at '{}': blob size {} exceeds max_size {}",
                                display_path(path),
                                size,
                                max,
                            )
                        } else {
                            format!(
                                "schema violation at '{}': blob size {} exceeds the schema max_size",
                                display_path(path),
                                size,
                            )
                        };
                        self.push(crate::blob::code::TOO_LARGE, message, path, spans.clone());
                    }
                }
                // Schema bug, reported once per field like a bad
                // pattern: max_size must be a non-negative integer.
                _ if self.bug_once(schema_id, "blob_max_size") => {
                    self.violation(path, "invalid max_size in schema", spans.clone());
                }
                _ => {}
            }
        }
        if let Some(media_id) = field.blob_media_type {
            // The expectation itself can be an expression (grammar
            // allows it), so echo it only when the schema spells a
            // literal; a computed value may be material.
            let expected_is_literal = matches!(
                self.module.get(media_id),
                Some(Node::Literal(Value::String(_)))
            );
            match self.values.get(&media_id) {
                Some(Value::String(expected)) => {
                    if media_type != expected {
                        let message = if expected_is_literal {
                            format!(
                                "schema violation at '{}': blob media type does not match expected '{}'",
                                display_path(path),
                                expected,
                            )
                        } else {
                            format!(
                                "schema violation at '{}': blob media type does not match the schema's computed media_type",
                                display_path(path),
                            )
                        };
                        self.push(crate::blob::code::MEDIA_MISMATCH, message, path, spans);
                    }
                }
                // Schema bug, reported once per field: media_type must
                // be a string.
                _ if self.bug_once(schema_id, "blob_media_type") => {
                    self.violation(path, "invalid media_type in schema", spans);
                }
                _ => {}
            }
        }
    }

    /// Enum member descriptions (spec/schema-extensions.md §3): a parallel
    /// string list whose length must equal the member count. Length or
    /// element mismatch is a schema bug reported once per field;
    /// descriptions never affect membership.
    fn check_enum_descriptions(
        &mut self,
        field: &SchemaField,
        field_id: NodeId,
        members: &[String],
        path: &str,
        spans: (Option<Span>, Option<String>),
    ) {
        let Some(desc_id) = field.enum_descriptions else {
            return;
        };
        let ok = match self.values.get(&desc_id) {
            Some(Value::Array(items)) => {
                items.len() == members.len() && items.iter().all(|v| matches!(v, Value::String(_)))
            }
            _ => false,
        };
        if !ok && self.bug_once(field_id, "descriptions") {
            self.violation(
                path,
                &format!(
                    "descriptions length ({}) must match Enum members ({})",
                    match self.values.get(&desc_id) {
                        Some(Value::Array(items)) => items.len().to_string(),
                        _ => "not a string list".to_string(),
                    },
                    members.len()
                ),
                spans,
            );
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
        if let Some(format_id) = field.format {
            self.check_format(field, field_id, format_id, s, path, spans);
        }
    }

    /// Format validators (spec/schema-extensions.md section 1): anchored
    /// full matches for a closed registry of names. Unknown names and
    /// non-string arguments are schema bugs reported once per field.
    fn check_format(
        &mut self,
        field: &SchemaField,
        field_id: NodeId,
        format_id: NodeId,
        s: &str,
        path: &str,
        spans: (Option<Span>, Option<String>),
    ) {
        let _ = field;
        static FORMATS: std::sync::OnceLock<Vec<(String, regex::Regex)>> =
            std::sync::OnceLock::new();
        let formats = FORMATS.get_or_init(|| {
            [
                ("email", r"^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$"),
                ("uri", r"^[a-zA-Z][a-zA-Z0-9+.-]*://[^\s]+$"),
                (
                    "uuid",
                    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
                ),
                (
                    "semver",
                    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$",
                ),
            ]
            .into_iter()
            .map(|(name, pattern)| {
                (
                    name.to_string(),
                    regex::Regex::new(pattern).expect("fixed format"),
                )
            })
            .collect()
        });
        let name: Option<String> = match self.values.get(&format_id) {
            Some(Value::String(name)) => Some(name.clone()),
            _ => None,
        };
        let Some(name) = name else {
            if self.bug_once(field_id, "format") {
                self.violation(path, "incomparable format bound for String fields", spans);
            }
            return;
        };
        match formats.iter().find(|(known, _)| known == &name) {
            None => {
                if self.bug_once(field_id, "format") {
                    // A computed format string may be material; echo
                    // only an authored literal.
                    let message = if matches!(
                        self.module.get(format_id),
                        Some(Node::Literal(Value::String(_)))
                    ) {
                        format!("unknown format '{name}' in schema")
                    } else {
                        "unknown format in schema".to_string()
                    };
                    self.violation(path, &message, spans);
                }
            }
            Some((_, re)) => {
                if !re.is_match(s) {
                    self.violation(path, &format!("does not match format '{name}'"), spans);
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
        self.push_diag(
            code,
            message,
            path,
            spans,
            DiagMeta {
                severity: Severity::Error,
                repair: None,
                confidence: 1.0,
            },
        );
    }

    /// Full-shape diagnostic push: healing codes (H005/H007) and
    /// T005 suggestions carry repairs with confidence scores and
    /// non-error severities; plain violations stay errors without
    /// repairs. The severity/repair/confidence triple travels as one
    /// struct to keep the arity lint-clean.
    fn push_diag(
        &mut self,
        code: &str,
        message: String,
        path: &str,
        spans: (Option<Span>, Option<String>),
        meta: DiagMeta,
    ) {
        let (span, file) = spans;
        self.diagnostics.push(Diagnostic {
            severity: meta.severity,
            code: code.into(),
            message,
            path: Some(path.to_string()),
            span,
            repair: meta.repair,
            confidence: meta.confidence,
            file,
        });
    }
}

/// Severity + repair + confidence for repair-carrying diagnostics.
struct DiagMeta {
    severity: Severity,
    repair: Option<Repair>,
    confidence: f64,
}

/// Root-level violations would render as `at ''`; name the root instead.
fn display_path(path: &str) -> &str {
    if path.is_empty() {
        "<root>"
    } else {
        path
    }
}

/// Look up a dotted annotation path in the final tree. Keys are
/// identifiers by construction, so splitting is exact. Returns None
/// for removed keys, `let` bindings, and profile-scoped paths (the
/// caller falls back to the annotated node's value).
fn lookup_result_path<'v>(result: &'v Value, path: &str) -> Option<&'v Value> {
    let mut cursor = result;
    for segment in path.split('.') {
        match cursor {
            Value::Table(map) => cursor = map.get(segment)?,
            _ => return None,
        }
    }
    Some(cursor)
}

/// Overwrite `overlay` with final-tree values by walking the arena
/// alongside the result (Table by key, Array by index while lengths
/// line up). Keys absent from the final tree (patched away) lose
/// their whole subtree so predicates cannot read stale descendants;
/// length-changed arrays lose theirs for the same reason (positions
/// no longer correspond). `let` bindings keep their evaluated values
/// (they never transform).
fn overlay_values(
    module: &Module,
    node_id: NodeId,
    value: &Value,
    overlay: &mut HashMap<NodeId, Value>,
) {
    overlay.insert(node_id, value.clone());
    match (module.get(node_id), value) {
        (Some(Node::Table { fields }), Value::Table(map)) => {
            for (key, child_id) in fields {
                match map.get(key) {
                    Some(child_value) => overlay_values(module, *child_id, child_value, overlay),
                    None => {
                        remove_subtree(module, *child_id, overlay);
                    }
                }
            }
        }
        (Some(Node::Array { elements }), Value::Array(values))
            if elements.len() == values.len() =>
        {
            for (child_id, child_value) in elements.iter().zip(values.iter()) {
                overlay_values(module, *child_id, child_value, overlay);
            }
        }
        (Some(Node::Array { .. }), Value::Array(_)) => {
            remove_subtree(module, node_id, overlay);
            overlay.insert(node_id, value.clone());
        }
        _ => {}
    }
}

/// Remove a node and every composite descendant from the overlay, so
/// no predicate can observe pre-transform values through a removed
/// (or reshaped) subtree. Expression nodes hold no independent state
/// beyond their children, so Table/Array descent suffices.
fn remove_subtree(module: &Module, node_id: NodeId, overlay: &mut HashMap<NodeId, Value>) {
    overlay.remove(&node_id);
    match module.get(node_id) {
        Some(Node::Table { fields }) => {
            for child_id in fields.values() {
                remove_subtree(module, *child_id, overlay);
            }
        }
        Some(Node::Array { elements }) => {
            for child_id in elements {
                remove_subtree(module, *child_id, overlay);
            }
        }
        _ => {}
    }
}

/// Evaluate a constraint predicate against the post-transform overlay
/// (spec/annotations.md §3). Literals evaluate directly; references
/// read the overlay (final values); operators reuse the main
/// evaluator's pure functions. Anything exotic (interpolation,
/// tables, secrets-as-values) falls back to its graph result via the
/// overlay entry, which the walk above already finalized.
fn predicate_value(
    module: &Module,
    overlay: &HashMap<NodeId, Value>,
    base: &HashMap<NodeId, Value>,
    expr: NodeId,
) -> Option<Value> {
    match module.get(expr) {
        Some(Node::Literal(v)) => Some(v.clone()),
        Some(Node::Ref(target)) => overlay.get(target).cloned(),
        Some(Node::Ident(name)) => module
            .bindings
            .get(name)
            .and_then(|target| overlay.get(target))
            .cloned(),
        Some(Node::Binary { op, lhs, rhs }) => {
            let l = predicate_value(module, overlay, base, *lhs)?;
            let r = predicate_value(module, overlay, base, *rhs)?;
            eval_binary(*op, &l, &r).ok()
        }
        Some(Node::Unary { op, expr }) => {
            let v = predicate_value(module, overlay, base, *expr)?;
            eval_unary(*op, &v).ok()
        }
        // Composite or opaque nodes: their overlay entry already holds
        // the finalized value (walked above for reachable nodes).
        _ => overlay
            .get(&expr)
            .cloned()
            .or_else(|| base.get(&expr).cloned()),
    }
}

/// Render a constraint source for T006: secret-bearing predicates
/// render generic so no locator or literal can leak; every source
/// truncates to 120 characters total. A predicate counts as
/// secret-bearing when a `Secret` node is reachable — following
/// references and `let` bindings, so indirection cannot smuggle
/// secret material into a message either.
fn render_source(module: &Module, source: &str, expr: NodeId) -> String {
    if subtree_has_secret(module, expr) {
        return "constraint".to_string();
    }
    truncate_120(source)
}

/// True when a `Secret` node is reachable from `expr`, following
/// references and `let` bindings as well as composite children
/// (cycle-free DAG walk with a visited set for determinism).
fn subtree_has_secret(module: &Module, expr: NodeId) -> bool {
    let mut stack = vec![expr];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        match module.get(id) {
            None => {}
            Some(Node::Secret { .. }) => return true,
            Some(Node::Ref(target)) => stack.push(*target),
            Some(Node::Ident(name)) => {
                if let Some(target) = module.bindings.get(name) {
                    stack.push(*target);
                }
            }
            Some(Node::Binary { lhs, rhs, .. }) => stack.extend([*lhs, *rhs]),
            Some(Node::Unary { expr, .. }) => stack.push(*expr),
            Some(Node::Array { elements }) => stack.extend(elements.iter().copied()),
            Some(Node::Table { fields }) => stack.extend(fields.values().copied()),
            Some(Node::Interpolate { parts }) => {
                for part in parts {
                    if let crate::ir::InterpPart::Expr(e) = part {
                        stack.push(*e);
                    }
                }
            }
            Some(Node::Semantic { inner, .. }) => stack.push(*inner),
            Some(Node::Env {
                default: Some(def), ..
            }) => stack.push(*def),
            Some(Node::ContentAddressed { media_type, data }) => {
                stack.push(*media_type);
                stack.push(*data);
            }
            _ => {}
        }
    }
    false
}

/// Runtime material for T006: authored literals inside secret locators
/// plus the evaluated text of every structurally sensitive node (an
/// environment lookup is not visible in the module, so its value must
/// come from the walk).
fn runtime_secret_redactor(
    values: &HashMap<NodeId, Value>,
    profile: &crate::secret::SecretProfile,
) -> crate::secret::SecretRedactor {
    let mut redactor = profile.literals().clone();
    for (id, value) in values {
        if profile.node_is_secret(*id) {
            if let Value::String(text) = value {
                redactor.insert(text.clone());
            }
        }
    }
    redactor
}

/// Render an actual value for T006 (spec/annotations.md §6): secrets
/// as `secret`, strings debug-quoted and material-redacted, scalars
/// plain, containers by kind — truncated to 120 characters. Secret
/// material can never reach a message through here.
fn render_constrained(value: &Value, redactor: &crate::secret::SecretRedactor) -> String {
    let text = match value {
        Value::Secret(_) => "secret".to_string(),
        Value::String(s) => match redactor.redact_text(s) {
            Some(redacted) => format!("{redacted:?}"),
            None => format!("{s:?}"),
        },
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::DurationMs(ms) => format!("{ms}ms"),
        Value::Quantity(n, unit) => unit.format_amount(*n),
        Value::Array(_) => "array".to_string(),
        Value::Table(_) => "table".to_string(),
        // Identity only: hashes are metadata, never secret material.
        Value::Blob { hash, size, .. } => {
            format!("blob(blake3:{}, size={size})", crate::blob::hash_hex(hash))
        }
    };
    truncate_120(&text)
}

/// Truncate rendered diagnostic text to 120 characters total
/// (119 content characters plus the ellipsis marker).
fn truncate_120(text: &str) -> String {
    const LIMIT: usize = 120;
    if text.chars().count() > LIMIT {
        format!("{}…", text.chars().take(LIMIT - 1).collect::<String>())
    } else {
        text.to_string()
    }
}

/// Presence-plus-enablement test for `requires` targets
/// (spec/schema-extensions.md section 4): walk dotted segments through
/// nested tables, then apply truthiness. Present-but-disabled
/// (`false`, `""`, `0`, empty table/array, null) does not trigger;
/// a provided secret always counts as enabled.
fn table_has(table: &indexmap::IndexMap<String, Value>, dotted: &str) -> bool {
    let mut current = table;
    let mut parts = dotted.split('.');
    let mut last = match parts.next() {
        Some(first) => first,
        None => return false,
    };
    for part in parts {
        match current.get(last) {
            Some(Value::Table(nested)) => {
                current = nested;
                last = part;
            }
            _ => return false,
        }
    }
    match current.get(last) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Int(n)) => *n != 0,
        Some(Value::Float(f)) => *f != 0.0,
        Some(Value::DurationMs(ms)) => *ms != 0,
        Some(Value::Quantity(v, _)) => *v != 0.0,
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Table(fields)) => !fields.is_empty(),
        Some(Value::Secret(_)) => true,
        // A present blob always counts as enabled (identity present).
        Some(Value::Blob { .. }) => true,
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

/// Read a value at a structured path (the immutable twin of
/// `insert_at` below): used to revalidate renamed and coerced
/// subtrees after resilient repairs move them.
fn value_at<'v>(result: &'v Value, segments: &[PathSeg]) -> Option<&'v Value> {
    let mut cursor = result;
    for segment in segments {
        match (segment, cursor) {
            (PathSeg::Key(key), Value::Table(map)) => cursor = map.get(key)?,
            (PathSeg::Index(i), Value::Array(elements)) => cursor = elements.get(*i)?,
            _ => return None,
        }
    }
    Some(cursor)
}

/// Mutable twin of `value_at`: navigates to a parent container so
/// resilient renames can move keys within it.
fn value_at_mut<'v>(result: &'v mut Value, segments: &[PathSeg]) -> Option<&'v mut Value> {
    let mut cursor = result;
    for segment in segments {
        match (segment, cursor) {
            (PathSeg::Key(key), Value::Table(map)) => cursor = map.get_mut(key)?,
            (PathSeg::Index(i), Value::Array(elements)) => cursor = elements.get_mut(*i)?,
            _ => return None,
        }
    }
    Some(cursor)
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
fn type_accepts(typ: &ActiveType, value: &Value) -> bool {
    use ActiveType::*;
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
            | (Blob, Value::Blob { .. })
            | (Any, _)
            | (Table { .. }, Tab(_))
            | (Enum(_), Value::String(_))
    )
}
