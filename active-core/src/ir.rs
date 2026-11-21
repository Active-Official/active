//! Intermediate Representation and Dependency Graph

use indexmap::IndexMap;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::diag::{Diagnostic, Span};
use crate::types::{ActiveType, Constraint, Unit};
use crate::unify::PendingMerge;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone)]
pub struct Module {
    pub arena: Vec<Node>,
    pub bindings: HashMap<String, NodeId>,
    pub graph: DiGraph<NodeId, ()>,
    pub node_to_graph: HashMap<NodeId, NodeIndex>,
    pub root: Option<NodeId>,
    pub schemas: HashMap<String, NodeId>,
    pub diagnostics: Vec<Diagnostic>,
    pub source_name: String,
    /// SHA-256 of the main source bytes (L-P29 cache identity).
    pub source_sha256: String,
    /// Source spans for nodes, recorded by the parser so evaluation-time
    /// diagnostics can point at real positions.
    pub node_spans: HashMap<NodeId, Span>,
    /// Source file per node, parallel to `node_spans` (C-025 includes).
    pub node_files: HashMap<NodeId, String>,
    /// Healing diagnostics (H0xx): repairs the parser applied or declined,
    /// with confidence scores. Separate from `diagnostics` so callers can
    /// apply mode-dependent gating.
    pub healing: Vec<Diagnostic>,
    /// Comments with their normative attachment (spec/dual-syntax.md §7-§8),
    /// recorded so the formatter can re-emit them losslessly.
    pub trivia: Vec<TriviaComment>,
    /// The layout style the source predominantly used (for Preserve mode).
    pub source_style: Style,
    /// Include statements in source order (spec §2.7). The formatter
    /// re-emits them; evaluation merges them per the defaults rule, so
    /// the composed tree does not need them.
    pub includes: Vec<IncludeStmt>,
    /// Every resolved include's content pin (L-P03): spelling and the
    /// SHA-256 of the bytes read. The lockfile records these; the audit
    /// receipt shows what was verified.
    pub include_pins: Vec<IncludePin>,
    /// How each include was accepted during this parse (L-P03/L-P06):
    /// inline pin, lockfile path/content, or unpinned. Recorded in the
    /// audit receipt; empty when the document has no includes.
    pub include_resolutions: Vec<(IncludePin, IncludeResolutionKind)>,
    /// Named profiles in source order (spec §2.8, C-027). Never exported
    /// directly; the selected one merges into the root value at
    /// evaluation.
    pub profiles: IndexMap<String, NodeId>,
    /// Structural patch directives in source order (spec §2.9, C-028).
    /// Applied to the evaluated value tree after profile merge.
    pub patches: Vec<NodeId>,
    /// The document schema tree (spec §2.10, C-030): the root `schema`
    /// table and every nested one, extracted from value fields at parse.
    /// None when the document declares no schema. C-031 validates
    /// against it.
    pub schema: Option<NodeId>,
    /// Parsed `# @…` annotations by annotated node (spec/annotations.md,
    /// C-033): intent documentation, constraint predicates, and
    /// immutability flags for validators, the language server, and AI
    /// tooling. Comments themselves stay in `trivia` for the formatter.
    pub annotations: HashMap<NodeId, Vec<Annotation>>,
    /// Cross-field invariants declared by a root schema `check` block
    /// (L-P19, spec/validation.md section 11), keyed by the schema node
    /// that declared them. Only the effective root schema's rules run, so
    /// an included or overwritten schema can never contribute a rule.
    pub schema_checks: HashMap<NodeId, Vec<CheckRule>>,
    /// Every arena node belonging to a check predicate. The main walk
    /// skips them: predicates are evaluated after the transform pass,
    /// against the final tree, and their expression errors are T009, not
    /// fatal evaluation errors.
    pub check_nodes: std::collections::HashSet<NodeId>,
    /// Deferred same-priority merges recorded by the parser, keyed by the
    /// scope table node that owns them (L-P09). Their values are compared
    /// after evaluation: equal values agree, a disagreement is E046, and
    /// a table union replaces the kept first candidate at its path.
    pub merge_pending: HashMap<NodeId, Vec<PendingMerge>>,
    /// Extra bodies of a repeated table, in source order, keyed by the
    /// surviving first table (L-P09 phase 3). The parser merges their
    /// fields structurally and defers same-priority leaf collisions; the
    /// formatter re-emits each body as its own statement.
    pub repeated_tables: HashMap<NodeId, Vec<NodeId>>,
    /// The fields a repeated table's first body had before later bodies
    /// merged into it, for the formatter's statement-preserving output.
    pub body_fields: HashMap<NodeId, IndexMap<String, NodeId>>,
    /// Full document paths where a local body replaced an include-derived
    /// value (L-P09 phase 3). Deferred include merges below these paths are
    /// masked: the local value wins and the include pair is not compared.
    pub replaced_paths: std::collections::HashSet<String>,
    /// Dependency declarations by scope table (spec/linter.md §2,
    /// C-042): `depends_on` keys holding string arrays are extracted
    /// from value fields at parse (never exported) and recorded here
    /// against their scope's table node.
    pub depends_on: HashMap<NodeId, DependsOn>,
    /// Evaluation mode resolved at parse (PROPOSAL §0.2, Phase M):
    /// `Config` unless a root-level `active_mode = "data"` marker is
    /// present. The marker stays a regular value key (self-describing
    /// documents); the field lets the engine budget progressively
    /// without re-reading the tree.
    pub mode: Mode,
    /// Whether the ROOT document itself declares `active_mode` (a
    /// marker merged from an include, or a nested key with the same
    /// name, does not count). Tools report marker presence from this.
    pub mode_marker: bool,
    /// Declared capabilities (PROPOSAL §3.5, CHECKLIST CAP.001): the
    /// root-level `active_capabilities = [...]` string list a document
    /// asks the host for. Hosts may refuse excess requests (the
    /// `active capability --check-doc` handshake and `--sandbox`
    /// gate); evaluation itself never branches on this list.
    pub capabilities: Vec<String>,
    /// Whether the root document declared at least one brace-style
    /// table and at least one indentation-style table (spec/dual-syntax.md).
    /// Both styles are legal; mixing them in one document is a style
    /// smell the linter reports (L015). Included files are counted in
    /// their own parse, so this reflects the root file only.
    pub mixed_syntax: bool,
}

