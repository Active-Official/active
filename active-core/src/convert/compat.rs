//! Schema compatibility reports (PROPOSAL §2.4, CHECKLIST SC.006–SC.007).
//!
//! Compares two documents' schema blocks field by field: added and
//! removed fields, type changes, tightened blob constraints, version
//! regressions, and raised reader floors. Findings carry stable C04x
//! codes (E040 summarizes a breaking result); the CLI fails closed
//! when any error-severity finding exists. Nothing here evaluates
//! values — only the two schema trees are read, so secrets cannot
//! enter the report.

use serde::Serialize;
use std::collections::HashMap;

use crate::ir::{Module, Node, SchemaField};

/// Compatibility finding codes (CHECKLIST SC.007).
pub mod code {
    /// Added required field (breaking for old readers).
    pub const ADDED_REQUIRED: &str = "C040";
    /// Removed field (breaking unless removed on schedule).
    pub const REMOVED_FIELD: &str = "C041";
    /// Field type changed or blob constraint tightened.
    pub const TYPE_CHANGED: &str = "C042";
    /// New version regressed below the old version.
    pub const VERSION_REGRESSED: &str = "C043";
    /// `min_reader` raised above the old version (drops readers).
    pub const READER_FLOOR_RAISED: &str = "C044";
    /// A root-schema cross-field `check` rule was added, removed, or
    /// changed (L-P19): the document's behavior changes.
    pub const CHECK_CHANGED: &str = "C045";
    /// A table's `closed` flag changed in either direction (L-P11).
    /// Closing rejects keys that were addable; opening withdraws the
    /// "no extra keys" promise readers may rely on.
    pub const CLOSED_CHANGED: &str = "C046";
    /// Breaking-change summary for a non-empty error set.
    pub const BREAKING_SUMMARY: &str = "E040";
}

/// One compatibility finding: stable code, severity, dotted path, and
/// a value-free message (type names and versions only — never data).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompatFinding {
    pub code: String,
    pub severity: String,
    pub path: String,
    pub message: String,
}

impl CompatFinding {
    fn error(code: &str, path: &str, message: String) -> Self {
        Self {
            code: code.to_string(),
            severity: "error".to_string(),
            path: path.to_string(),
            message,
        }
    }

    fn info(code: &str, path: &str, message: String) -> Self {
        Self {
            code: code.to_string(),
            severity: "info".to_string(),
            path: path.to_string(),
            message,
        }
    }

    pub fn is_breaking(&self) -> bool {
        self.severity == "error"
    }
}

/// Parsed `major.minor.patch` for ordering. Returns None for
/// non-conforming strings (the parser already rejects those with
/// E020, so this is a defensive second gate, never a silent pass).
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// A field is required when it is neither Null-typed nor has a *static*
/// default. The static rule is shared with the generators
/// (`convert::static_default`): a computed default counts as required on
/// every surface, so the compatibility report and the generated JSON
/// Schema cannot disagree about `required`.
fn field_required(module: &Module, field: &SchemaField) -> bool {
    !matches!(field.typ, crate::types::ActiveType::Null)
        && crate::convert::static_default(module, field).is_none()
}

/// One comparable schema entry: a scalar field, a table, or an array.
/// Nested tables/arrays are expanded to dotted paths so a change deep
/// in the tree can never hide behind an unchanged parent signature.
#[derive(Clone)]
struct Row {
    path: String,
    signature: String,
    /// Static constraints, compared with direction awareness. A
    /// type-only signature reported min/pattern/format/closed
    /// tightenings as `no schema changes` (LANG-B-004).
    constraints: Constraints,
    required: bool,
    removed_in: Option<String>,
    blob: Option<(Option<i64>, Option<String>)>,
    /// A blob bound argument exists but is not a literal, so it cannot
    /// be compared statically (LANG-B-029).
    blob_dynamic: bool,
}

fn node_signature(module: &Module, id: crate::ir::NodeId) -> String {
    match module.get(id) {
        Some(Node::SchemaField { field }) => field.typ.name(),
        Some(Node::SchemaTable { .. }) => "Table".to_string(),
        Some(Node::SchemaArray { element }) => {
            format!("Array<{}>", node_signature(module, *element))
        }
        _ => "unknown".to_string(),
    }
}

/// An exact constraint literal. Integers stay integers: casting them to
/// f64 collapses adjacent bounds above 2^53, so `9007199254740992` and
/// `9007199254740993` compared equal and a tightening went unreported
/// (LANG-B-004).
#[derive(Clone, Copy, PartialEq)]
enum Bound {
    Int(i64),
    Float(f64),
}

impl Bound {
    /// Total order between exact bounds; `None` only for NaN.
    fn order(self, other: Bound) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;
        match (self, other) {
            (Bound::Int(a), Bound::Int(b)) => Some(a.cmp(&b)),
            (Bound::Float(a), Bound::Float(b)) => a.partial_cmp(&b),
            (Bound::Int(a), Bound::Float(b)) => crate::eval::cmp_int_float(a, b),
            (Bound::Float(a), Bound::Int(b)) => {
                crate::eval::cmp_int_float(b, a).map(Ordering::reverse)
            }
        }
    }
}

impl std::fmt::Display for Bound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bound::Int(i) => write!(f, "{i}"),
            Bound::Float(x) => write!(f, "{x}"),
        }
    }
}

