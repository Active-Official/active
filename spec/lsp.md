# ACTIVE Language Server (C-043/C-044/C-045)

`active-lsp` speaks LSP 3.17 over stdio (JSON-RPC, `Content-Length`
framing) using `lsp-server` (sync main loop — no async runtime) and
`lsp-types`. One in-memory document store; analysis reuses
ACTIVE-core directly (parse recovering, evaluate, lint, format).

## 1. Transport and lifecycle

- `initialize` → advertises: hover, definition, references, rename
  (prepare not needed), completion (no trigger chars beyond
  identifiers), code actions (quickfix), inlay hints, semantic
  tokens (full only), full and range formatting, document symbols,
  workspace symbols, diagnostic publishing (push, not pull).
  `initialized` → ready. `shutdown`/`exit` handled; unknown
  methods get `MethodNotFound`, never a crash.
- Analysis is hermetic and budgeted: the server evaluates buffers
  with an empty environment snapshot and the strict budget preset,
  so ambient host values never leak into hover/completion and a
  pathological document fails closed as an X0xx diagnostic instead
  of hanging the editor.
- `textDocument/didOpen` (full text, `languageId: ACTIVE`),
  `didChange` (full sync), `didSave` (acknowledged; full sync means
  no state change), `didClose` (drop state, clear
  diagnostics). `file://` URIs map to paths; includes resolve from
  disk relative to the workspace file (unsaved include buffers are
  out of scope — documented).
- Every request is total: malformed positions return empty success
  (`null` / `[]`), never errors, never panics. Secret values never
  reach any response (C-036): hover values render redacted.
- Positions are LSP UTF-16: core 1-based character columns convert
  explicitly at every boundary (covered by Unicode tests).

## 2. Hover (C-043)

On a key, value, `let` name, or reference: a Markdown block with,
each when available: the schema type at the path (or the evaluated
value kind — `Int`, `String`, `Array`, `Secret`, … — when no schema
covers it), the evaluated value (redacted; unit-normalized —
`5s` shows `5000 ms`), origin (`file:line`, include-aware),
`@intent` text, `@constraint` sources, `@ai_immutable` /
deprecated flags, and active lint diagnostics on the key. Missing
data renders as explicitly-absent lines, never blanks. Hover on a
literal or string content resolves to the owning key; comments
resolve to nothing. Definition, references, and rename initiate
only from identifier tokens and interpolation references — never
from comments or string contents.

## 3. Definition, references, rename (C-043)

- **Definition**: a key/value/identifier resolves to its defining
  statement span — the value node for keys, the binding expression
  for lets — including across includes (target URI + span from
  `node_files`). A cursor on a literal value falls back to its
  owning key. Resolution follows evaluator precedence through
  scope extents derived from node spans (innermost containing table
  outward, then `let` bindings); extents carry (line, column)
  precision so same-line sibling blocks never capture each other's
  cursors, extend lexically to the next sibling-or-shallower scope
  so open tables hold their trailing lines, and scopes from other
  files never capture the cursor (included spans share the line
  space). Definition ranges anchor
  on the value span's start so same-line duplicates (inline tables)
  point at the right declaration. Profile bodies resolve as
  document scope (approximation). Unresolvable names return `null`.
- **References**: the definition span plus every use: sibling and
  `let` references (`Ref` spans, including inside interpolations,
  whose spans are source-rebased), patch *source*-path strings and
  `depends_on` elements (sub-spans located by quoted-path search
  within the directive/array extent). String-encoded paths match by
  node resolution, not by leaf name: each dot-prefix resolves from
  the document root and only prefixes resolving to the target count
  (a same-named key in another table never matches). `@rename`'s
  destination argument is a new name, never a reference.
  Unresolved idents are excluded (renaming one would change its
  meaning). Schema field names are excluded (no name spans are
  recorded — documented gap). Results sort by (file, line, column)
  for determinism.
- **Rename**: edits for the definition plus all references above
  (same-file edits grouped per URI; cross-file included).
  Collisions check the target's own parent scope (siblings, not
  just the document root). Renaming onto an existing sibling is
  refused, as is renaming an `@ai_immutable` path (hard boundary,
  consistent with the patch guard). `let` bindings live at document
  scope, so renaming one onto an existing root key is refused too
  (fields would shadow the binding). New names must be valid
  identifiers and non-reserved (`let`, `env`, `secret`, `true`,
  `false`, `null`, `include`, `as`, `private`, `profile` — else the
  edit would not parse as a name). Symbol lookup only initiates
  from identifier tokens and interpolation references, never from
  comments or string contents.

