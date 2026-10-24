# ACTIVE Annotations: @intent, @constraint, @ai_immutable (C-033/C-034)

Annotations are `#` comments on the lines immediately preceding a
binding, table, or `let`, parsed into IR and enforced. They are the
machine-readable contract layer for humans, agents, and tooling
(P-031).

## 1. Surface syntax

```active
# @intent: "Public HTTP port; must stay below 1024 in prod"
# @constraint: value >= 1 && value <= 65535
# @ai_immutable: true
port = 8080
```

- Each annotation is one comment line: `#`, optional whitespace, `@`,
  the exact kind name, a required `:`, then the payload.
- The annotated statement is the first statement below a contiguous
  comment block (comment lines with no blank or code gap between
  them). Plain comments may sit inside the block; they stay comments.
  A comment sharing its line with code (trailing) never opens or
  extends a block — it belongs to its own line.
- A `# @…` line with any other kind name (e.g. `# @todo`) is an
  ordinary comment, never an error.
- Annotations attach to key-values, `let` bindings, and tables
  (headers, brace blocks, indented blocks). They do not attach to
  `schema` blocks, profiles, includes, or patch directives.

## 2. @intent

- Payload: a string literal (`"…"` or `'…'`) with the language's
  string escapes — or bare prose, which attaches verbatim. A payload
  that opens a quote is held to literal rules (E021 on failure); an
  empty payload is E021. Documentation must never punish informality,
  and older documents already used free-text `@intent` comments.
- Semantics: documentation only — origin, rationale, operational
  guidance. Never enforced, never evaluated. Surfaced by hover (LSP),
  documentation generation, and AI tooling.

## 3. @constraint

- Payload: a boolean expression over the language's pure expression
  grammar (comparisons, boolean connectives, arithmetic, `let`
  references, `env` refs, literals). It is parsed with the normal
  expression parser, so every operator behaves identically.
- `value` denotes the annotated value as a whole. It shadows any
  `let` named `value` inside constraint payloads (deterministic rule).
  Other identifiers resolve exactly like expression identifiers
  (siblings, then `let` bindings, else W001 + null).
- There is deliberately **no** implicit-subject form: P-031's
  `<= os.cpu_count()` sketch and bare predicate names (`odd`) are not
  accepted — write `value <= os.cpu_count()` and
  `value % 2 == 1`. Accepting P-031's sketch would require new
  expression syntax; full boolean expressions cover it without
  grammar growth (logged deviation, see Memories-and-Observations).
- The payload must parse as one complete expression (trailing input
  is E021) and must evaluate to a boolean (T006 otherwise).
- Constraints are pure by construction (the expression language has
  no I/O) and budget-limited by the existing expression depth guard
  plus DAG evaluation — no new budget machinery.
- Enforcement runs after schema validation, in walk order, under the
  same strict/resilient policy: strict collects every constraint
  failure and fails; resilient collects alongside the tree.
- Predicates evaluate against **post-transform values**: patches,
  profile merges, and default fills apply first, and the predicate
  sees the final tree (re-evaluated, not the pre-transform graph
  result). A constraint on a patched-away key is skipped.
- Annotations attach at document level only — never inside profile
  bodies (variant contracts are future work; profile keys keep their
  comments as plain trivia). Included files contribute their
  annotations with file attribution; aliased includes scope paths
  under the alias so enforcement matches the merged tree.

## 4. @ai_immutable

- Payload: the literal `true` or `false` — or nothing at all, in
  which case the bare flag means `true` (older documents used it as a
  plain comment; annotations must never invalidate them). Anything
  else is E021. `false` is explicit mutability and behaves as no
  annotation.
- `true` marks the key (and, for tables, its whole subtree) as
  agent-immutable: structural patches (`@patch`, `@remove`,
  `@rename`) whose target path overlaps the annotated path — in
  either direction, compared segment-wise — fail with E019, quoting
  the path. Patches are the automated-edit path; that is what
  immutability guards.
- Out of scope (documented): profile overrides of immutable keys and
  resilient default fills are not patch edits and are allowed. LSP
  surfaces the flag for both.

## 5. IR

`Module.annotations: HashMap<NodeId, Vec<Annotation>>` side table
(parallel to `node_spans`):

```rust
Annotation { kind: AnnotationKind, path: String, span: Span }
AnnotationKind::Intent(String)
AnnotationKind::Constraint { source: String, expr: NodeId }
AnnotationKind::AiImmutable(bool)
```

- `path` is the dotted path of the annotated key (scope path + key;
  bare key at root; binding name for `let`s) — the enforcement
  passes work on paths.
- `span` is the annotation comment's span (for diagnostics and LSP).
- `source` is the raw constraint payload (quoted in T006; it is
  author-written and safe).
- `expr` is the parsed predicate with `value` rewritten to a direct
  reference to the annotated node, participating in the dependency
  graph like any expression (edges keep evaluation order).

## 6. Diagnostics

- E021 `invalid @<kind> annotation: <detail>` (parse, comment span):
  bad string, non-boolean `ai_immutable`, unparsable or trailing
  constraint payload.
- T006 `constraint violation at '<path>': <source> does not hold
  (actual: <rendered>)` — `<source>` is the raw payload, truncated at
  120 characters; predicates containing `secret(…)` render as the
  generic `constraint` so no locator or literal can leak. `<rendered>`
  is the actual value rendered safely: a material actual of any kind
  (a value used as a secret locator, or built from one) renders
  `***SECRET***`; a string merely containing a known secret is
  redacted in place; otherwise strings quote with debug escaping and
  output truncates at 120 characters with an ellipsis.
- T006 `constraint violation at '<path>': <source> must evaluate to
  a boolean, got <kind>` — non-boolean predicate result.
- E019 `patch target '<path>' overlaps ai_immutable '<apath>'` —
  immutable patch overlap, at the patch directive's span.

## 7. Tooling notes

- Annotations remain comments in the trivia stream: the formatter
  preserves them byte-for-byte with no annotation-aware code
  (round-trip tested).
- `Module.annotations` is the LSP/doc-gen read API (hover shows
  `@intent`, constraints, and the immutable flag; code actions
  respect it later).