/// Static constraint set for one schema field. Direction matters: raising
/// a minimum (or lowering a maximum) can only invalidate documents that
/// validated before, while the reverse only widens the accepted set
/// (LANG-B-004).
#[derive(Clone, Default, PartialEq)]
struct Constraints {
    min: Option<Bound>,
    max: Option<Bound>,
    min_len: Option<Bound>,
    max_len: Option<Bound>,
    pattern: Option<String>,
    format: Option<String>,
    members: Option<Vec<String>>,
    closed: Option<bool>,
    has_computed: bool,
    /// Disjunction branches (L-P10) as structural facts, sorted by key
    /// so branch order is not a change; `None` for an ordinary field.
    one_of: Option<Vec<BranchFact>>,
    /// The declared default's canonical literal text (L-P10): part of the
    /// contract, so any add/remove/change is reported.
    default: Option<String>,
}

/// One disjunction branch, compared direction-aware: a removed branch is
/// compatible only when some new branch is a strict widening of it.
#[derive(Clone, PartialEq)]
struct BranchFact {
    /// Full structural key (nested table fields, closed flags, and
    /// constraints are part of it).
    key: String,
    /// The same key with every table's closed flag ignored (L-P11): two
    /// branches with equal open keys differ only in closedness, so the
    /// compat check reports one close/open change instead of a removal.
    key_open: String,
    /// The type skeleton with closed flags and every constraint argument
    /// ignored (L-P11). Two branches with equal skeletons describe the
    /// same shape, so a close/open inside them is paired even when the
    /// same edit also tightens a bound (C046 plus C042).
    key_shape: String,
    /// Every table's closed flag in the branch, keyed by position path
    /// (field names, array indices, disjunction shapes). The path makes
    /// the comparison order-independent, so nested changes in a nested
    /// disjunction are reported too.
    closed_flags: Vec<(String, bool)>,
    /// The nested branches of a field-level disjunction, for messages and
    /// structural recursion; `None` for an ordinary node.
    nested: Option<Vec<BranchFact>>,
    /// Base type name, for the widening check.
    base: String,
    /// Leaf constraints, when the branch is a leaf field.
    leaf: Option<LeafBounds>,
    /// A constraint argument is computed: it cannot be compared, so the
    /// branch can never be proven to widen (conservative).
    computed: bool,
    /// A constraint is opaque (material or an unrenderable expression):
    /// two such branches are never considered equal.
    opaque: bool,
    /// Table shape, when the branch is a nested table.
    table: Option<TableShape>,
    /// Element fact, when the branch is an array.
    element: Option<Box<BranchFact>>,
}

/// One table branch: its closed flag and fields with requiredness.
#[derive(Clone, PartialEq)]
struct TableShape {
    closed: bool,
    fields: Vec<(String, bool, BranchFact)>,
}

/// The comparable leaf constraints of one branch. Computed constraints
/// are folded into `key` instead (two branches with different computed
/// constraints never compare equal).
#[derive(Clone, PartialEq)]
struct LeafBounds {
    min: Option<Bound>,
    max: Option<Bound>,
    min_len: Option<Bound>,
    max_len: Option<Bound>,
    pattern: Option<String>,
    format: Option<String>,
    members: Option<Vec<String>>,
    blob_max: Option<i64>,
    blob_media: Option<String>,
}

/// Whether a schema node is required in a table: fields follow the
/// static-default rule, structural nodes are always required.
fn branch_required(module: &Module, id: crate::ir::NodeId) -> bool {
    match module.get(id) {
        Some(Node::SchemaField { field }) => {
            !matches!(field.typ, crate::types::ActiveType::Null)
                && crate::convert::static_default(module, field).is_none()
        }
        _ => true,
    }
}

/// Read one branch bound; the bool marks a computed (uncomparable) node.
fn branch_number(module: &Module, slot: Option<crate::ir::NodeId>) -> (Option<Bound>, bool) {
    match slot.and_then(|n| module.get(n)) {
        Some(Node::Literal(crate::ir::Value::Int(n))) => (Some(Bound::Int(*n)), false),
        Some(Node::Literal(crate::ir::Value::Float(f))) => (Some(Bound::Float(*f)), false),
        Some(Node::Literal(crate::ir::Value::DurationMs(ms))) => {
            (Some(Bound::Int(*ms as i64)), false)
        }
        Some(_) => (None, true),
        None => (None, false),
    }
}

/// Read one branch text constraint; the bool marks a computed node.
fn branch_text(module: &Module, slot: Option<crate::ir::NodeId>) -> (Option<String>, bool) {
    match slot.and_then(|n| module.get(n)) {
        Some(Node::Literal(crate::ir::Value::String(text))) => (Some(text.clone()), false),
        Some(_) => (None, true),
        None => (None, false),
    }
}

/// Read a branch blob bound; the bool marks a computed node.
fn branch_blob_max(module: &Module, slot: Option<crate::ir::NodeId>) -> (Option<i64>, bool) {
    match slot.and_then(|n| module.get(n)) {
        Some(Node::Literal(crate::ir::Value::Int(n))) => (Some(*n), false),
        Some(_) => (None, true),
        None => (None, false),
    }
}

