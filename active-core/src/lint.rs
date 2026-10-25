//! Linter rules (C-042, spec/linter.md).
//!
//! Static checks beyond validation: hygiene, probable mistakes, and
//! style risks. Every rule is independently disableable, emits stable
//! `L0xx` Warning diagnostics the language server surfaces as-is, and
//! never affects evaluation — linting reads the module only.

use std::collections::{HashMap, HashSet};

use crate::diag::{Diagnostic, Repair, RepairKind, Severity};
use crate::ir::{Module, Node, NodeId, Value};

/// Rule IDs, shared by CLI flags, options, and tests.
pub const RULE_UNUSED_LETS: &str = "unused-lets";
pub const RULE_DEPENDS_ON: &str = "depends-on";
pub const RULE_COMPLEXITY: &str = "complexity";
pub const RULE_HARDCODED_SECRETS: &str = "hardcoded-secrets";
pub const RULE_DEPRECATED: &str = "deprecated";
pub const RULE_SCHEMA_EVOLUTION: &str = "schema-evolution";
pub const RULE_UNKNOWN_CAPABILITY: &str = "unknown-capability";
pub const RULE_KEY_STYLE: &str = "key-style";
pub const RULE_LARGE_LITERAL: &str = "large-literal";
pub const RULE_DATA_MODE: &str = "data-mode";

/// All rule IDs in stable order (also the CLI's unknown-name error).
pub const ALL_RULES: &[&str] = &[
    RULE_UNUSED_LETS,
    RULE_DEPENDS_ON,
    RULE_COMPLEXITY,
    RULE_HARDCODED_SECRETS,
    RULE_DEPRECATED,
    RULE_SCHEMA_EVOLUTION,
    RULE_UNKNOWN_CAPABILITY,
    RULE_KEY_STYLE,
    RULE_LARGE_LITERAL,
    RULE_DATA_MODE,
];

/// Expression operator nodes that trip the complexity budget.
const COMPLEXITY_LIMIT: u64 = 100;

/// Shannon entropy floor (bits/char) and minimum length for the
/// content-based hard-coded-secret signal. Tuned for precision: with
/// no per-file suppression yet, false positives hurt more than misses
/// (spec/linter.md §1).
const ENTROPY_BITS: f64 = 4.5;
const ENTROPY_MIN_LEN: usize = 20;

/// Linter options: rules in `disabled` do not run.
#[derive(Debug, Clone, Default)]
pub struct LintOptions {
    pub disabled: HashSet<String>,
}

impl LintOptions {
    /// Fails on the first unknown rule name, listing the valid ones.
    pub fn with_disabled(names: &[String]) -> Result<Self, String> {
        let mut disabled = HashSet::new();
        for name in names {
            if !ALL_RULES.contains(&name.as_str()) {
                return Err(format!(
                    "unknown lint rule `{name}` (expected one of: {})",
                    ALL_RULES.join(", ")
                ));
            }
            disabled.insert(name.clone());
        }
        Ok(Self { disabled })
    }

    fn enabled(&self, rule: &str) -> bool {
        !self.disabled.contains(rule)
    }
}

/// Run all enabled rules in stable rule order. Deterministic for a
/// fixed input: every rule walks source-ordered structures (table
/// fields are `IndexMap`; bindings sort by name).
pub fn lint(module: &Module) -> Vec<Diagnostic> {
    lint_with(module, &LintOptions::default())
}

/// Run the enabled subset of rules.
pub fn lint_with(module: &Module, options: &LintOptions) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if options.enabled(RULE_UNUSED_LETS) {
        out.extend(unused_lets(module));
    }
    if options.enabled(RULE_DEPENDS_ON) {
        out.extend(depends_on(module));
    }
    if options.enabled(RULE_COMPLEXITY) {
        out.extend(complexity(module));
    }
    if options.enabled(RULE_HARDCODED_SECRETS) {
        out.extend(hardcoded_secrets(module));
    }
    if options.enabled(RULE_DEPRECATED) {
        out.extend(deprecated(module));
    }
    if options.enabled(RULE_SCHEMA_EVOLUTION) {
        out.extend(schema_evolution(module));
    }
    if options.enabled(RULE_UNKNOWN_CAPABILITY) {
        out.extend(unknown_capability(module));
    }
    if options.enabled(RULE_KEY_STYLE) {
        out.extend(key_style(module));
    }
    if options.enabled(RULE_LARGE_LITERAL) {
        out.extend(large_literal(module));
    }
    if options.enabled(RULE_DATA_MODE) {
        out.extend(data_mode(module));
    }
    out
}