/// A parsed `# @…` annotation attached to a node (spec/annotations.md).
#[derive(Debug, Clone)]
pub struct Annotation {
    pub kind: AnnotationKind,
    /// Dotted path of the annotated key (scope path + key; bare key at
    /// root; binding name for `let`s) — the path-based enforcement
    /// passes (patches, constraints) work on paths.
    pub path: String,
    /// The annotation comment's span (diagnostics and LSP).
    pub span: Span,
}

/// Annotation payloads (spec/annotations.md §2–§4).
#[derive(Debug, Clone)]
pub enum AnnotationKind {
    /// Documentation only, never enforced.
    Intent(String),
    /// Boolean predicate with `value` bound to the annotated value.
    /// `source` is the raw payload (quoted in T006); `expr` is the
    /// parsed tree with `value` rewritten to the annotated node.
    Constraint { source: String, expr: NodeId },
    /// Agent-immutable flag; `false` behaves as no annotation.
    AiImmutable(bool),
}

/// One root-schema `check` invariant: a named boolean predicate over the
/// document's evaluated value (L-P19).
#[derive(Clone)]
pub struct CheckRule {
    /// Author-written rule name (diagnostics).
    pub name: String,
    /// The authored predicate text, preserved for the formatter so a
    /// dotted reference is never rewritten.
    pub source: String,
    /// The predicate expression node, parsed in the document's scope.
    pub expr: NodeId,
    /// The rule name's span (diagnostics).
    pub span: Span,
}

/// Redacted Debug: a rule name or predicate can quote material text;
/// logging a module must never print it.
impl std::fmt::Debug for CheckRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckRule")
            .field("name", &"<check>")
            .field("source", &"<source>")
            .field("expr", &self.expr)
            .field("span", &self.span)
            .finish()
    }
}

/// A parsed `depends_on` block for one table scope
/// (spec/linter.md §2).
#[derive(Debug, Clone)]
pub struct DependsOn {
    /// Declared dependency paths, dotted from the document root.
    pub paths: Vec<String>,
    /// The `depends_on` key's span (diagnostics).
    pub span: Span,
}
#[derive(Debug, Clone)]
pub struct IncludeStmt {
    /// Literal path as written.
    pub path: String,
    /// Optional `as alias` namespace.
    pub alias: Option<String>,
    /// Whether the included keys stay out of the exported tree.
    pub private: bool,
    /// Optional inline content pin: `include "x" sha256:<hex>`
    /// (L-P06). The digest is over the included file's bytes; a mismatch
    /// is a hard error (E041).
    pub expected_hash: Option<String>,
    /// Scope path where the statement appeared (empty = document root).
    pub scope_path: Vec<String>,
    /// True when the statement appeared inside a profile body. Profiles
    /// and value tables are separate namespaces with the same path text.
    pub profile: bool,
    /// Source line of the statement, for stable formatter emission.
    pub line: u32,
}

/// One resolved include's content pin (L-P03): the path as the document
/// spelled it, the root-relative identity the bytes were read from
/// (`..` for outside-root includes; absolute only across unrelated
/// roots), and the SHA-256 of those bytes. The lockfile records these so
/// eval can resolve an include by content and the audit trail can show
/// what was verified. `resolved` disambiguates one spelling used from
/// several directories and stays valid when the tree moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludePin {
    pub as_written: String,
    pub resolved: String,
    /// The exact path the parser stamped on the nodes (canonical on the
    /// filesystem, lexical-absolute under a host loader). Provenance
    /// reuses this to key parse-time digests without touching the disk.
    pub stamp: String,
    pub sha256: String,
}

/// Why an include was accepted during the locked/inline resolution
/// (recorded in the audit receipt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncludeResolutionKind {
    /// The document carried the matching inline `sha256:<hex>`.
    Inline,
    /// The lockfile pinned this include spelling to the matching digest.
    LockPath,
    /// The lockfile pinned a different spelling of the same content.
    LockContent,
    /// No lockfile pin applied (lock mode off, or the include is new).
    Unpinned,
}