fn branch_fact(module: &Module, id: crate::ir::NodeId) -> BranchFact {
    let key = crate::ir::schema_node_key(module, id);
    let key_open = crate::ir::schema_node_key_open(module, id);
    let key_shape = crate::ir::schema_node_key_shape(module, id);
    let closed_flags = crate::ir::schema_node_closed_flags(module, id);
    match module.get(id) {
        Some(Node::SchemaField { field }) if field.one_of.is_empty() => {
            let (min, c_min) = branch_number(module, field.min);
            let (max, c_max) = branch_number(module, field.max);
            let (min_len, c_min_len) = branch_number(module, field.min_len);
            let (max_len, c_max_len) = branch_number(module, field.max_len);
            let (pattern, c_pattern) = branch_text(module, field.pattern);
            let (format, c_format) = branch_text(module, field.format);
            let (blob_media, c_media) = branch_text(module, field.blob_media_type);
            let (blob_max, c_blob_max) = branch_blob_max(module, field.blob_max_size);
            let members = match &field.typ {
                crate::types::ActiveType::Enum(members) => Some(members.clone()),
                _ => None,
            };
            let leaf = LeafBounds {
                min,
                max,
                min_len,
                max_len,
                pattern,
                format,
                members,
                blob_max,
                blob_media,
            };
            BranchFact {
                key,
                key_open,
                key_shape,
                closed_flags,
                nested: None,
                base: field.typ.name(),
                leaf: Some(leaf),
                computed: c_min
                    || c_max
                    || c_min_len
                    || c_max_len
                    || c_pattern
                    || c_format
                    || c_media
                    || c_blob_max,
                opaque: field.branch_opaque(module),
                table: None,
                element: None,
            }
        }
        Some(Node::SchemaTable { fields, closed, .. }) => {
            let shape = TableShape {
                closed: *closed,
                fields: fields
                    .iter()
                    .map(|(name, child)| {
                        (
                            name.clone(),
                            branch_required(module, *child),
                            branch_fact(module, *child),
                        )
                    })
                    .collect(),
            };
            BranchFact {
                key,
                key_open,
                key_shape,
                closed_flags,
                nested: None,
                base: "Table".to_string(),
                leaf: None,
                computed: shape.fields.iter().any(|(_, _, fact)| fact.computed),
                opaque: shape.fields.iter().any(|(_, _, fact)| fact.opaque),
                table: Some(shape),
                element: None,
            }
        }
        Some(Node::SchemaArray { element }) => BranchFact {
            key,
            key_open,
            key_shape,
            closed_flags,
            nested: None,
            base: "Array".to_string(),
            leaf: None,
            computed: false,
            opaque: branch_fact(module, *element).opaque,
            table: None,
            element: Some(Box::new(branch_fact(module, *element))),
        },
        Some(Node::SchemaField { field }) => BranchFact {
            // A nested disjunction is flattened by the parser; this is
            // defensive only. The nested facts let messages describe the
            // alternative that changed.
            key: field.branch_key(module),
            key_open,
            key_shape,
            closed_flags,
            nested: Some(
                field
                    .one_of
                    .iter()
                    .map(|branch| branch_fact(module, *branch))
                    .collect(),
            ),
            base: "OneOf".to_string(),
            leaf: None,
            computed: false,
            opaque: field.branch_opaque(module),
            table: None,
            element: None,
        },
        _ => BranchFact {
            key: "schema".to_string(),
            key_open,
            key_shape,
            closed_flags,
            nested: None,
            base: "schema".to_string(),
            leaf: None,
            computed: false,
            opaque: true,
            table: None,
            element: None,
        },
    }
}

/// Does `new` accept every value `old` accepts? Used to treat a removed
/// branch as compatible only when it was genuinely relaxed.
fn widens(old: &BranchFact, new: &BranchFact) -> bool {
    if old.base != new.base {
        return false;
    }
    // A computed constraint cannot be compared: never claim a widening.
    // An opaque constraint cannot even be identified across documents.
    if old.computed || new.computed || old.opaque || new.opaque {
        return false;
    }
    match (&old.table, &new.table) {
        (Some(o), Some(n)) => {
            // L-P11: a widening must preserve the promise. Opening a
            // closed table grows the accepted set but withdraws
            // "no extra keys", so it cannot excuse a removed branch;
            // closing one adds a promise that did not hold before.
            if o.closed != n.closed {
                return false;
            }
            for (name, o_required, o_fact) in &o.fields {
                match n.fields.iter().find(|(n_name, ..)| n_name == name) {
                    Some((_, n_required, n_fact)) => {
                        if !o_required && *n_required {
                            return false;
                        }
                        if !widens(o_fact, n_fact) {
                            return false;
                        }
                    }
                    None => {
                        if n.closed {
                            return false;
                        }
                    }
                }
            }
            for (n_name, n_required, _) in &n.fields {
                if *n_required && !o.fields.iter().any(|(o_name, ..)| o_name == n_name) {
                    return false;
                }
            }
            return true;
        }
        (None, None) => {}
        _ => return false,
    }
    match (&old.element, &new.element) {
        (Some(o), Some(n)) => return widens(o, n),
        (None, None) => {}
        _ => return false,
    }
    let (Some(o), Some(n)) = (&old.leaf, &new.leaf) else {
        // Two opaque nodes widen only by exact equality.
        return old.key == new.key;
    };
    let lower_ok = |o: Option<Bound>, n: Option<Bound>| match (o, n) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => true,
        (Some(a), Some(b)) => b.order(a) != Some(std::cmp::Ordering::Greater),
    };
    let upper_ok = |o: Option<Bound>, n: Option<Bound>| match (o, n) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => true,
        (Some(a), Some(b)) => b.order(a) != Some(std::cmp::Ordering::Less),
    };
    let opt_text_ok = |o: &Option<String>, n: &Option<String>| match (o, n) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => true,
        (Some(a), Some(b)) => a == b,
    };
    let members_ok = match (&o.members, &n.members) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => true,
        (Some(old), Some(new)) => old.iter().all(|member| new.contains(member)),
    };
    let blob_max_ok = match (o.blob_max, n.blob_max) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => true,
        (Some(a), Some(b)) => b >= a,
    };
    lower_ok(o.min, n.min)
        && upper_ok(o.max, n.max)
        && lower_ok(o.min_len, n.min_len)
        && upper_ok(o.max_len, n.max_len)
        && opt_text_ok(&o.pattern, &n.pattern)
        && opt_text_ok(&o.format, &n.format)
        && members_ok
        && blob_max_ok
        && opt_text_ok(&o.blob_media, &n.blob_media)
}