/// Every table key longer than this is flagged as a content-addressing
/// opportunity (L009).
const LARGE_LITERAL_BYTES: usize = 65_536;

/// Capabilities the shipped descriptor advertises. Mirrors
/// `convert::capability` and the descriptor; a test pins them equal.
pub const KNOWN_CAPABILITIES: &[&str] = &[
    "check",
    "eval",
    "format",
    "lint",
    "convert",
    "graph",
    "generate",
    "schema-introspect",
    "explain",
    "patch",
    "repair",
    "compatibility",
    "capability",
];

/// Node → dotted value path map (tables win ties, like resolution).
fn value_path_map(module: &Module) -> HashMap<NodeId, String> {
    let mut paths: HashMap<NodeId, String> = HashMap::new();
    for (name, id) in &module.bindings {
        paths.insert(*id, name.clone());
    }
    for (id, path) in module.value_paths() {
        paths.insert(id, path);
    }
    paths
}

/// L006: partial evolution metadata without a `version`. A schema with
/// no evolution metadata at all is allowed — many contracts never
/// negotiate; mixing `min_reader`/`min_writer`/`removed_in` with no
/// version is the inconsistency (spec/schema-evolution.md).
fn schema_evolution(module: &Module) -> Vec<Diagnostic> {
    let Some(root) = module.schema else {
        return Vec::new();
    };
    let Some(Node::SchemaTable { fields, meta, .. }) = module.get(root) else {
        return Vec::new();
    };
    let has_removed = fields.values().any(|id| {
        matches!(module.get(*id), Some(Node::SchemaField { field }) if field.removed_in.is_some())
    });
    let partial = meta.min_reader.is_some() || meta.min_writer.is_some() || has_removed;
    if partial && meta.version.is_none() {
        return vec![warning(
            module,
            "L006",
            "schema carries reader/writer or removal metadata but no version; \
             add version = \"x.y.z\" so compatibility can be checked"
                .to_string(),
            Some("schema".to_string()),
            Some(root),
        )];
    }
    Vec::new()
}

/// L007: a declared capability the shipped descriptor does not
/// advertise (a typo or an unimplemented host would be refused at the
/// handshake anyway; catching it source-side is cheaper).
fn unknown_capability(module: &Module) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for capability in &module.capabilities {
        if !KNOWN_CAPABILITIES.contains(&capability.as_str()) {
            out.push(warning(
                module,
                "L007",
                format!("unknown capability '{capability}' (not advertised by the descriptor)"),
                Some("active_capabilities".to_string()),
                None,
            ));
        }
    }
    out
}

/// L008: one table mixes camel/Pascal-case keys with snake_case keys.
/// Kebab keys are not legal identifiers, so the observable style split
/// is case-based (grammar §1.3: ident chars are letters, digits, `_`).
fn key_style(module: &Module) -> Vec<Diagnostic> {
    let paths = value_path_map(module);
    let mut out = Vec::new();
    for (index, node) in module.arena.iter().enumerate() {
        let Node::Table { fields } = node else {
            continue;
        };
        if fields.len() < 2 {
            continue;
        }
        let mut cased = false;
        let mut snake = false;
        for key in fields.keys() {
            if key.chars().any(|c| c.is_uppercase()) {
                cased = true;
            } else if key.contains('_') {
                snake = true;
            }
        }
        if cased && snake {
            let id = NodeId(index as u32);
            let path = paths.get(&id).cloned().unwrap_or_default();
            out.push(warning(
                module,
                "L008",
                format!(
                    "table '{}' mixes cased and snake_case keys",
                    if path.is_empty() { "(root)" } else { &path }
                ),
                Some(path),
                Some(id),
            ));
        }
    }
    out
}