/// One field's schema: a type plus its constraint arguments (spec §2.10,
/// C-030). Constraint values are expression nodes so bounds may use
/// literals, env refs, and arithmetic; C-031 evaluates them during
/// validation. `default` is parsed and stored; C-032 applies it.
/// `deprecated` is static text (C-042): `None` is live, `Some("")` is
/// a bare `deprecated = true`, `Some(msg)` carries the migration
/// message.
#[derive(Debug, Clone)]
pub struct SchemaField {
    pub typ: ActiveType,
    pub min: Option<NodeId>,
    pub max: Option<NodeId>,
    pub min_len: Option<NodeId>,
    pub max_len: Option<NodeId>,
    pub pattern: Option<NodeId>,
    pub format: Option<NodeId>,
    pub description: Option<NodeId>,
    pub enum_descriptions: Option<NodeId>,
    pub requires: Option<NodeId>,
    pub default: Option<NodeId>,
    /// Removal schedule (PROPOSAL §2.4, CHECKLIST SC.005): the
    /// `major.minor.patch` version this field is removed in. The
    /// compatibility surface treats removal at-or-before the new
    /// version as scheduled (info), earlier removal as breaking.
    /// Parsed from `removed_in = "1.2.0"`; malformed strings are E020.
    pub removed_in: Option<String>,
    /// Blob-only constraints (spec/blob.md §4): expected media type
    /// (`media_type = "..."`) and maximum payload bytes
    /// (`max_size = N`). Ignored on non-Blob types (E020 at parse).
    pub blob_media_type: Option<NodeId>,
    pub blob_max_size: Option<NodeId>,
    pub deprecated: Option<String>,
    /// Disjunction branches (L-P10), in source order: schema nodes
    /// (`SchemaField`, `SchemaTable`, or `SchemaArray`). Empty for an
    /// ordinary field; non-empty iff `typ == ActiveType::OneOf`.
    pub one_of: Vec<NodeId>,
    pub span: Option<crate::diag::Span>,
}

impl SchemaField {
    /// A field with no constraint arguments.
    pub fn bare(typ: ActiveType, span: Option<crate::diag::Span>) -> Self {
        Self {
            typ,
            min: None,
            max: None,
            min_len: None,
            max_len: None,
            pattern: None,
            format: None,
            description: None,
            enum_descriptions: None,
            requires: None,
            default: None,
            removed_in: None,
            blob_media_type: None,
            blob_max_size: None,
            deprecated: None,
            one_of: Vec::new(),
            span,
        }
    }

    /// Human-readable type for diagnostics and generated docs: the full
    /// disjunction when one is present, else the leaf name with its
    /// literal constraints (`Int(min = 1)`), so a branch list explains
    /// itself. The canonical source form stays the formatter's job.
    pub fn type_label(&self, module: &Module) -> String {
        if !self.one_of.is_empty() {
            let branches: Vec<String> = self
                .one_of
                .iter()
                .map(|id| schema_node_label(module, *id))
                .collect();
            return format!("OneOf({})", branches.join(", "));
        }
        let mut label = self.typ.name();
        let mut args: Vec<String> = Vec::new();
        for (name, slot) in [
            ("min", self.min),
            ("max", self.max),
            ("min_len", self.min_len),
            ("max_len", self.max_len),
            ("pattern", self.pattern),
            ("format", self.format),
        ] {
            if let Some(text) = slot.and_then(|id| literal_label(module, id)) {
                args.push(format!("{name} = {text}"));
            }
        }
        if !args.is_empty() {
            label.push('(');
            label.push_str(&args.join(", "));
            label.push(')');
        }
        label
    }

    /// A structural key for a leaf schema branch (L-P10 dedup): the type
    /// label plus every constraint argument. A computed argument is keyed
    /// by its *expression signature* (so two documents with the same
    /// expression compare equal), falling back to the node id when the
    /// expression is opaque.
    pub fn branch_key(&self, module: &Module) -> String {
        let mut key = self.type_label(module);
        key.push_str(&self.arguments_key(module));
        key
    }

    /// The constraint-argument part of [`SchemaField::branch_key`], without
    /// the type label. A disjunction's open key rebuilds its type label
    /// from its branches (L-P11), so it needs the arguments separately.
    pub(crate) fn arguments_key(&self, module: &Module) -> String {
        let mut key = String::new();
        for (name, slot) in [
            ("min", self.min),
            ("max", self.max),
            ("min_len", self.min_len),
            ("max_len", self.max_len),
            ("pattern", self.pattern),
            ("format", self.format),
            ("descriptions", self.enum_descriptions),
            ("media_type", self.blob_media_type),
            ("max_size", self.blob_max_size),
            ("requires", self.requires),
            ("default", self.default),
            ("description", self.description),
        ] {
            let Some(id) = slot else { continue };
            match literal_label(module, id) {
                Some(text) => {
                    key.push('|');
                    key.push_str(name);
                    key.push('=');
                    key.push_str(&text);
                }
                None => {
                    key.push('|');
                    key.push_str(name);
                    match expr_signature(module, id) {
                        Some(text) => {
                            key.push_str("=@");
                            key.push_str(&text);
                        }
                        None => {
                            key.push_str("=?#");
                            key.push_str(&format!("{id:?}"));
                        }
                    }
                }
            }
        }
        if let Some(removed) = &self.removed_in {
            key.push_str(&format!("|removed_in={removed}"));
        }
        if let Some(deprecated) = &self.deprecated {
            key.push_str(&format!("|deprecated={deprecated}"));
        }
        key
    }