fn constraints_of(module: &Module, field: &SchemaField) -> Constraints {
    fn number(module: &Module, id: Option<crate::ir::NodeId>) -> (Option<Bound>, bool) {
        match id.and_then(|n| module.get(n)) {
            Some(Node::Literal(crate::ir::Value::Int(n))) => (Some(Bound::Int(*n)), false),
            Some(Node::Literal(crate::ir::Value::Float(f))) => (Some(Bound::Float(*f)), false),
            Some(_) => (None, true),
            None => (None, false),
        }
    }
    fn text(module: &Module, id: Option<crate::ir::NodeId>) -> (Option<String>, bool) {
        match id.and_then(|n| module.get(n)) {
            Some(Node::Literal(crate::ir::Value::String(s))) => (Some(s.clone()), false),
            Some(_) => (None, true),
            None => (None, false),
        }
    }
    let (min, c1) = number(module, field.min);
    let (max, c2) = number(module, field.max);
    let (min_len, c3) = number(module, field.min_len);
    let (max_len, c4) = number(module, field.max_len);
    let (pattern, c5) = text(module, field.pattern);
    let (format, c6) = text(module, field.format);
    let (members, closed) = match &field.typ {
        crate::types::ActiveType::Enum(members) => (Some(members.clone()), None),
        crate::types::ActiveType::Table { closed } => (None, Some(*closed)),
        _ => (None, None),
    };
    let one_of = if field.one_of.is_empty() {
        None
    } else {
        let mut branches: Vec<BranchFact> = field
            .one_of
            .iter()
            .map(|branch| branch_fact(module, *branch))
            .collect();
        branches.sort_by(|a, b| a.key.cmp(&b.key));
        branches.dedup_by(|a, b| a == b);
        Some(branches)
    };
    let default = match field.default.and_then(|id| module.get(id)) {
        Some(Node::Literal(value)) => Some(crate::convert::value_to_pretty_json(value)),
        // A computed default cannot be compared statically; the marker
        // keeps "present but dynamic" distinct from absent without
        // double-reporting it through `has_computed` (the required/default
        // findings already describe it).
        Some(_) => Some("<computed>".to_string()),
        None => None,
    };
    Constraints {
        min,
        max,
        min_len,
        max_len,
        pattern,
        format,
        members,
        closed,
        has_computed: c1 || c2 || c3 || c4 || c5 || c6,
        one_of,
        default,
    }
}

/// True when `new` can reject a document `old` accepted.
fn tightened(old: &Constraints, new: &Constraints) -> bool {
    // A computed constraint cannot be compared statically; any change in
    // its presence is surfaced rather than assumed safe.
    if old.has_computed != new.has_computed {
        return true;
    }
    let raises = |o: Option<Bound>, n: Option<Bound>| match (o, n) {
        (None, Some(_)) => true,
        (Some(o), Some(n)) => n.order(o) == Some(std::cmp::Ordering::Greater),
        _ => false,
    };
    let lowers = |o: Option<Bound>, n: Option<Bound>| match (o, n) {
        (None, Some(_)) => true,
        (Some(o), Some(n)) => n.order(o) == Some(std::cmp::Ordering::Less),
        _ => false,
    };
    if raises(old.min, new.min)
        || lowers(old.max, new.max)
        || raises(old.min_len, new.min_len)
        || lowers(old.max_len, new.max_len)
    {
        return true;
    }
    if old.pattern != new.pattern || old.format != new.format {
        // Any edit can narrow the accepted set; report rather than guess.
        return old.pattern != new.pattern || old.format != new.format;
    }
    // L-P10: the disjunction and the default are part of the contract.
    // A removed branch narrows the accepted set unless a new branch is a
    // genuine widening of it; branch order is not a change (sorted sets).
    match (&old.one_of, &new.one_of) {
        (Some(o), Some(n)) => {
            // An opaque branch cannot be identified across documents, so
            // it can never be treated as unchanged.
            // A branch whose open key matches but whose full key differs
            // changed only in closedness: that close/open is reported as
            // C046 by the caller, not as a removed branch.
            let same = |a: &BranchFact, b: &BranchFact| {
                !a.opaque && !b.opaque && (a.key == b.key || a.key_open == b.key_open)
            };
            for branch in o {
                if n.iter().any(|candidate| same(branch, candidate)) {
                    continue;
                }
                if n.iter().any(|candidate| widens(branch, candidate)) {
                    continue;
                }
                return true;
            }
        }
        (None, Some(_)) | (Some(_), None) => {
            // The type signature changes with it; the row signature
            // reports the break.
        }
        (None, None) => {}
    }
    // A default on a disjunction entry is part of the branch contract
    // (it must satisfy a branch), so any change is reported. Ordinary
    // fields keep their existing required/default finding instead of a
    // duplicate.
    if (old.one_of.is_some() || new.one_of.is_some()) && old.default != new.default {
        return true;
    }
    match (&old.members, &new.members) {
        // Removing a member narrows the accepted set: an old document using
        // it is rejected by the new schema. Adding one only widens
        // (LANG-B-004: the comparison used to run the other way).
        (Some(o), Some(n)) => o.iter().any(|m| !n.contains(m)),
        (None, Some(_)) => true,
        _ => false,
    }
    // The closed flag is deliberately not compared here: L-P11 reports
    // every close/open transition on its own C046 finding because the
    // opening direction is breaking and has no tightening message.
}