/// L009: a string literal large enough that content addressing would
/// pay for itself (identity, dedupe, lazy materialisation).
fn large_literal(module: &Module) -> Vec<Diagnostic> {
    let paths = value_path_map(module);
    let mut out = Vec::new();
    for (index, node) in module.arena.iter().enumerate() {
        let Node::Literal(Value::String(text)) = node else {
            continue;
        };
        if text.len() <= LARGE_LITERAL_BYTES {
            continue;
        }
        let id = NodeId(index as u32);
        let path = paths.get(&id).cloned().unwrap_or_default();
        out.push(warning(
            module,
            "L009",
            format!(
                "string literal at '{}' is {} bytes; consider blob() content addressing",
                if path.is_empty() { "(inline)" } else { &path },
                text.len()
            ),
            Some(path),
            Some(id),
        ));
    }
    out
}

/// L010: Data-oriented surface (content-addressed values, evolution
/// metadata, capability declarations) without the Data Mode marker.
fn data_mode(module: &Module) -> Vec<Diagnostic> {
    if module.mode == crate::ir::Mode::Data {
        return Vec::new();
    }
    let has_ca = module
        .arena
        .iter()
        .any(|node| matches!(node, Node::ContentAddressed { .. }));
    let has_evolution = module.schema.is_some_and(|root| {
        matches!(
            module.get(root),
            Some(Node::SchemaTable { fields, meta, .. })
                if meta.version.is_some()
                    || meta.min_reader.is_some()
                    || meta.min_writer.is_some()
                    || fields.values().any(|id| matches!(
                        module.get(*id),
                        Some(Node::SchemaField { field }) if field.removed_in.is_some()
                    ))
        )
    });
    if !has_ca && !has_evolution && module.capabilities.is_empty() {
        return Vec::new();
    }
    vec![warning(
        module,
        "L010",
        "Data-oriented constructs without active_mode = \"data\"".to_string(),
        Some("active_mode".to_string()),
        module.root,
    )]
}

fn warning(
    module: &Module,
    code: &str,
    message: String,
    path: Option<String>,
    node: Option<NodeId>,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        code: code.into(),
        message,
        path,
        span: node.and_then(|id| module.node_spans.get(&id).cloned()),
        repair: None,
        confidence: 1.0,
        file: node.and_then(|id| module.node_files.get(&id).cloned()),
    }
}

/// Every `Ref` target reachable from `id`, including through nested
/// tables and arrays. Iterative: reference chains cannot recurse the
/// stack.
fn ref_targets(module: &Module, id: NodeId) -> HashSet<NodeId> {
    let mut targets = HashSet::new();
    let mut stack = vec![id];
    let mut seen = HashSet::new();
    while let Some(nid) = stack.pop() {
        if !seen.insert(nid) {
            continue;
        }
        match module.get(nid) {
            Some(Node::Ref(target)) => {
                targets.insert(*target);
            }
            Some(Node::Table { fields }) => {
                stack.extend(fields.values().copied());
            }
            Some(Node::Array { elements }) => {
                stack.extend(elements.iter().copied());
            }
            Some(Node::Binary { lhs, rhs, .. }) => {
                stack.push(*lhs);
                stack.push(*rhs);
            }
            Some(Node::Unary { expr, .. }) => {
                stack.push(*expr);
            }
            Some(Node::Interpolate { parts }) => {
                for part in parts {
                    if let crate::ir::InterpPart::Expr(e) = part {
                        stack.push(*e);
                    }
                }
            }
            Some(Node::Semantic { inner, .. }) => {
                stack.push(*inner);
            }
            Some(Node::Env {
                default: Some(def), ..
            }) => {
                stack.push(*def);
            }
            Some(Node::Secret { locator }) => {
                stack.push(*locator);
            }
            Some(Node::ContentAddressed { media_type, data }) => {
                stack.push(*media_type);
                stack.push(*data);
            }
            _ => {}
        }
    }
    targets
}

