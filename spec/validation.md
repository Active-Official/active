# SOLID Schema Validation (C-031)

Validation walks the fully evaluated value tree against the active schema
tree and reports every violation as a structured diagnostic. It runs
**after evaluation, after profile merge, and after patch application**
(spec §2.9 ordering), **before export and conversion**. Validation never
mutates the tree: default filling (C-032) is a separate later step.

## 1. When validation runs

- Only when the document declares a schema (`Module::schema` is `Some`).
  Documents without schemas validate trivially (no diagnostics).
- Exactly once per `evaluate()` call, on the final merged result.
- `let` bindings are not validated: schemas describe the exported value
  tree, and bindings never export.

## 2. The walk

Validation pairs each schema node with its value by key, recursively:

- The root schema pairs with the root table; a nested schema field named
  `k` pairs with the value table's key `k` (including inline-table
  values: `{…}` literals are ordinary tables).
- Pairs are visited in **schema field order** (source order), so
  diagnostics are deterministic for a fixed input.
- Arrays pair positionally: element `i` validates against the element
  schema with path `<path>[<i>]`. Length is unconstrained (the grammar
  gives `Array` no length arguments).
- A schema field whose value key is absent is a **missing key** (§5).
  A value key with no schema field is **ignored** (open-world);
  closed-table rejection is C-032.
- Constraint argument values (`min`, `max`, …) are read from the
  evaluated node map: they are ordinary expression nodes evaluated with
  the document (C-030). An argument that somehow has no evaluated value
  is skipped defensively (never a crash, never a false violation).

## 3. Type acceptance (T002 on mismatch)

| Schema type | Accepted values |
|---|---|
| `Null` | `null` only (absent keys are also accepted, §5) |
| `Bool` | booleans |
| `Int` | integers. Bytes literals (`512MiB`) evaluate to integers and check here, including `min`/`max` bounds |
| `Float` | floats **and integers** (widening: an integer is always exactly the value it names; the reverse is lossy and rejected) |
| `String` | strings |
| `Duration` | durations (normalized to ms at evaluation) |
| `Bytes` | integers (byte literals evaluate to integers, and plain
  integers are indistinguishable from them — any integer is accepted;
  byte-ness is erased at evaluation, a known limitation) |
| `Secret` | secrets |
| `Any` | anything, always valid |
| `Table` (open) | tables; nested entries recurse when the schema table has them |
| `Enum(…)` | strings are checked for membership in §4 (T003), non-strings fail here |
| `Array<T>` | arrays; every element recurses into `T` |

Anything else is T002: `schema violation at '<path>': expected <type>,
got <kind>`, where `<kind>` is the coarse kind name (`null`, `boolean`,
`number`, `string`, `duration`, `array`, `table`, `secret`,
`quantity:<unit>`). Kinds never contain values, so secrets cannot leak
through them.

## 4. Constraints (T003 on violation)

Checked only after the type accepts the value:

- **Numeric** (`Int`, `Float`): `min`/`max` compare in a common domain —
  integers as `i64`, floats as `f64`, durations in ms, byte counts as
  integers. An `Int` bound on a `Float` field (and vice versa) compares
  through `f64`. A bound of an incomparable kind (e.g. a string `min`
  on an `Int` field) is a **schema bug**, reported once as T003
  `incomparable min bound for Int fields` — it points at the value span
  like any other violation, never at secret data.
- **String**: `min_len`/`max_len` count Unicode scalar values
  (`str::chars().count()`); `pattern` must match (see below).
- **Duration/Bytes**: `min`/`max` accept duration values, byte-count
  integers, or plain integers (read as ms / bytes respectively). Any
  other bound kind — notably `Float` bounds on `Bytes` fields — is an
  incomparable-bound schema bug (below).
- **Enum**: membership in the declared member list, else T003 quoting
  the members: `not a member of Enum("a", "b")`. Members are grammar
  string literals, safe to quote.
- **Schema bugs are reported once per schema field, not per value**: an
  uncompilable pattern or an incomparable bound emits a single T003 the
  first time the walk reaches it (deterministic: schema order decides);
  the broken constraint is then skipped for the remaining values while
  genuine per-value violations keep reporting per value.
- **Pattern**: unanchored regular-expression search via the `regex`
  crate (JSON-Schema-style: authors anchor with `^…$` for a full
  match). A pattern that fails to compile is a schema bug: T003
  `invalid pattern in schema`.

Detail strings are structural (`below minimum`, `above maximum`,
`too short`, `too long`, `does not match pattern`, …): evaluated
values and bounds are **never** echoed into messages (secret
invariant, §7).

## 5. Missing keys (T004)

A schema field with no corresponding value key is T004 (`missing
required key`) unless:

- the field declares a `default` — filling it is C-032's job; C-031
  stays silent so the two phases never contradict each other; or
- the field's type is `Null` — absence is equivalent to null.

The span is the schema field's own span (there is no value to point
at).

## 6. Modes

- **Strict** (`Engine.resilient == false`, the CLI default): all
  violations are collected in deterministic order, then evaluation
  fails with `EvalError::ValidationFailed` carrying every diagnostic.
  No value tree is returned: a violation is fatal. The CLI prints each
  diagnostic and exits 1.
- **Resilient** (`Engine.resilient == true`, CLI `--resilient`): the
  same diagnostics are collected and returned alongside the value
  tree, unchanged and unfilled. Export proceeds; the CLI exit code
  follows the standard policy (any Error diagnostic exits 1, but the
  output is still produced).

`Engine::new()` defaults to resilient, preserving the alpha behavior
that evaluation succeeds; the CLI passes its `--resilient` flag into
the engine so the default invocation is strict.

## 7. Security and determinism

- Messages quote only schema-authored literals (paths, type names,
  enum members) and kinds. Actual values, bounds, and patterns never
  appear — a `secret()` value, an environment value, or any computed
  bound cannot reach a diagnostic, a log, or an export through
  validation.
- Same input → same diagnostics in the same order: schema field order
  drives the walk, message templates are fixed, and spans resolve
  deterministically (§8).

## 8. Span policy

Diagnostics prefer the **value span** (where the user fixes the
problem), resolved by walking the arena value tree alongside the
result tree: when a result key matches an arena child key, its
recorded span is used. Patched or profile-merged values attribute to
their key's home span in the base tree — deterministic and usually
where the key is declared; ideal patch-line provenance is future work
(see Memories-and-Observations 2026-10-16). Array elements resolve
positionally against the arena array when the lengths line up, so
nested arrays attribute to their own level. When no value span
resolves, the **schema field span** is used; missing-key diagnostics
always use the schema span. Every span carries its file in multi-file
documents.