/// Render the constraints that are set, for the finding message.
fn describe_constraints(c: &Constraints) -> String {
    let mut parts = Vec::new();
    if let Some(v) = c.min {
        parts.push(format!("min={v}"));
    }
    if let Some(v) = c.max {
        parts.push(format!("max={v}"));
    }
    if let Some(v) = c.min_len {
        parts.push(format!("min_len={v}"));
    }
    if let Some(v) = c.max_len {
        parts.push(format!("max_len={v}"));
    }
    if let Some(v) = &c.pattern {
        parts.push(format!("pattern={v:?}"));
    }
    if let Some(v) = &c.format {
        parts.push(format!("format={v:?}"));
    }
    if let Some(v) = &c.members {
        parts.push(format!("enum={}", v.join("|")));
    }
    if let Some(v) = c.closed {
        parts.push(format!("closed={v}"));
    }
    if c.has_computed {
        parts.push("computed".to_string());
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    }
}

fn row_for_field(module: &Module, path: &str, field: &SchemaField) -> Row {
    let blob = matches!(field.typ, crate::types::ActiveType::Blob).then(|| {
        (
            literal_int(module, field.blob_max_size),
            literal_string(module, field.blob_media_type),
        )
    });
    let blob_dynamic = field
        .blob_max_size
        .is_some_and(|id| literal_int(module, Some(id)).is_none())
        || field
            .blob_media_type
            .is_some_and(|id| literal_string(module, Some(id)).is_none());
    Row {
        path: path.to_string(),
        signature: field.typ.name(),
        constraints: constraints_of(module, field),
        required: field_required(module, field),
        removed_in: field.removed_in.clone(),
        blob,
        blob_dynamic,
    }
}

/// Collect every schema entry (tables and arrays included) as dotted
/// rows in source order.
fn collect_rows(module: &Module, root: crate::ir::NodeId, prefix: &str, out: &mut Vec<Row>) {
    let Some(Node::SchemaTable { fields, .. }) = module.get(root) else {
        return;
    };
    for (key, id) in fields {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match module.get(*id) {
            Some(Node::SchemaField { field }) => out.push(row_for_field(module, &path, field)),
            Some(Node::SchemaTable { closed, .. }) => {
                out.push(Row {
                    path: path.clone(),
                    signature: "Table".to_string(),
                    // The `closed` flag lives on the table node; a
                    // false->true flip rejects previously valid keys
                    // (LANG-B-004).
                    constraints: Constraints {
                        closed: Some(*closed),
                        ..Constraints::default()
                    },
                    required: true,
                    removed_in: None,
                    blob: None,
                    blob_dynamic: false,
                });
                collect_rows(module, *id, &path, out);
            }
            Some(Node::SchemaArray { element }) => {
                out.push(Row {
                    path: path.clone(),
                    signature: format!("Array<{}>", node_signature(module, *element)),
                    constraints: Constraints::default(),
                    required: true,
                    removed_in: None,
                    blob: None,
                    blob_dynamic: false,
                });
                // Every element shape is addressable: tables expand to
                // fields, nested arrays recurse, and a scalar element is
                // a row at `path[]` (this is how Array<Blob> bounds and
                // media changes become visible).
                collect_element_rows(module, *element, &format!("{path}[]"), out);
            }
            _ => {}
        }
    }
}

fn collect_fields(module: &Module, root: crate::ir::NodeId) -> Vec<Row> {
    let mut out = Vec::new();
    collect_rows(module, root, "", &mut out);
    out
}

/// Rows inside an array element. Table elements expand to
/// `path[].field`; scalar elements (including blobs) become one row at
/// `path[]`; nested arrays recurse with another `[]` segment.
fn collect_element_rows(
    module: &Module,
    element: crate::ir::NodeId,
    prefix: &str,
    out: &mut Vec<Row>,
) {
    match module.get(element) {
        Some(Node::SchemaTable { closed, .. }) => {
            // L-P11: the element table's own row carries its closed flag;
            // without it, `Array<Table{closed}>` -> open (or the reverse)
            // was invisible.
            out.push(Row {
                path: prefix.to_string(),
                signature: "Table".to_string(),
                constraints: Constraints {
                    closed: Some(*closed),
                    ..Constraints::default()
                },
                required: true,
                removed_in: None,
                blob: None,
                blob_dynamic: false,
            });
            collect_rows(module, element, prefix, out);
        }
        Some(Node::SchemaArray { element: inner }) => {
            out.push(Row {
                path: prefix.to_string(),
                signature: format!("Array<{}>", node_signature(module, *inner)),
                constraints: Constraints::default(),
                required: true,
                removed_in: None,
                blob: None,
                blob_dynamic: false,
            });
            collect_element_rows(module, *inner, &format!("{prefix}[]"), out);
        }
        Some(Node::SchemaField { field }) => out.push(row_for_field(module, prefix, field)),
        _ => {}
    }
}

fn schema_meta(module: &Module) -> crate::ir::SchemaMeta {
    match module.schema.and_then(|root| module.get(root)) {
        Some(Node::SchemaTable { meta, .. }) => meta.clone(),
        _ => crate::ir::SchemaMeta::default(),
    }
}

fn literal_int(module: &Module, id: Option<crate::ir::NodeId>) -> Option<i64> {
    match id.and_then(|node| module.get(node)) {
        Some(Node::Literal(crate::ir::Value::Int(n))) => Some(*n),
        _ => None,
    }
}

fn literal_string(module: &Module, id: Option<crate::ir::NodeId>) -> Option<String> {
    match id.and_then(|node| module.get(node)) {
        Some(Node::Literal(crate::ir::Value::String(s))) => Some(s.clone()),
        _ => None,
    }
}