/// L001: `let` bindings never referenced by any `Ref`.
fn unused_lets(module: &Module) -> Vec<Diagnostic> {
    // Single flat scan: every Ref node in the arena contributes its
    // target. The old code unioned `ref_targets(id)` over every node —
    // O(V·E) re-walks of the same subgraphs. The sets are identical:
    // each arena node is reachable from itself, so the union over all
    // nodes is exactly the set of all Ref targets (C-055 perf audit).
    let mut used = HashSet::new();
    for node in module.arena.iter() {
        if let Node::Ref(target) = node {
            used.insert(*target);
        }
    }
    // Bindings sort by name: HashMap iteration is random, diagnostics
    // must not be.
    let mut names: Vec<&String> = module.bindings.keys().collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let id = module.bindings[name];
        if !used.contains(&id) {
            out.push(warning(
                module,
                "L001",
                format!("unused let binding '{name}'"),
                Some(name.clone()),
                Some(id),
            ));
        }
    }
    out
}

/// L002: incomplete, dangling, or self-scoped `depends_on` sets.
fn depends_on(module: &Module) -> Vec<Diagnostic> {
    let paths = module.value_paths();
    // Scope path per depends_on table, by table node id.
    let mut scopes: HashMap<NodeId, String> = HashMap::new();
    if let Some(root) = module.root {
        let mut stack = vec![(root, String::new())];
        while let Some((id, path)) = stack.pop() {
            if module.depends_on.contains_key(&id) {
                scopes.insert(id, path.clone());
            }
            if let Some(Node::Table { fields }) = module.get(id) {
                for (key, child) in fields {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    stack.push((*child, child_path));
                }
            }
        }
    }
    // Deterministic: sort scopes by path (HashMap order is random).
    let mut entries: Vec<(NodeId, String)> = scopes.into_iter().collect();
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    let mut out = Vec::new();
    for (table_id, scope_path) in entries {
        let dep = &module.depends_on[&table_id];
        let declared: HashSet<&str> = dep.paths.iter().map(String::as_str).collect();
        // Referenced names/paths inside the scope, mapped through the
        // reverse index; unmapped nodes are skipped, never guessed.
        let mut referenced: HashSet<String> = HashSet::new();
        if let Some(Node::Table { fields }) = module.get(table_id) {
            for child in fields.values() {
                for target in ref_targets(module, *child) {
                    if let Some(name) = paths.get(&target) {
                        referenced.insert(name.clone());
                    }
                }
            }
        }
        let span_node = Some(table_id);
        if dep.paths.is_empty() {
            if !referenced.is_empty() {
                let mut missing: Vec<&String> = referenced.iter().collect();
                missing.sort();
                for name in missing {
                    out.push(missing_entry(module, &scope_path, span_node, name));
                }
            }
            continue;
        }
        for path in &dep.paths {
            if path == &scope_path
                || (!scope_path.is_empty() && path.starts_with(&format!("{scope_path}.")))
            {
                out.push(warning(
                    module,
                    "L002",
                    format!("unnecessary depends_on entry '{path}' (own scope)"),
                    Some(scope_path.clone()),
                    span_node,
                ));
            } else if module.resolve_value_path(path).is_none() {
                out.push(warning(
                    module,
                    "L002",
                    format!("dangling depends_on path '{path}'"),
                    Some(scope_path.clone()),
                    span_node,
                ));
            }
        }
        let mut missing: Vec<&String> = referenced
            .iter()
            .filter(|name| !declared.contains(name.as_str()))
            .collect();
        missing.sort();
        for name in missing {
            out.push(missing_entry(module, &scope_path, span_node, name));
        }
    }
    out
}

/// An L002 missing-entry diagnostic: human prose plus a machine
/// repair carrying the missing path (empty original = insertion),
/// so tooling never parses the message.
fn missing_entry(
    module: &Module,
    scope_path: &str,
    span_node: Option<NodeId>,
    name: &str,
) -> Diagnostic {
    let mut diag = warning(
        module,
        "L002",
        format!("missing depends_on entry for '{name}'"),
        Some(scope_path.to_string()),
        span_node,
    );
    diag.repair = Some(Repair {
        original: String::new(),
        suggested: name.to_string(),
        kind: RepairKind::Other("add_depends_on".to_string()),
        safe: true,
        side_effects: Vec::new(),
        goal: None,
        steps: Vec::new(),
    });
    diag
}

