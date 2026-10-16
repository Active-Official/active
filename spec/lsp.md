# SOLID Language Server (C-043/C-044/C-045)

`solid-ls` speaks LSP 3.17 over stdio (JSON-RPC, `Content-Length`
framing) using `lsp-server` (sync main loop — no async runtime) and
`lsp-types`. One in-memory document store; analysis reuses
solid-core directly (parse recovering, evaluate, lint, format).

## 1. Transport and lifecycle

- `initialize` → advertises: hover, definition, references, rename
  (prepare not needed), completion (no trigger chars beyond
  identifiers), code actions (quickfix), inlay hints, semantic
  tokens (full only), diagnostic publishing (push, not pull).
  `initialized` → ready. `shutdown`/`exit` handled; unknown
  methods get `MethodNotFound`, never a crash.
- `textDocument/didOpen` (full text, `languageId: solid`),
  `didChange` (full sync), `didClose` (drop state, clear
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
  cursors, and scopes from other files never capture the cursor
  (included spans share the line space). Definition ranges anchor
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

- **Completions**: schema field names valid at the cursor's table
  (from the active schema tree), in-scope `let` bindings, and the
  keywords `true`/`false`/`null`. Prefix-filtered, sorted,
  deterministic. No snippet or auto-import machinery.
- **Diagnostics (push)**: on open and every change — parse errors
  (E-codes), lint findings (L-codes, all enabled rules), healing
  notices (H-codes as hints), and full evaluation + validation +
  constraint diagnostics (resilient mode; S/T/W codes). Ranges from
  spans; severity maps Error→Error, Warning→Warning, Info/Hint→Hint.
  Unparseable files still publish what the recovering parser
  salvaged.
- **Inlay hints**: after each key/let value, its type (`: Int`)
  and — for duration/byte quantities — the normalized form
  (`5s` ⇒ `5000 ms`). Kind `Type`, padding on the left. Values
  redact like hover.

## 5. Code actions (C-045)

All `quickfix`-kind, scoped to the document, applied as workspace
edits:

1. **Apply healing** — for H-diagnostics at/around the cursor whose
   repair confidence is ≥ 0.95 (auto-apply tier): replace the repair
   span with the suggested text.
2. **Extract let** — on a selected value expression: insert
   `let <name> = <expression>` after the last top-level `let`
   (or at the top of file) and replace the selection with the name.
   `<name>` defaults to `extracted`; the client renames after.
3. **Convert file style** — to brace or to indent: the whole file
   through the exact formatter (guarantees hold). File-level rather
   than block-level by design: block splicing risks comment
   corruption, while file conversion is exact. (Logged deviation
   from "convert a block".)
4. **Add missing depends_on** — on an L002 missing-entry diagnostic:
   insert the path into the scope's `depends_on` array (creating
   the array when absent).

## 6. Semantic tokens (C-043)

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
