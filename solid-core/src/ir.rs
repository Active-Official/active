//! Intermediate Representation and Dependency Graph

use indexmap::IndexMap;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::diag::{Diagnostic, Span};
use crate::types::{Constraint, SolidType, Unit};

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

/// An `include "path" [as alias] [private]` statement (spec §2.7).
#[derive(Debug, Clone)]
pub struct IncludeStmt {
    /// Literal path as written.
    pub path: String,
    /// Optional `as alias` namespace.
    pub alias: Option<String>,
    /// Whether the included keys stay out of the exported tree.
    pub private: bool,
    /// Scope path where the statement appeared (empty = document root).
    pub scope_path: Vec<String>,
    /// Source line of the statement, for stable formatter emission.
    pub line: u32,
}

/// One field's schema: a type plus its constraint arguments (spec §2.10,
/// C-030). Constraint values are expression nodes so bounds may use
/// literals, env refs, and arithmetic; C-031 evaluates them during
/// validation. `default` is parsed and stored; C-032 applies it.
#[derive(Debug, Clone)]
pub struct SchemaField {
    pub typ: SolidType,
    pub min: Option<NodeId>,
    pub max: Option<NodeId>,
    pub min_len: Option<NodeId>,
    pub max_len: Option<NodeId>,
    pub pattern: Option<NodeId>,
    pub default: Option<NodeId>,
    pub span: Option<crate::diag::Span>,
}

impl SchemaField {
    /// A field with no constraint arguments.
    pub fn bare(typ: SolidType, span: Option<crate::diag::Span>) -> Self {
        Self {
            typ,
            min: None,
            max: None,
            min_len: None,
            max_len: None,
            pattern: None,
            default: None,
            span,
        }
    }
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
            node_spans: HashMap::new(),
            node_files: HashMap::new(),
            healing: Vec::new(),
            trivia: Vec::new(),
            source_style: Style::Brace,
            includes: Vec::new(),
            profiles: IndexMap::new(),
            patches: Vec::new(),
            schema: None,
            annotations: HashMap::new(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    Secret(String), // in production this is zeroized; here plain for demo
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