/// Count operator nodes in a value subtree (spec/linter.md §1).
/// Data nodes are free but descended: a big inline table under a key
/// is that key's complexity.
fn operator_count(module: &Module, id: NodeId) -> u64 {
    let mut count = 0u64;
    let mut stack = vec![id];
    let mut seen = HashSet::new();
    while let Some(nid) = stack.pop() {
        if !seen.insert(nid) {
            continue;
        }
        match module.get(nid) {
            Some(Node::Binary { lhs, rhs, .. }) => {
                count += 1;
                stack.push(*lhs);
                stack.push(*rhs);
            }
            Some(Node::Unary { expr, .. }) => {
                count += 1;
                stack.push(*expr);
            }
            Some(Node::Interpolate { parts }) => {
                for part in parts {
                    if let crate::ir::InterpPart::Expr(e) = part {
                        count += 1;
                        stack.push(*e);
                    }
                }
            }
            Some(Node::Ref(_)) | Some(Node::Ident(_)) => {
                count += 1;
            }
            Some(Node::Env { default, .. }) => {
                // Environment defaults are expressions too: a large
                // default must not bypass the budget.
                count += 1;
                if let Some(def) = default {
                    stack.push(*def);
                }
            }
            Some(Node::Secret { locator }) => {
                // Secret locators are expressions with the same rule.
                count += 1;
                stack.push(*locator);
            }
            Some(Node::Table { fields }) => {
                stack.extend(fields.values().copied());
            }
            Some(Node::Array { elements }) => {
                stack.extend(elements.iter().copied());
            }
            Some(Node::Semantic { inner, .. }) => {
                stack.push(*inner);
            }
            _ => {}
        }
    }
    count
}

/// L003: per-statement operator budgets. Roots are key values and
/// `let` bindings (bindings sort by name; table fields keep source
/// order — both deterministic).
fn complexity(module: &Module) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let check = |path: String, id: NodeId, out: &mut Vec<Diagnostic>| {
        let count = operator_count(module, id);
        if count > COMPLEXITY_LIMIT {
            out.push(warning(
                module,
                "L003",
                format!("expression complexity {count} exceeds limit {COMPLEXITY_LIMIT}"),
                Some(path),
                Some(id),
            ));
        }
    };
    let mut names: Vec<&String> = module.bindings.keys().collect();
    names.sort();
    for name in names {
        check(name.clone(), module.bindings[name], &mut out);
    }
    if let Some(root) = module.root {
        if let Some(Node::Table { fields }) = module.get(root).cloned() {
            for (key, id) in &fields {
                check(key.clone(), *id, &mut out);
            }
        }
    }
    out
}

/// Normalized credential-name signals (lowercase, separators out).
fn is_credential_name(key: &str) -> bool {
    const SIGNALS: &[&str] = &[
        "password",
        "passwd",
        "secret",
        "token",
        "apikey",
        "privatekey",
        "auth",
        "credential",
        "accesskey",
        "secretkey",
    ];
    let normalized: String = key
        .to_lowercase()
        .chars()
        .filter(|c| *c != '_' && *c != '-')
        .collect();
    SIGNALS.iter().any(|signal| {
        normalized == *signal || normalized.starts_with(signal) || normalized.ends_with(signal)
    })
}

/// Values the name-based signal never flags: placeholders,
/// markers, empties, and URL locators (a `scheme://` value under a
/// credential-named key is an address, not a secret). The entropy
/// signal still applies to URLs — a token-laden query string stays
/// flaggable. Credentials embedded in low-entropy URLs are a
/// documented blind spot.
fn is_placeholder(value: &str) -> bool {
    if has_url_scheme(value) {
        return true;
    }
    matches!(
        value.to_lowercase().as_str(),
        "changeme"
            | "password"
            | "xxx"
            | "test"
            | "example"
            | "placeholder"
            | "redacted"
            | "***"
            | "***secret***"
            | ""
    )
}