## 4. Completions, diagnostics, inlay hints (C-044)

- **Completions**: computed for the cursor's innermost table scope
  (file-local scopes from §3, lexically extended to the next
  sibling-or-shallower scope so fresh lines in open and empty
  tables still complete in their table; profile bodies complete at
  document scope). Sources, in order: (1) schema field names valid
  at the scope's dotted path — walked from the active schema tree
  (`SchemaTable` fields by segment; a `SchemaField` or anything
  else yields no schema names); each item carries its type name as
  detail; (2) all in-scope `let` bindings (lets are document-scoped;
  kind Variable); (3) the keywords `true`/`false`/`null` (kind
  Keyword). The prefix is the identifier characters immediately
  before the cursor on its line (empty prefix completes
  everything); matching is case-sensitive prefix match. Results
  sort alphabetically by label for determinism. No snippet or
  auto-import machinery, no trigger characters (clients request
  explicitly).
- **Diagnostics (push)**: on open and every change, one
  notification per file with findings — the open document (always
  published, even when clean, so stale markers clear) plus
  included files with diagnostics (included spans resolve against
  the included file's text; unreadable files publish zero ranges
  on their own URI, never mis-ranged against the parent). Groups
  that vanish (a removed include) are withdrawn with empty
  publishes; closing a document clears everything it ever
  published — unless another open document still publishes the URI
  (a shared include, or the included file itself when still open),
  which republishes from the remaining owner's current state
  instead. Versions are per-URI: the open document's version for
  its own URI, an open included buffer's version for its URI, and
  no version otherwise (a parent's version is meaningless for
  another file). Buffer matching is by file identity (exact path,
  then case-insensitive, then canonicalized): include stamps carry
  canonical paths that may differ textually from the client's URI,
  and a live buffer always beats stale disk. Sources:
  parse errors (E-codes), evaluation diagnostics including
  converted hard errors (cycles S001, budgets X0xx, validation
  T0xx — resilient mode otherwise keeps them as ordinary
  diagnostics alongside the tree), lint findings (L-codes, all
  enabled rules), and healing notices (H-codes, merged once via
  the evaluation records). One entry per diagnostic, sorted by
  (line, character, code) for determinism. Severity maps
  Error→Error, Warning→Warning, Info→Information, Hint→Hint.
  Ranges come from spans; span-less diagnostics publish as
  zero-length ranges at (0,0) rather than vanishing. The `source`
  field is always `"active"` and `code` carries the stable code.
  Unparseable files still publish what the recovering parser
  salvaged; closing a document publishes an empty list with no
  version (stale-versioned clears may be discarded).
- **Inlay hints**: after each data key's value and each `let`
  binding's expression, its type (`: Int`); durations and
  quantities append the normalized form (`: Duration = 5000 ms`,
  `: Int = 5242880 MiB` — byte quantities evaluate to integers,
  so the unit reads from the source Quantity node). Positions come
  from value/expression span ends (same-document nodes only).
  Types prefer the schema type at the path, else the evaluated
  value kind; `let` hints use the binding expression's static kind
  (literals, arrays, tables, strings) and are absent when nothing
  static is known. Secret values hint `: Secret` — never the
  value. Kind `Type`, padding on the left. Hints outside a
  requested range are withheld.
- **Completion**: schema field names for the innermost scope,
  in-scope `let` bindings, `true`/`false`/`null`, unit suffixes
  after a numeric literal (the edit inserts only the unit, never
  replacing the digits), and root-level metadata keys
  (`active_mode`, `active_capabilities`, `schema`). Items are
  prefix-filtered and sorted by label.

## 5. Code actions (C-045)