    /// Whether any constraint argument is computed in a way the key
    /// cannot render (a material literal or an opaque expression).
    pub fn branch_opaque(&self, module: &Module) -> bool {
        [
            self.min,
            self.max,
            self.min_len,
            self.max_len,
            self.pattern,
            self.format,
            self.enum_descriptions,
            self.blob_media_type,
            self.blob_max_size,
            self.requires,
            self.default,
            self.description,
        ]
        .into_iter()
        .flatten()
        .any(|id| literal_label(module, id).is_none() && expr_signature(module, id).is_none())
    }
}

/// A stable structural signature for an expression node: literals,
/// env/ident reads, refs, and binary/unary arithmetic. Material literals
/// and opaque nodes return `None` (their content must not be rendered,
/// and two of them cannot be compared).
pub fn expr_signature(module: &Module, id: NodeId) -> Option<String> {
    match module.get(id) {
        Some(Node::Literal(_)) => literal_label(module, id),
        Some(Node::Env { name, default }) => match default {
            Some(default) => Some(format!(
                "env.{name} ?? {}",
                expr_signature(module, *default)?
            )),
            None => Some(format!("env.{name}")),
        },
        Some(Node::Ident(name)) => Some(name.clone()),
        Some(Node::Ref(to)) => expr_signature(module, *to),
        Some(Node::Binary { op, lhs, rhs }) => Some(format!(
            "({} {:?} {})",
            expr_signature(module, *lhs)?,
            op,
            expr_signature(module, *rhs)?
        )),
        Some(Node::Unary { op, expr }) => {
            Some(format!("{:?}({})", op, expr_signature(module, *expr)?))
        }
        _ => None,
    }
}

/// A literal node rendered for a type label: numbers, booleans, and
/// quotes strings; anything computed is omitted (a label is not an
/// evaluation).
fn literal_label(module: &Module, id: NodeId) -> Option<String> {
    match module.get(id) {
        Some(Node::Literal(Value::Int(n))) => Some(n.to_string()),
        Some(Node::Literal(Value::Float(f))) => Some(f.to_string()),
        Some(Node::Literal(Value::String(text))) => Some(format!("{text:?}")),
        Some(Node::Literal(Value::Bool(flag))) => Some(flag.to_string()),
        _ => None,
    }
}

/// The label of one schema node inside a disjunction: a leaf type, a
/// nested table, or an array of an element label.
pub fn schema_node_label(module: &Module, id: NodeId) -> String {
    match module.get(id) {
        Some(Node::SchemaField { field }) => field.type_label(module),
        Some(Node::SchemaTable { closed, .. }) => {
            if *closed {
                "Table(closed)".to_string()
            } else {
                "Table".to_string()
            }
        }
        Some(Node::SchemaArray { element }) => {
            format!("Array<{}>", schema_node_label(module, *element))
        }
        _ => "schema".to_string(),
    }
}

/// A structural key for a schema node: leaf branches include every
/// constraint (computed slots by node id), tables include their fields,
/// arrays include their element. Two nodes with equal keys accept the
/// same values, so a disjunction can deduplicate them without collapsing
/// structurally different branches.
pub fn schema_node_key(module: &Module, id: NodeId) -> String {
    match module.get(id) {
        Some(Node::SchemaField { field }) => field.branch_key(module),
        Some(Node::SchemaTable { fields, closed, .. }) => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(name, child)| format!("{name}:{}", schema_node_key(module, *child)))
                .collect();
            parts.sort();
            format!("Table({closed}){{{}}}", parts.join(","))
        }
        Some(Node::SchemaArray { element }) => {
            format!("Array<{}>", schema_node_key(module, *element))
        }
        _ => "schema".to_string(),
    }
}

/// The structural key of a schema node with every table's `closed` flag
/// ignored (L-P11). Two disjunction branches with equal open keys differ
/// only in closedness, so the compatibility check can pair them and
/// report one close/open change (C046) instead of a removed branch.
pub(crate) fn schema_node_key_open(module: &Module, id: NodeId) -> String {
    match module.get(id) {
        Some(Node::SchemaField { field }) if field.one_of.is_empty() => field.branch_key(module),
        Some(Node::SchemaField { field }) => {
            // Rebuild the type label from the branches with their closed
            // flags normalized, and keep the field's own arguments. A pure
            // nested close/open therefore compares equal.
            let mut parts: Vec<String> = field
                .one_of
                .iter()
                .map(|branch| schema_node_key_open(module, *branch))
                .collect();
            parts.sort();
            format!("OneOf({}){}", parts.join(","), field.arguments_key(module))
        }
        Some(Node::SchemaTable { fields, .. }) => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(name, child)| format!("{name}:{}", schema_node_key_open(module, *child)))
                .collect();
            parts.sort();
            format!("Table{{{}}}", parts.join(","))
        }
        Some(Node::SchemaArray { element }) => {
            format!("Array<{}>", schema_node_key_open(module, *element))
        }
        _ => "schema".to_string(),
    }
}