/// True for `scheme://` locators (`https://…`, `postgres://…`).
fn has_url_scheme(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut i = 0;
    if bytes.first().is_some_and(|c| c.is_ascii_alphabetic()) {
        i += 1;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'+' | b'-' | b'.'))
        {
            i += 1;
        }
    }
    // `get` returns None when the range would split a character, so a
    // non-ASCII suffix can never panic here.
    value.get(i..i + 3) == Some("://")
}

/// Shannon entropy in bits per character (unigram distribution).
/// Counts sort by character for determinism: HashMap iteration order
/// could otherwise flip boundary cases through float rounding.
fn shannon_entropy(value: &str) -> f64 {
    use std::collections::BTreeMap;
    let mut counts = BTreeMap::new();
    let mut total = 0usize;
    for c in value.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
        total += 1;
    }
    if total == 0 {
        return 0.0;
    }
    counts
        .values()
        .map(|count| {
            let p = *count as f64 / total as f64;
            -p * p.log2()
        })
        .sum()
}

/// L004: probable hard-coded secrets. Messages name the key path
/// only — quoting the value would leak it into logs (C-036).
fn hardcoded_secrets(module: &Module) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    // (path, node, text) for every pure string literal in value
    // position: root tables, nested tables, arrays (with indexed
    // paths), and top-level lets.
    let mut literals: Vec<(String, NodeId, String)> = Vec::new();
    // Iterative walk from the root; bindings handled separately below.
    if let Some(root) = module.root {
        let mut stack = vec![(root, String::new())];
        while let Some((id, path)) = stack.pop() {
            match module.get(id) {
                Some(Node::Table { fields }) => {
                    // Reverse push keeps source order on pop (stack).
                    let mut entries: Vec<_> = fields.iter().collect();
                    entries.reverse();
                    for (key, child) in entries {
                        let child_path = if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        };
                        match module.get(*child) {
                            Some(Node::Literal(Value::String(text))) => {
                                literals.push((child_path, *child, text.clone()));
                            }
                            _ => stack.push((*child, child_path)),
                        }
                    }
                }
                Some(Node::Array { elements }) => {
                    let mut indexed: Vec<_> = elements.iter().enumerate().collect();
                    indexed.reverse();
                    for (i, child) in indexed {
                        let child_path = format!("{path}[{i}]");
                        match module.get(*child) {
                            Some(Node::Literal(Value::String(text))) => {
                                literals.push((child_path, *child, text.clone()));
                            }
                            _ => stack.push((*child, child_path)),
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut names: Vec<&String> = module.bindings.keys().collect();
    names.sort();
    for name in names {
        if let Some(Node::Literal(Value::String(text))) = module.get(module.bindings[name]) {
            literals.push((name.clone(), module.bindings[name], text.clone()));
        }
    }
    for (path, id, text) in literals {
        // Array indices belong to the owning key (`password[0]` checks
        // against `password`).
        let key = path.rsplit('.').next().unwrap_or(&path);
        let key = key.split('[').next().unwrap_or(key);
        let named = is_credential_name(key) && !is_placeholder(&text);
        let entropic =
            text.chars().count() >= ENTROPY_MIN_LEN && shannon_entropy(&text) >= ENTROPY_BITS;
        if named || entropic {
            out.push(warning(
                module,
                "L004",
                format!(
                    "possible hard-coded secret in key '{path}' (use secret() or secret.env())"
                ),
                Some(path),
                Some(id),
            ));
        }
    }
    out
}

/// L005: values present under schema-deprecated fields. Walks the
/// schema and value trees together (spec/validation.md §2 pairing).
/// The migration message is schema-authored text, so a reason that
/// quotes a secret is redacted like any other prose.
fn deprecated(module: &Module) -> Vec<Diagnostic> {
    let Some(schema_id) = module.schema else {
        return Vec::new();
    };
    let Some(root) = module.root else {
        return Vec::new();
    };
    let profile = crate::secret::SecretProfile::analyze(module);
    let mut out = Vec::new();
    // (schema node, value node, path) worklist.
    let mut stack = vec![(schema_id, root, String::new())];
    while let Some((schema, value, path)) = stack.pop() {
        let (
            Some(Node::SchemaTable {
                fields: schema_fields,
                ..
            }),
            Some(Node::Table {
                fields: value_fields,
            }),
        ) = (module.get(schema), module.get(value))
        else {
            continue;
        };
        // Deterministic: schema fields keep source order; reverse for
        // stack discipline.
        let mut entries: Vec<_> = schema_fields.iter().collect();
        entries.reverse();
        for (name, field_id) in entries {
            let child_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            let Some(value_id) = value_fields.get(name).copied() else {
                continue;
            };
            match module.get(*field_id) {
                Some(Node::SchemaField { field }) => {
                    if let Some(reason) = &field.deprecated {
                        let reason = profile
                            .redact_authored_text(reason)
                            .unwrap_or_else(|| reason.clone());
                        let message = if reason.is_empty() {
                            format!("deprecated field '{child_path}' is still used")
                        } else {
                            format!("deprecated field '{child_path}' is still used: {reason}")
                        };
                        out.push(warning(
                            module,
                            "L005",
                            message,
                            Some(child_path),
                            Some(value_id),
                        ));
                    }
                }
                Some(Node::SchemaTable { .. }) => {
                    stack.push((*field_id, value_id, child_path));
                }
                Some(Node::SchemaArray { element }) => {
                    if let Some(Node::Array { elements }) = module.get(value_id) {
                        for (i, element_id) in elements.iter().enumerate() {
                            stack.push((*element, *element_id, format!("{child_path}[{i}]")));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // The stack reverses source order; restore it for determinism.
    out.reverse();
    out
}

// ------------------------------------------------------------------
// Machine-readable reports (LINT.008)
// ------------------------------------------------------------------

/// Stable JSON report: one object per finding with the owning file,
/// severity, code, message, path, and span (when known). Field names
/// are contractual; additive changes only.
pub fn json_report(items: &[(String, Diagnostic)]) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = items
        .iter()
        .map(|(file, diagnostic)| {
            serde_json::json!({
                "file": file,
                "severity": diagnostic.severity,
                "code": diagnostic.code,
                "message": diagnostic.message,
                "path": diagnostic.path,
                "span": diagnostic.span,
            })
        })
        .collect();
    serde_json::Value::Array(rows)
}

/// URI-safe rendering of a filesystem path for SARIF: forward slashes,
/// and percent-encoding for anything outside RFC 3986 unreserved,
/// `/`, and `:`.
fn sarif_uri(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let mut out = String::with_capacity(normalized.len());
    for byte in normalized.bytes() {
        let keep =
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':');
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// SARIF 2.1.0 log for CI upload. Rule metadata is derived from the
/// findings (unique rule IDs, stable order); locations carry the span
/// when the diagnostic has one and a file-level location otherwise.
pub fn sarif_report(items: &[(String, Diagnostic)], tool_version: &str) -> serde_json::Value {
    let mut rule_ids: Vec<String> = Vec::new();
    for (_, diagnostic) in items {
        if !rule_ids.contains(&diagnostic.code) {
            rule_ids.push(diagnostic.code.clone());
        }
    }
    rule_ids.sort();
    let rules: Vec<serde_json::Value> = rule_ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "name": id,
                "shortDescription": {"text": format!("Active rule {id}")},
            })
        })
        .collect();
    let results: Vec<serde_json::Value> = items
        .iter()
        .map(|(file, diagnostic)| {
            let mut physical = serde_json::json!({
                "artifactLocation": {"uri": sarif_uri(file)},
            });
            if let Some(span) = &diagnostic.span {
                physical["region"] = serde_json::json!({
                    "startLine": span.line,
                    "startColumn": span.column,
                    "endLine": span.end_line,
                    "endColumn": span.end_column,
                });
            }
            serde_json::json!({
                "ruleId": diagnostic.code,
                "level": match diagnostic.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Info => "note",
                    Severity::Hint => "note",
                },
                "message": {"text": diagnostic.message},
                "locations": [{"physicalLocation": physical}],
            })
        })
        .collect();
    serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {"driver": {"name": "active", "version": tool_version, "rules": rules}},
            "results": results,
        }],
    })
}