/// The C046 message for one close/open transition (L-P11). `branch`
/// carries the changed branch's type descriptor (an order-independent
/// identity: `Table{closed=true, a:Int}`), so a wide disjunction stays
/// auditable.
fn closed_message(path: &str, now_closed: bool, branch: Option<&str>) -> String {
    let subject = if let Some(descriptor) = branch {
        format!("disjunction branch {descriptor} at '{path}'")
    } else if path == "<root>" {
        "the document's root table".to_string()
    } else {
        format!("table '{path}'")
    };
    if now_closed {
        format!(
            "{subject} closed (was open): keys outside the declared set now fail T005; \
             old documents that added keys fail the new schema"
        )
    } else {
        format!(
            "{subject} opened (was closed): the no-extra-keys promise is withdrawn; \
             readers that relied on the closed key set may misread documents"
        )
    }
}

/// A short, order-independent identity for a branch (L-P11 messages):
/// the table/array/disjunction structure with field types and leaf
/// bounds, so a reader can find the branch without relying on position
/// (branch order is not a change). Field names, enum members, and
/// alternative branches are sorted.
fn branch_descriptor(fact: &BranchFact) -> String {
    if let Some(shape) = &fact.table {
        let mut fields: Vec<String> = shape
            .fields
            .iter()
            .map(|(name, required, child)| {
                format!(
                    "{name}{}:{}",
                    if *required { "" } else { "?" },
                    branch_descriptor(child)
                )
            })
            .collect();
        fields.sort();
        let closed = if shape.closed { "closed=true, " } else { "" };
        return format!("Table{{{closed}{}}}", fields.join(", "));
    }
    if let Some(element) = &fact.element {
        return format!("Array<{}>", branch_descriptor(element));
    }
    if let Some(nested) = &fact.nested {
        let mut parts: Vec<String> = nested.iter().map(branch_descriptor).collect();
        parts.sort();
        return format!("OneOf({})", parts.join(", "));
    }
    let Some(leaf) = &fact.leaf else {
        return fact.base.clone();
    };
    let mut args: Vec<String> = Vec::new();
    if let Some(value) = &leaf.min {
        args.push(format!("min={value}"));
    }
    if let Some(value) = &leaf.max {
        args.push(format!("max={value}"));
    }
    if let Some(value) = &leaf.min_len {
        args.push(format!("min_len={value}"));
    }
    if let Some(value) = &leaf.max_len {
        args.push(format!("max_len={value}"));
    }
    if let Some(members) = &leaf.members {
        let mut members = members.clone();
        members.sort();
        args.push(format!("members={}", members.join("|")));
    }
    if let Some(value) = &leaf.blob_max {
        args.push(format!("max_size={value}"));
    }
    if args.is_empty() {
        fact.base.clone()
    } else {
        format!("{}({})", fact.base, args.join(", "))
    }
}

/// Per-path closed/open counts for one branch's table flags (L-P11).
/// The number of tables at a path is structural, so a count difference
/// is exactly the number of tables that changed direction - even when
/// several disjunction alternatives share a shape.
fn closed_flag_counts(flags: &[(String, bool)]) -> std::collections::HashMap<&str, (usize, usize)> {
    let mut counts: std::collections::HashMap<&str, (usize, usize)> =
        std::collections::HashMap::new();
    for (path, closed) in flags {
        let entry = counts.entry(path.as_str()).or_insert((0, 0));
        if *closed {
            entry.1 += 1;
        } else {
            entry.0 += 1;
        }
    }
    counts
}

/// Close/open transitions between two disjunction branch sets (L-P11).
/// Branches are paired by their skeleton (the same shape with closed flags
/// and constraints ignored) and the closed flags are compared per
/// position path, so a nested change - including one inside a nested
/// disjunction - is reported with the direction it happened in. Each
/// entry is `(branch descriptor, now closed)`, deduplicated; a branch
/// that both closes one nested table and opens another yields two.
fn one_of_closed_changes(old: &[BranchFact], new: &[BranchFact]) -> Vec<(String, bool)> {
    let mut changes: Vec<(String, bool)> = Vec::new();
    for o in old {
        if o.opaque {
            continue;
        }
        for n in new {
            if n.opaque || o.key_shape != n.key_shape || o.key == n.key {
                continue;
            }
            let old_counts = closed_flag_counts(&o.closed_flags);
            let new_counts = closed_flag_counts(&n.closed_flags);
            let mut closed = false;
            let mut opened = false;
            for (path, &(_, old_closed)) in &old_counts {
                let new_closed = new_counts.get(path).map(|(_, closed)| *closed).unwrap_or(0);
                if new_closed > old_closed {
                    closed = true;
                } else if old_closed > new_closed {
                    opened = true;
                }
            }
            let descriptor = branch_descriptor(n);
            if closed
                && !changes
                    .iter()
                    .any(|(text, now)| text == &descriptor && *now)
            {
                changes.push((descriptor.clone(), true));
            }
            if opened
                && !changes
                    .iter()
                    .any(|(text, now)| text == &descriptor && !*now)
            {
                changes.push((descriptor, false));
            }
        }
    }
    changes
}

