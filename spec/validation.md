# SOLID Schema Validation (C-031)

Validation walks the fully evaluated value tree against the active schema
tree and reports every violation as a structured diagnostic. It runs
**after evaluation, after profile merge, and after patch application**
(spec §2.9 ordering), **before export and conversion**. Validation
never drops or rewrites user data: the single exception is resilient
default filling (§9), which only adds schema-declared values for
missing keys.

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
  A value key with no schema field is **ignored** in open tables
  (open-world); closed tables reject it (§10).
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

- the field declares a `default` — strict mode still reports T004
  (explicitness), while resilient mode fills the default and records
  W005 instead (§9). A default that cannot be applied (defensive;
  unreachable on well-formed evaluations) reports T004 noting the
  failure rather than going silent; or
- the field's type is `Null` **and it declares no default** — absence
  is equivalent to null. `Null` with a `default` follows the default
  rule above in both modes.

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
  tree, with schema defaults filled (§9) and schema-driven repairs
  applied (§10.1–§10.2). Export proceeds; the CLI
  exit code follows the standard policy (any Error diagnostic exits
  1, but the output is still produced).

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

## 9. Schema-driven defaults (C-032)

A schema field with a missing value and a declared `default` behaves
by mode:

- **Strict**: T004 (`missing required key`, with a message noting that
  strict mode requires explicit values despite the default). Strict
  means explicit: every value in the tree is written by someone.
- **Resilient**: the evaluated default is inserted into the result
  tree at the missing key and a W005 warning is recorded (`filled
  missing key '<path>' from schema default`, at the schema field
  span). The filled value is then validated exactly like a written
  value, so a mistyped default surfaces as T002/T003 rather than
  corrupting the tree silently.

Only leaf fields fill: a missing table (even one whose schema has
entries) is T004 in both modes, and array positions are never
synthesized. Defaults are ordinary evaluated expressions, so they can
use literals, env refs, and arithmetic — and the usual secret rules
apply (a filled secret redacts on export; W005 never names values).

## 10. Closed tables (C-032)

A schema table with `closed = true` rejects value keys with no
corresponding schema field: T005 (`unexpected key '<key>' in closed
table`) at the value expression's span (the parser records spans on
value nodes, so this is the token that produced the value — same line
as the key in practice), falling back to the schema table span when
unresolvable. Open tables (the default) keep ignoring extra keys.
Nesting composes: each table enforces its own flag.

### 10.1 Key repair (C-016 phase 2, wired here)

An unexpected key is scored against the schema field names with the
phase-2 similarity engine (`best_key_match`). Suggestions clear the
medium band (≥ 0.70) in both modes — informational only, never
applied there. Renames additionally require the high band
(≥ 0.85, the C-016 item threshold). On a match:

- **Strict**: the T005 message gains `; did you mean '<field>'?`
  and a `KeyTypo` repair attachment (confidence = the score;
  `safe` follows the canonical near-identical-edit rule) for
  tooling (the LSP `apply healing` action). The tree is untouched:
  key identity is semantic, and strict mode requires explicitness —
  the missing field still reports T004 alongside.
- **Resilient**: when the value already type-checks against the
  suggested field and the target key is absent (never clobber a
  written key), the key is renamed in the result tree and an H005
  warning is recorded (`renamed key '<a>' to '<b>' (confidence
  <n>)`, with the applied repair attached; renames always warn,
  even at ≥ 0.95, because a key identity changed). The stale
  missing-key T004 for the healed path is retracted, queued
  default fills for it are cancelled, and the renamed subtree
  is revalidated like a written value — constraints still bite,
  and nested typos queue further renames to a fixpoint. Otherwise
  (low score, mistyped value, or taken target) the T005 is
  collected with its suggestion and the tree is kept as-is.

### 10.2 String coercion (C-016 phase 3, wired here)

A T002 mismatch where the value is a string that parses as the
target type (`Int`, `Float`, `Bool`, `Duration` via the phase-3
functions) is coercible:

- **Strict**: the T002 message notes coercibility (`expected
  Duration, got string (coercible under --resilient)`). A
  recognized sentinel word (`"unlimited"`, `"none"`, `"inf"`, …)
  is named by intent instead (`sentinel 'unlimited' signals limit
  but the schema designates no value — set an explicit value or a
  schema default`): sentinels are never mapped, because the schema
  language has no syntax for designating their values and guessing
  would invent data. The echoed word is safe — it is one of a
  fixed dictionary, never user data.
- **Resilient**: the parsed value replaces the string, an H007
  warning is recorded (value-free, per §7), and the coerced value
  is constraint-checked like a written one (an out-of-range
  coercion still reports T003).