/// Every table's `closed` flag inside a schema node, keyed by its
/// position path (field names, `[]` array indices, `#shape` disjunction
/// alternatives). Paths make the comparison order-independent: branches
/// with equal shapes keep the same keys whichever order the document
/// lists them in, and a per-path count reports the direction precisely.
pub(crate) fn schema_node_closed_flags(module: &Module, id: NodeId) -> Vec<(String, bool)> {
    fn collect(module: &Module, id: NodeId, prefix: &str, out: &mut Vec<(String, bool)>) {
        match module.get(id) {
            Some(Node::SchemaTable { fields, closed, .. }) => {
                out.push((prefix.to_string(), *closed));
                let mut names: Vec<&String> = fields.keys().collect();
                names.sort();
                for name in names {
                    let child = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    collect(module, fields[name], &child, out);
                }
            }
            Some(Node::SchemaArray { element }) => {
                collect(module, *element, &format!("{prefix}[]"), out);
            }
            Some(Node::SchemaField { field }) if !field.one_of.is_empty() => {
                // Alternatives are a set: the branch's shape is its
                // identity, so the same shape has the same path in both
                // documents.
                for branch in &field.one_of {
                    let shape = schema_node_key_shape(module, *branch);
                    let child = format!("{prefix}#{shape}");
                    collect(module, *branch, &child, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    collect(module, id, "", &mut out);
    out.sort();
    out
}

/// The type skeleton of a schema node (L-P11): like [`schema_node_key_open`]
/// but with constraint arguments and enum members ignored too. Two
/// branches with equal skeletons describe the same shape, so a compat
/// check can pair them, report a closedness change (C046), and still let
/// the branch-set comparison report a simultaneous constraint tightening
/// (C042) independently.
pub(crate) fn schema_node_key_shape(module: &Module, id: NodeId) -> String {
    match module.get(id) {
        Some(Node::SchemaField { field }) if field.one_of.is_empty() => field_shape(&field.typ),
        Some(Node::SchemaField { field }) => {
            // A nested disjunction is flattened by the parser; this is
            // defensive. Branches are a sorted set (order is not a
            // change).
            let mut parts: Vec<String> = field
                .one_of
                .iter()
                .map(|branch| schema_node_key_shape(module, *branch))
                .collect();
            parts.sort();
            format!("OneOf({})", parts.join(","))
        }
        Some(Node::SchemaTable { fields, .. }) => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(name, child)| format!("{name}:{}", schema_node_key_shape(module, *child)))
                .collect();
            parts.sort();
            format!("Table{{{}}}", parts.join(","))
        }
        Some(Node::SchemaArray { element }) => {
            format!("Array<{}>", schema_node_key_shape(module, *element))
        }
        _ => "schema".to_string(),
    }
}

/// The kind of a leaf field, with members, bounds, and the closed marker
/// removed: the part of a shape that cannot change without making it a
/// different branch.
fn field_shape(typ: &crate::types::ActiveType) -> String {
    match typ {
        crate::types::ActiveType::Enum(_) => "Enum".to_string(),
        crate::types::ActiveType::Table { .. } => "Table".to_string(),
        other => other.name(),
    }
}

/// Schema evolution metadata (PROPOSAL §2.4, CHECKLIST SC.005):
/// the document's own version plus the oldest reader/writer that
/// can still process it. All three are `major.minor.patch` strings;
/// the parser rejects malformed versions with E020.
#[derive(Debug, Clone, Default)]
pub struct SchemaMeta {
    pub version: Option<String>,
    pub min_reader: Option<String>,
    pub min_writer: Option<String>,
}

/// Evaluation mode (PROPOSAL §0.2, CHECKLIST Phase M): Config is the
/// default and stays 1.0-compatible; Data is explicit opt-in via an
/// `active_mode = "data"` marker key. Both modes share the IR, type
/// system, taint, budgets, and conformance guarantees — Data only
/// adds progressive budget warnings (hard ceilings never move).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Config,
    Data,
}

/// Layout style for output (spec/dual-syntax.md §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// `[name]` headers and `key = value`; nested tables as `name { … }`.
    Brace,
    /// `key:` headers with two-space indentation.
    Indent,
    /// Re-emit in the style the source used.
    Preserve,
}

/// Where a comment belongs (spec/dual-syntax.md §7.2). Every comment
/// receives exactly one attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attachment {
    /// Own-line comment(s) before a statement, identified by key.
    Leading(String),
    /// After code on the same line as a statement.
    Trailing(String),
    /// No following statement in that block.
    TrailingBlock(String),
    /// No following statement in the document.
    TrailingDocument,
}

/// A source comment retained for lossless round-tripping.
#[derive(Debug, Clone)]
pub struct TriviaComment {
    pub text: String,
    pub span: Span,
    pub attachment: Attachment,
    /// Source file (includes parse as their own documents, C-025):
    /// line numbers repeat across files, so attachment scans match
    /// (line, file) — without this, comments in an included file
    /// attach to same-numbered lines of the including document.
    pub file: Option<String>,
}