/// Compare two documents' schema blocks. Deterministic: fields walk in
/// source order, versions compare numerically, findings emit in a
/// fixed order (root closed, removed, added, changed, version gates).
pub fn compare(old: &Module, new: &Module) -> Result<Vec<CompatFinding>, String> {
    let old_root = old
        .schema
        .ok_or_else(|| "old document has no schema block".to_string())?;
    let new_root = new
        .schema
        .ok_or_else(|| "new document has no schema block".to_string())?;
    let old_fields = collect_fields(old, old_root);
    let new_fields = collect_fields(new, new_root);
    // Index by path: comparing two n-row schemas is O(n), not O(n²).
    let old_by_path: HashMap<&str, &Row> = old_fields
        .iter()
        .map(|row| (row.path.as_str(), row))
        .collect();
    let new_by_path: HashMap<&str, &Row> = new_fields
        .iter()
        .map(|row| (row.path.as_str(), row))
        .collect();

    let mut findings = Vec::new();

    // L-P11: the schema root is a table too. Its closed flag governs the
    // document's top-level keys, and close/open is the same promise
    // change there as on any nested table. The root has no row (rows
    // describe fields), so it is compared here.
    let old_root_closed = matches!(
        old.get(old_root),
        Some(Node::SchemaTable { closed: true, .. })
    );
    let new_root_closed = matches!(
        new.get(new_root),
        Some(Node::SchemaTable { closed: true, .. })
    );
    if old_root_closed != new_root_closed {
        findings.push(CompatFinding::error(
            code::CLOSED_CHANGED,
            "<root>",
            closed_message("<root>", new_root_closed, None),
        ));
    }

    // Removed fields: breaking unless the removal was scheduled at or
    // before the new version (removed_in <= new version).
    let new_version = schema_meta(new).version;
    for old_field in &old_fields {
        if new_by_path.contains_key(old_field.path.as_str()) {
            continue;
        }
        let scheduled = match (&old_field.removed_in, &new_version) {
            (Some(removed_in), Some(current)) => {
                match (parse_version(removed_in), parse_version(current)) {
                    (Some(r), Some(c)) => r <= c,
                    // Unparseable versions fail safe: scheduled only
                    // when both sides parse and order correctly.
                    _ => false,
                }
            }
            _ => false,
        };
        let path = &old_field.path;
        if scheduled {
            findings.push(CompatFinding::info(
                code::REMOVED_FIELD,
                path,
                format!("field '{path}' removed on schedule (removed_in matches new version)"),
            ));
        } else {
            findings.push(CompatFinding::error(
                code::REMOVED_FIELD,
                path,
                format!("field '{path}' removed: old readers and writers lose data"),
            ));
        }
    }

    // Added fields, type changes, and blob tightening.
    for new_field in &new_fields {
        match old_by_path.get(new_field.path.as_str()).copied() {
            None => {
                let path = &new_field.path;
                if new_field.required {
                    findings.push(CompatFinding::error(
                        code::ADDED_REQUIRED,
                        path,
                        format!(
                            "field '{path}' added without a default: old documents fail the new schema"
                        ),
                    ));
                } else {
                    findings.push(CompatFinding::info(
                        code::ADDED_REQUIRED,
                        path,
                        format!(
                            "field '{path}' added with a default or Null type: old documents still validate in resilient mode; strict mode still requires the value explicitly (T004)"
                        ),
                    ));
                }
            }
            Some(old_field) => {
                let path = &new_field.path;
                if old_field.signature != new_field.signature {
                    // An enum member-set change is breaking only when it
                    // removes a member; adding one keeps every old value
                    // valid (LANG-B-004). Other type changes stay breaking.
                    let enum_only_widened = match (
                        &old_field.constraints.members,
                        &new_field.constraints.members,
                    ) {
                        (Some(old_members), Some(new_members)) => old_members
                            .iter()
                            .all(|member| new_members.contains(member)),
                        _ => false,
                    };
                    if !enum_only_widened {
                        findings.push(CompatFinding::error(
                            code::TYPE_CHANGED,
                            path,
                            format!(
                                "field '{path}' type changed from {} to {}",
                                old_field.signature, new_field.signature
                            ),
                        ));
                    }
                }
                // These checks are INDEPENDENT (LANG-B-029): the old
                // else-if chain reported only the first change and hid
                // the rest (a simultaneous requiredness + blob change
                // dropped the blob finding entirely).
                // L-P11: a table's closed flag is its own contract. Both
                // directions are breaking: closing rejects keys that were
                // addable, opening withdraws the "no extra keys" promise.
                if let (Some(old_closed), Some(new_closed)) =
                    (old_field.constraints.closed, new_field.constraints.closed)
                {
                    if old_closed != new_closed {
                        findings.push(CompatFinding::error(
                            code::CLOSED_CHANGED,
                            path,
                            closed_message(path, new_closed, None),
                        ));
                    }
                }
                if let (Some(old_branches), Some(new_branches)) = (
                    old_field.constraints.one_of.as_ref(),
                    new_field.constraints.one_of.as_ref(),
                ) {
                    let changes = one_of_closed_changes(old_branches, new_branches);
                    for (descriptor, now_closed) in changes {
                        findings.push(CompatFinding::error(
                            code::CLOSED_CHANGED,
                            path,
                            closed_message(path, now_closed, Some(&descriptor)),
                        ));
                    }
                }
                if tightened(&old_field.constraints, &new_field.constraints) {
                    // A disjunction change is a branch contract change; the
                    // leaf-constraint wording would read "(none -> none)".
                    let message = if old_field.constraints.one_of != new_field.constraints.one_of {
                        format!(
                            "field '{path}' disjunction changed: a branch was removed or \
                                 narrowed; documents that validated before may fail now"
                        )
                    } else {
                        format!(
                                "field '{path}' constraints tightened ({} -> {}): documents that validated before may fail now",
                                describe_constraints(&old_field.constraints),
                                describe_constraints(&new_field.constraints)
                            )
                    };
                    findings.push(CompatFinding::error(code::TYPE_CHANGED, path, message));
                }
                if old_field.required != new_field.required && new_field.required {
                    // Removing a default makes the field required
                    // without changing its type: old documents that
                    // omitted it now fail.
                    findings.push(CompatFinding::error(
                        code::TYPE_CHANGED,
                        path,
                        format!(
                            "field '{path}' became required (default removed): old documents fail the new schema"
                        ),
                    ));
                }
                if let (Some((old_max, old_media)), Some((new_max, new_media))) =
                    (&old_field.blob, &new_field.blob)
                {
                    if old_max.is_some() || new_max.is_some() || old_media != new_media {
                        check_blob_tightening(
                            path,
                            old_max,
                            old_media,
                            new_max,
                            new_media,
                            &mut findings,
                        );
                    }
                }
                if old_field.blob_dynamic || new_field.blob_dynamic {
                    findings.push(CompatFinding::info(
                        code::TYPE_CHANGED,
                        path,
                        format!(
                            "field '{path}' blob bounds are computed; compatibility cannot compare them"
                        ),
                    ));
                }
            }
        }
    }

    // Version gates from schema metadata.
    check_version_gates(old, new, &mut findings);

    // Messages echo schema prose (versions, media types); redact with
    // L-P19: a cross-field check is behavior. Adding, removing, or
    // changing a rule can refuse a document that used to pass (or admit
    // one that used to fail), so each is a breaking finding.
    let old_checks = old
        .schema_checks
        .get(&old_root)
        .cloned()
        .unwrap_or_default();
    let new_checks = new
        .schema_checks
        .get(&new_root)
        .cloned()
        .unwrap_or_default();
    let old_by_name: HashMap<&str, &crate::ir::CheckRule> = old_checks
        .iter()
        .map(|rule| (rule.name.as_str(), rule))
        .collect();
    let new_by_name: HashMap<&str, &crate::ir::CheckRule> = new_checks
        .iter()
        .map(|rule| (rule.name.as_str(), rule))
        .collect();
    for rule in &old_checks {
        let path = format!("check.{}", rule.name);
        match new_by_name.get(rule.name.as_str()) {
            None => findings.push(CompatFinding::error(
                code::CHECK_CHANGED,
                &path,
                format!("check '{}' removed", rule.name),
            )),
            Some(other) if other.source != rule.source => findings.push(CompatFinding::error(
                code::CHECK_CHANGED,
                &path,
                format!("check '{}' changed", rule.name),
            )),
            _ => {}
        }
    }
    for rule in &new_checks {
        if !old_by_name.contains_key(rule.name.as_str()) {
            findings.push(CompatFinding::error(
                code::CHECK_CHANGED,
                &format!("check.{}", rule.name),
                format!("check '{}' added", rule.name),
            ));
        }
    }

    // both documents' material sets so neither side can leak a copy.
    let old_profile = crate::secret::SecretProfile::analyze(old);
    let new_profile = crate::secret::SecretProfile::analyze(new);
    for finding in &mut findings {
        if let Some(text) = old_profile.redact_authored_text(&finding.message) {
            finding.message = text;
        }
        if let Some(text) = new_profile.redact_authored_text(&finding.message) {
            finding.message = text;
        }
        // Paths carry the rule name for C045; redact them with both
        // documents' material models exactly like messages.
        finding.path = new_profile.redact_path(&old_profile.redact_path(&finding.path));
    }

    Ok(findings)
}