All `quickfix`-kind, scoped to the document, applied as workspace
edits. Diagnostics are recomputed from the document (never trusted
from the client's context, which may be stale); an action fires
when the request range touches a qualifying diagnostic's range
(cursor point or selection overlap). Actions sort by title for
determinism.

1. **Apply healing** — for H-diagnostics at the range whose repair
   confidence is ≥ 0.95 (auto-apply tier) *and* whose repair is
   marked safe: replace/insert per the repair shape (empty
   `original` inserts at the span start — except `InsertQuote`,
   whose span points at the string's opening delimiter while the
   closer belongs at the string token's end, located from the
   lexer stream; otherwise the span is replaced with the suggested
   text). Title names the code (`Apply healing (H001)`); the
   triggering diagnostic rides along.
2. **Extract let** — on a non-empty selection that exactly matches
   a single non-table value node's span (partial-token selections
   are withheld rather than emitted as invalid lets) and whose
   inner references resolve to `let` bindings or root keys only
   (moving a scope-relative reference to document scope would
   rebind it): insert `let <name> = <expression>` after the last
   top-level `let` (column-1 lets only, or at the top of file) and
   replace the selection with the name. `<name>` defaults to
   `extracted`, suffixed (`extracted2`, …) past existing bindings
   and root keys; the client renames after. All span/offset
   mappings are character-aware (never UTF-16 columns for core
   spans — non-BMP characters differ); line widths exclude CRLF
   carriage returns, like every other position.
3. **Convert file style** — to brace or to indent: the whole file
   through the exact formatter (guarantees hold). Each direction
   is offered only when it would change the text; the replacement
   range excludes a trailing carriage return. File-level rather
   than block-level by design: block splicing risks comment
   corruption, while file conversion is exact. (Logged deviation
   from "convert a block".)
4. **Add missing depends_on** — on an L002 missing-entry diagnostic
   at the range: insert the missing path into the scope's
   `depends_on` array (appending `"path", ` after the `[`, plain
   `"path"` into an empty array, or creating
   `depends_on = ["path"]` on its own line at the scope's depth
   when absent). The missing path rides the diagnostic's repair
   record when present (prose parsing is the fallback); the scope
   resolves from the diagnostic's path field. (Missing-entry
   diagnostics fire for declared arrays, empty included — so the
   creation branch is defensive, kept total against linter
   evolution.)
5. **Set `active_mode`** — at the document start when no marker
   exists: insert `active_mode = "data"` (spec/mode.md). Offered
   only for a request at (0,0), so it cannot fire mid-document.
6. **Mark `@ai_immutable`** — on a non-comment, non-header line
   containing `=`: insert `# @ai_immutable: true` above the
   statement. The value itself is never rewritten.
7. **Apply safe repair plan** — run the shipped multi-step repair
   engine (hermetic engine, strict budget, validate goal, eight-step
   cap) with the buffer's own file context, so an included schema
   plans the same repair `active repair` does. When the plan contains
   at least one safe step, offer the repaired source as one document
   edit. The action and `active repair` share the engine, so their
   plans cannot drift, and included content is never copied into the
   buffer.

## 5.1 Formatting

- **Full** (`textDocument/formatting`): the canonical formatter
  with `Style::Preserve`, returned as one full-document edit; a
  canonical document returns no edits, so format-on-save is
  idempotent. Formatting parses the buffer's own text with includes
  recorded but unresolved: the merged analysis tree carries included
  keys, and re-emitting it would inline private include content.
  `include` statements are preserved in place.
- **Range** (`textDocument/rangeFormatting`): compute the single
  canonical diff hunk and return it only when it lies inside the
  requested line range. A hunk that spills past the selection
  yields no edit rather than corrupting neighboring lines.

## 6. Semantic tokens (C-043)

The legend is contractual: `keyword`, `string`, `number`, `comment`,
`property`, `variable`, `operator`, plus four custom types:
`secret` (the `secret` construct; values never travel in tokens),
`mode` (`active_mode` / `active_capabilities` keys), `contentAddressed`
(the `blob` construct), and `schema` (the `schema` keyword). Tokens are
full-document, split per line, UTF-16 lengths, delta-encoded.

Full-document tokens from the lexer stream (no new parsing):
keywords (`let`, `true`, `false`, `null`, `profile`, `include`,
`as`, `private`, `secret`, `env`), strings, numbers (with units),
comments (incl. `# @…` annotations as comments), property (keys),
variable (`let` names and references), operators, brackets. Delta
encoding per LSP; legend fixed in the server.

## 7. Testing contract

- Handler unit tests (no I/O): hover/definition/references/rename/
  completion/hints/actions/semantic-tokens over parsed fixtures.
- One stdio end-to-end test driving the real binary through
  initialize → didOpen → hover/definition/diagnostics → shutdown,
  proving framing, capabilities, and redaction over the wire.