impl Module {
    pub fn new(source_name: impl Into<String>) -> Self {
        Self {
            arena: Vec::new(),
            bindings: HashMap::new(),
            graph: DiGraph::new(),
            node_to_graph: HashMap::new(),
            root: None,
            schemas: HashMap::new(),
            diagnostics: Vec::new(),
            source_name: source_name.into(),
            source_sha256: String::new(),
            node_spans: HashMap::new(),
            node_files: HashMap::new(),
            healing: Vec::new(),
            trivia: Vec::new(),
            source_style: Style::Brace,
            includes: Vec::new(),
            include_pins: Vec::new(),
            include_resolutions: Vec::new(),
            profiles: IndexMap::new(),
            patches: Vec::new(),
            schema: None,
            annotations: HashMap::new(),
            schema_checks: HashMap::new(),
            check_nodes: std::collections::HashSet::new(),
            merge_pending: HashMap::new(),
            repeated_tables: HashMap::new(),
            body_fields: HashMap::new(),
            replaced_paths: std::collections::HashSet::new(),
            depends_on: HashMap::new(),
            mode: Mode::default(),
            mode_marker: false,
            capabilities: Vec::new(),
            mixed_syntax: false,
        }
    }

    pub fn add_node(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.arena.len() as u32);
        self.arena.push(node);
        let idx = self.graph.add_node(id);
        self.node_to_graph.insert(id, idx);
        id
    }

    pub fn add_edge(&mut self, from: NodeId, to: NodeId) {
        if let (Some(&f), Some(&t)) = (self.node_to_graph.get(&from), self.node_to_graph.get(&to)) {
            // Edge direction: from dependency → dependent
            // so topological order evaluates dependencies first
            self.graph.add_edge(f, t, ());
        }
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.arena.get(id.0 as usize)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.arena.get_mut(id.0 as usize)
    }

    /// Record the source span of a node for evaluation-time diagnostics.
    pub fn record_span(&mut self, id: NodeId, span: Span) {
        self.node_spans.insert(id, span);
    }

    /// Record the source file of a node (includes, C-025).
    pub fn record_file(&mut self, id: NodeId, file: String) {
        self.node_files.insert(id, file);
    }

    /// Dotted paths for value nodes walked from the document root
    /// (`server.port`), plus binding names for `let` values. Unmapped
    /// nodes (schema internals, constraint expressions, orphans) are
    /// absent: callers skip what they cannot name rather than
    /// guessing. Shared by the linter and the language server (C-043
    /// hover/definition).
    pub fn value_paths(&self) -> HashMap<NodeId, String> {
        let mut paths = HashMap::new();
        for (name, id) in &self.bindings {
            paths.insert(*id, name.clone());
        }
        if let Some(root) = self.root {
            let mut stack = vec![(root, String::new())];
            while let Some((id, path)) = stack.pop() {
                if let Some(Node::Table { fields }) = self.get(id) {
                    for (key, child) in fields {
                        let child_path = if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        };
                        paths.entry(*child).or_insert_with(|| child_path.clone());
                        stack.push((*child, child_path));
                    }
                }
            }
        }
        paths
    }

    /// Resolve a dotted path from the document root to a value-table
    /// node (C-042): each segment indexes a `Node::Table`'s fields.
    /// A single segment additionally resolves `let` bindings (table
    /// fields win ties). Returns None for missing keys, non-table
    /// hops, and a missing root. Shared by graph construction
    /// (`depends_on` edges) and the linter (dangling-path detection),
    /// so both agree on what resolves.
    pub fn resolve_value_path(&self, path: &str) -> Option<NodeId> {
        if let Some(root) = self.root {
            let mut id = root;
            let mut found = true;
            for segment in path.split('.') {
                match self.get(id) {
                    Some(Node::Table { fields }) => match fields.get(segment) {
                        Some(child) => id = *child,
                        None => {
                            found = false;
                            break;
                        }
                    },
                    _ => {
                        found = false;
                        break;
                    }
                }
            }
            if found {
                return Some(id);
            }
        }
        if !path.contains('.') {
            return self.bindings.get(path).copied();
        }
        None
    }
}

#[derive(Debug, Clone)]
pub enum Node {
    Literal(Value),
    Env {
        name: String,
        default: Option<NodeId>,
    },
    Secret {
        locator: NodeId,
    },
    /// Content-addressed blob construction site (PROPOSAL §1.1,
    /// spec/blob.md): `blob(media_type, data)`. The hash, size, and
    /// taint are computed at evaluation from the data node; `payload`
    /// retains the data node for demand-driven materialisation.
    ContentAddressed {
        media_type: NodeId,
        data: NodeId,
    },
    Ref(NodeId),
    Ident(String), // unresolved name, resolved during graph build
    Interpolate {
        parts: Vec<InterpPart>,
    },
    Binary {
        op: BinOp,
        lhs: NodeId,
        rhs: NodeId,
    },
    Unary {
        op: UnOp,
        expr: NodeId,
    },
    Array {
        elements: Vec<NodeId>,
    },
    Table {
        fields: IndexMap<String, NodeId>,
    },
    Quantity {
        value: NodeId,
        unit: Unit,
    },
    ApplyConstraint {
        target: NodeId,
        constraint: Constraint,
    },
    /// A structural patch directive (spec §2.9, C-028): one operation
    /// applied to the evaluated value tree after evaluation (and after
    /// profile merge), before schema validation. The value expressions
    /// are ordinary graph nodes, so patches are dependency-aware.
    PatchDirective {
        op: PatchOp,
    },
    /// One field's schema (spec §2.10, C-030). The constraint arguments
    /// are expression nodes evaluated with the document; C-031 reads
    /// their values during validation. Edges run argument → field.
    SchemaField {
        field: SchemaField,
    },
    /// A table of named schemas (spec §2.10). Fields hold SchemaField
    /// or nested SchemaTable nodes. Extracted from value fields at
    /// parse, so schema tables never export as data.
    SchemaTable {
        fields: IndexMap<String, NodeId>,
        closed: bool,
        /// Schema evolution metadata (PROPOSAL §2.4): reserved
        /// `version` / `min_reader` / `min_writer` string keys inside
        /// the schema block. Absent by default; the compatibility
        /// surface reads them.
        meta: SchemaMeta,
    },
    /// An array element schema (spec §2.10): `element` is a SchemaField
    /// node carrying the full element constraints.
    SchemaArray {
        element: NodeId,
    },
    /// A multi-line string carrying a semantic tag (`@prompt`, `@policy`,
    /// `@template`). The tag is tool treatment only — evaluation is
    /// transparent to the inner string value (spec §1.4.1).
    Semantic {
        tag: SemanticTag,
        inner: NodeId,
    },
}