/// Blob constraint tightening is breaking for existing payloads:
/// `max_size` lowered or `media_type` changed (or newly pinned).
fn check_blob_tightening(
    path: &str,
    old_max: &Option<i64>,
    old_media: &Option<String>,
    new_max: &Option<i64>,
    new_media: &Option<String>,
    findings: &mut Vec<CompatFinding>,
) {
    if let (Some(old), Some(new)) = (old_max, new_max) {
        if new < old {
            findings.push(CompatFinding::error(
                code::TYPE_CHANGED,
                path,
                format!("blob '{path}' max_size tightened from {old} to {new}: existing payloads may fail"),
            ));
        }
    } else if old_max.is_none() && new_max.is_some() {
        // Newly pinning a bound is a tightening: payloads that were
        // acceptable before may now fail.
        let new = new_max.unwrap_or(0);
        findings.push(CompatFinding::error(
            code::TYPE_CHANGED,
            path,
            format!("blob '{path}' max_size newly pinned to {new}: existing payloads may fail"),
        ));
    }
    if old_media != new_media {
        findings.push(CompatFinding::error(
            code::TYPE_CHANGED,
            path,
            format!(
                "blob '{path}' media type changed from {} to {}",
                old_media.as_deref().unwrap_or("(unset)"),
                new_media.as_deref().unwrap_or("(unset)"),
            ),
        ));
    }
}

/// Version gates: the new version must not regress, and `min_reader`
/// must not leapfrog the old version (that orphans old readers).
fn check_version_gates(old: &Module, new: &Module, findings: &mut Vec<CompatFinding>) {
    let old_meta = schema_meta(old);
    let new_meta = schema_meta(new);
    if let (Some(old_v), Some(new_v)) = (&old_meta.version, &new_meta.version) {
        if let (Some(o), Some(n)) = (parse_version(old_v), parse_version(new_v)) {
            if n < o {
                findings.push(CompatFinding::error(
                    code::VERSION_REGRESSED,
                    "schema.version",
                    format!("version regressed from {old_v} to {new_v}"),
                ));
            }
        }
    }
    if let (Some(old_v), Some(floor)) = (&old_meta.version, &new_meta.min_reader) {
        if let (Some(o), Some(f)) = (parse_version(old_v), parse_version(floor)) {
            if f > o {
                findings.push(CompatFinding::error(
                    code::READER_FLOOR_RAISED,
                    "schema.min_reader",
                    format!(
                        "min_reader {floor} exceeds old version {old_v}: old readers are dropped"
                    ),
                ));
            }
        }
    }
    if let (Some(old_v), Some(floor)) = (&old_meta.version, &new_meta.min_writer) {
        if let (Some(o), Some(f)) = (parse_version(old_v), parse_version(floor)) {
            if f > o {
                findings.push(CompatFinding::error(
                    code::READER_FLOOR_RAISED,
                    "schema.min_writer",
                    format!(
                        "min_writer {floor} exceeds old version {old_v}: old writers are dropped"
                    ),
                ));
            }
        }
    }
}