/// Semantic-block tag for multi-line strings (spec §1.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticTag {
    Prompt,
    Policy,
    Template,
}

impl SemanticTag {
    pub fn as_str(&self) -> &'static str {
        match self {
            SemanticTag::Prompt => "prompt",
            SemanticTag::Policy => "policy",
            SemanticTag::Template => "template",
        }
    }

    pub fn parse_tag(s: &str) -> Option<Self> {
        match s {
            "prompt" => Some(SemanticTag::Prompt),
            "policy" => Some(SemanticTag::Policy),
            "template" => Some(SemanticTag::Template),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum InterpPart {
    Literal(String),
    Expr(NodeId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    DurationMs(u64),     // normalized to milliseconds for simplicity in alpha
    Quantity(f64, Unit), // rate/custom units retain their dimension
    Array(Vec<Value>),
    Table(IndexMap<String, Value>),
    /// Secret material: scrubbed on drop by `Zeroizing` (LANG-I-005). The
    /// bytes exist in plaintext while the value is live - the redaction
    /// contract governs every output path, and Rust cannot hash a secret
    /// into unreachability - but they do not survive the value.
    Secret(zeroize::Zeroizing<String>),
    /// Content-addressed blob identity (PROPOSAL §1.1, spec/blob.md).
    /// The payload is present only after explicit materialisation;
    /// tainted blobs never carry a payload. Hashes never appear in
    /// ordinary Active source — only here, in evaluated values.
    ///
    /// Invariant for hand-built values: the evaluator marks a blob
    /// `tainted` whenever its media type, payload, or serving path is
    /// material, so a retained payload is non-material. A host that
    /// constructs this variant directly must uphold the same rule: the
    /// profile-unaware serializers (`Serialize`, `value_to_json`) are
    /// type dumps and cannot classify content, while the profile-aware
    /// exporters (`convert::redact_value`, `*_with`) do.
    Blob {
        hash: [u8; 32],
        size: u64,
        tainted: bool,
        media_type: String,
        payload: Option<Vec<u8>>,
    },
}

impl std::fmt::Debug for Value {
    /// Redacting formatter (spec/secrets.md §3): secrets render as
    /// `Secret(***)` at any depth, so generic `{:?}` logging,
    /// assertion output, and error messages can never leak secret
    /// material. Every other variant keeps its derived shape exactly.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Secret(_) => write!(f, "Secret(***)"),
            Value::Array(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item:?}")?;
                }
                write!(f, "]")
            }
            Value::Table(map) => {
                write!(f, "{{")?;
                for (i, (k, val)) in map.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k:?}: {val:?}")?;
                }
                write!(f, "}}")
            }
            Value::Null => write!(f, "Null"),
            Value::Bool(b) => write!(f, "{b:?}"),
            Value::Int(i) => write!(f, "{i:?}"),
            Value::Float(x) => write!(f, "{x:?}"),
            Value::String(s) => write!(f, "{s:?}"),
            Value::DurationMs(ms) => write!(f, "DurationMs({ms})"),
            Value::Quantity(n, unit) => write!(f, "Quantity({n:?}, {unit:?})"),
            // Identity only: hashes are metadata, payloads never leak
            // through Debug (tainted or not).
            Value::Blob {
                hash,
                size,
                media_type,
                tainted,
                payload,
            } => {
                let identity =
                    crate::blob::verified_identity(hash, *tainted, media_type, payload.as_ref());
                write!(
                    f,
                    "Blob(blake3:{}, size={size}, media={:?}, tainted={tainted})",
                    crate::blob::hash_hex(&identity),
                    crate::secret::exposed_media_type(*tainted, media_type)
                )
            }
        }
    }
}

/// Reserved wire-wrapper keys. A DATA table whose key spells one of these
/// would serialize exactly like the typed wrapper (a host can build such a
/// table; source keys are identifiers and cannot contain `$`). Escaping the
/// data key keeps both serializers injective and keeps `content_hash` from
/// giving a table and a duration the same identity (LANG-F-001/F-002).
const RESERVED_WIRE_TAGS: [&str; 4] = [
    "$active.duration",
    "$active.quantity",
    "$active.blob",
    "$active.float",
];

/// A data-table key, escaped when it spells a reserved wrapper tag or
/// already starts with the escape character. The rule is a prefix code:
/// a reserved key and a key that literally begins with `\` can never
/// encode to the same string (host-built tables can carry any key, and
/// two distinct values must not share a wire form or a `content_hash`).
/// Shared by `convert::value_to_json`, the TOML exporter, and the
/// `Serialize` impl below, so every wire path agrees.
pub(crate) fn wire_key(key: &str) -> String {
    if key.starts_with('\\') || RESERVED_WIRE_TAGS.contains(&key) {
        format!("\\{key}")
    } else {
        key.to_owned()
    }
}

impl Serialize for Value {
    /// Redacting serializer (spec/secrets.md §3): serializing a Value
    /// directly (`serde_json::to_string` and friends) can never emit
    /// secret material — secrets become the export marker, mirroring
    /// `convert::value_to_json` (kept in sync by hand; ir cannot depend
    /// on convert). Deserialization stays derived: the input direction
    /// cannot leak.
    ///
    /// Blob payloads are the exception to "direct serialization is
    /// profile-aware" (LANG-I-010): an untainted materialised payload
    /// emits `payload_b64` here, because this impl cannot see the
    /// `SecretProfile` that would redact a profile-selected path. Library
    /// hosts must route exported values through
    /// [`crate::convert::redact_value`] with the profile that produced the
    /// tree (the pattern every shipped exporter follows):
    ///
    /// ```
    /// # use active_core::{parse, Engine};
    /// # let module = parse("port = 8080\n", "doc").expect("parse");
    /// # let engine = Engine::hermetic();
    /// # let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("eval");
    /// let profile = active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds);
    /// let redacted = active_core::convert::redact_value(&profile, &value);
    /// let json = active_core::convert::value_to_pretty_json(&redacted);
    /// # assert!(json.contains("8080"));
    /// ```
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let json = match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => serde_json::Value::from(*b),
            Value::Int(i) => serde_json::Value::from(*i),
            Value::Float(x) if x.is_finite() => serde_json::Value::from(*x),
            // Never a bare `null` for a non-finite value (LANG-F-003):
            // the tagged object cannot collide with null or with "inf".
            Value::Float(x) => serde_json::json!({
                "$active.float": if x.is_nan() {
                    "nan"
                } else if x.is_sign_negative() {
                    "-inf"
                } else {
                    "inf"
                }
            }),
            Value::String(s) => serde_json::Value::from(s.clone()),
            Value::DurationMs(ms) => serde_json::json!({ "$active.duration": { "ms": ms } }),
            // Same reserved wrapper as `convert::value_to_json`: the bare
            // `{ "<suffix>": n }` form was table-shaped and not injective
            // (LANG-B-019).
            Value::Quantity(n, unit) => serde_json::json!({
                "$active.quantity": { "unit": unit.suffix(), "value": n }
            }),
            Value::Array(items) => serde_json::Value::Array(
                items
                    .iter()
                    .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
                    .collect(),
            ),
            Value::Table(map) => {
                let mut obj = serde_json::Map::new();
                for (k, val) in map {
                    obj.insert(
                        wire_key(k),
                        serde_json::to_value(val).unwrap_or(serde_json::Value::Null),
                    );
                }
                serde_json::Value::Object(obj)
            }
            Value::Secret(_) => serde_json::Value::from("***SECRET***"),
            // Identity shape (spec/blob.md §5): hash, size, media type,
            // taint bit. Payload bytes are NEVER emitted here: this impl
            // is profile-unaware and a generic serializer must not be a
            // payload path. Explicit materialisation uses
            // `convert::value_to_json_materialised` / the `*_materialised`
            // exporters, after profile redaction.
            Value::Blob {
                hash,
                size,
                tainted,
                media_type,
                payload,
            } => {
                // A retained payload verifies the identity.
                let identity =
                    crate::blob::verified_identity(hash, *tainted, media_type, payload.as_ref());
                let mut inner = serde_json::Map::new();
                inner.insert(
                    "hash".to_string(),
                    serde_json::Value::from(format!("blake3:{}", crate::blob::hash_hex(&identity))),
                );
                inner.insert("size".to_string(), serde_json::Value::from(*size));
                // A tainted identity may carry a material media type
                // (an env lookup used as a locator); fail closed and
                // redact it. `tainted` itself tells consumers why.
                inner.insert(
                    "media_type".to_string(),
                    serde_json::Value::from(if *tainted {
                        crate::secret::MARKER.to_string()
                    } else {
                        media_type.clone()
                    }),
                );
                inner.insert("tainted".to_string(), serde_json::Value::from(*tainted));
                serde_json::Value::Object(
                    [("$active.blob".to_string(), serde_json::Value::Object(inner))]
                        .into_iter()
                        .collect(),
                )
            }
        };
        json.serialize(serializer)
    }
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum PatchOp {
    Set { path: String, value: NodeId },
    Remove { path: String },
    Rename { from: String, to: String },
}
