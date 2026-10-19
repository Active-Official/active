# Converter Suite (C-046/C-047, P-043)

Bidirectional conversion between ACTIVE value trees and the JSON /
YAML / TOML ecosystem, plus typed-code generation from schemas.
Secret redaction is uniform on every export path: the literal
`***SECRET***` replaces any secret value, and redacted exports are
one-way by design (no import reconstructs a secret).

## 1. Canonical value mapping (ACTIVE → JSON)

`value_to_json` is the canonical mapping; YAML and TOML derive
from the value tree with the same shapes:

- Null/Bool/Int/Float/String → the JSON same-kind value.
- DurationMs(ms) → `{"ms": ms}` (pinned by
  `tests/spec/02_unit_normalization.expected.json`).
- Quantity(n, unit) → `{suffix: n}` with the normalized suffix
  (e.g. `{"MiB": 5}`).
- Array/Table → JSON array/object, recursively.
- Secret(_) → `"***SECRET***"` (never the value, C-036).

## 2. YAML export

`value_to_yaml_string`: the canonical JSON value rendered as YAML
1.2 (block style) via the JSON model — one mapping, two syntaxes,
so JSON and YAML exports always agree. Secrets redact identically.

## 3. TOML export

`value_to_toml_string` builds a TOML document from the tree:

- Tables → `[sections]` (nested), arrays of tables → `[[lists]]`,
  dotted keys never emitted (single-segment headers only, per the
  grammar).
- Scalars → TOML same-kind literals with exact string escaping.
- DurationMs(ms) → the string `"5000ms"` (normalized milliseconds;
  TOML has no duration type, and the original unit is already lost
  at evaluation — explicit magnitude beats silent Int).
- Quantity(n, unit) → the string `"5MiB"` (same reasoning; in
  practice byte quantities already normalize to Int at evaluation,
  so this arm is defensive).
- Secret → `"***SECRET***"`.
- Null has no TOML form: null-valued keys are OMITTED with one
  W006 warning each (`null has no TOML representation; omitted
  'path'`); null array elements omit the same way. Data loss is
  announced, never silent.
- NaN/inf floats emit TOML `nan`/`inf`/`-inf`.
- Mixed-type arrays have no TOML form at all: the export fails
  hard naming the array path (durations/quantities/secrets count
  as strings for this check; nulls don't count). A silent empty
  document would be worse than an error.

Failures (unrepresentable shapes beyond Null) are hard errors, not
silent drops.

## 4. Reverse direction (JSON / YAML / TOML → ACTIVE)

`json_to_active`, `yaml_to_active`, `toml_to_active` return
`(String, Vec<Diagnostic>)`: best-effort ACTIVE source plus
diagnostics for lossy corners. Emission style is brace blocks
(`server { … }`) — never dotted headers (canonical emission is
nested blocks; the parser accepts dotted headers and they lower
to the same IR).

- Objects → nested brace blocks; arrays → `[...]`; strings with
  exact escaping; numbers → Int (i64) else Float (u64 past i64::MAX
  notes W010); non-finite floats (YAML `.nan`/`.inf`, TOML
  `nan`/`inf`) become null with a W010 note — silent flips would
  corrupt data.
- Top-level scalars/arrays wrap as `value = …` with a W009
  diagnostic (a document of only `value` is valid ACTIVE).
- YAML non-string keys (ints, bools) stringify with one W007
  diagnostic each; YAML timestamps arrive as strings (no
  diagnostic — they already are strings in the YAML model).
- TOML datetimes stringify with one W008 diagnostic each
  (TOML-only type, no ACTIVE equivalent).
- Keys sanitize to ACTIVE identifiers (W007 per change):
  non-identifier characters become `_`, leading digits and empty
  names are repaired, and hard keyword spellings
  (`let`/`env`/`secret`/`true`/`false`/`null` — never parseable as
  keys) gain a suffix. Contextual renames apply exactly where the
  shape would change meaning: a `schema` object (schema extraction
  would fail it as E020) and a `depends_on` array (dependency
  extraction would consume it as metadata) each gain a suffix;
  every other shape under those names parses as ordinary data
  (probed — `include`/`as`/`private`/`profile` need no renaming).
  Sanitized duplicates are last-wins with a W007 note, like JSON
  itself.
- `"***SECRET***"` imports as a plain string: redacted exports
  are one-way (a re-imported marker must never become a live
  secret — a security invariant, tested).
- JSON duplicate keys are last-wins (the JSON model cannot see
  them); documented, untestable at this layer.
- Float round-trips may lose precision past f64 (documented).
- Source-syntax failures (malformed JSON/YAML/TOML) are hard
  errors with the reason and location but WITHOUT source excerpts:
  a malformed line may contain a secret, which must never reach
  diagnostics or logs (security invariant, tested).
- W006–W010 are registered in `docs/diagnostics.md`.
- JSON Schema import (`active import -f jsonschema`, L-021) maps types,
  ranges, lengths, patterns, enums, defaults, required keys, and closed
  tables; combinators, references, conditionals, per-item counts, and
  non-string const/enum skip loudly with W012 (spec §4); optional
  properties without defaults tighten to required with W012.

Round-trip contract (tested): JSON → ACTIVE → evaluate → JSON is
fixed-point for the canonical mapping, except datetimes, secret
markers, and non-string YAML keys (all diagnosed).

## 5. CLI

- `ACTIVE export --format json|yaml|toml` (evaluated tree; secrets
  redact; TOML Null warnings surface; hard errors exit non-zero).
- `ACTIVE import --from json|yaml|toml <file>` prints ACTIVE source
  to stdout; import diagnostics go to stderr; lossy corners exit 0
  (diagnosed, best-effort), hard parse failures exit non-zero.
- `ACTIVE generate --from <file.active> --to
  jsonschema|openapi|typescript|go|rust [--name <Root>]` prints the
  binding to stdout; documents without a schema exit non-zero.

## 6. Testing contract

- Export fixtures: every value kind (incl. nested tables, arrays
  of tables, durations, quantities, secrets, nulls, NaN) in all
  three formats; redaction audit (secret text in no output).
- TOML validity: re-parse with the `toml` crate (not just string
  match); YAML validity: re-parse.
- Reverse fixtures per format incl. each lossy corner with its
  diagnostic code; round-trip fixed-point test.
- CLI tests for export/import/generate incl. exit codes.
- Generated code compiles (the Rust output passes a rustfmt
  syntax gate; TS/Go by golden snapshots).

## 7. Schema generators (C-047)

Typed bindings from the module's schema tree (`module.schema`;
error when absent). Constraint arguments are expression nodes, so
only static literals render (`minimum`, `minLength`, `pattern`,
`default`, enum members); dynamic bounds are skipped without
diagnostics (validation still enforces them at runtime).

Type mapping (SS = ACTIVE schema type):

| SS | JSON Schema | TypeScript | Go | Rust serde |
|---|---|---|---|---|
| Int | `integer` (+minimum/maximum) | `number` | `int64` | `i64` |
| Float | `number` (+minimum/maximum) | `number` | `float64` | `f64` |
| String | `string` (+minLength/maxLength/pattern) | `string` | `string` | `String` |
| Bool | `boolean` | `boolean` | `bool` | `bool` |
| Null | `null` | `null` | `interface{}` | `()` |
| Duration | `object{ms: integer}` (matches §1 export) | `number` (ms) | `int64` (ms) | `u64` (ms) |
| Bytes | `integer` (normalizes to Int) | `number` | `int64` | `u64` |
| Table | `object` + properties | `interface` | `struct` | `struct` |
| Enum(m) | `enum: [...]` | `"a" \| "b"` | `string` | real `enum` |
| Secret | `string` (+redaction note) | `string` | `string` | `String` |
| Any | `true` | `unknown` | `interface{}` | `serde_json::Value` |
| Array\<T\> | `array` + items | `T[]` | `[]T` | `Vec<T>` |

- `required`: fields without a static-literal default, except
  Null-typed fields (absent Null keys validate clean — C-032).
  Dynamic defaults count as required (not statically known).
- Closed tables → `additionalProperties: false`; open tables omit
  it. Static literal defaults render as JSON Schema `default`.
- Deprecated fields: JSON Schema `"deprecated": true`, TS
  `/** @deprecated msg */`, Go `// Deprecated: msg`,
  Rust `#[deprecated]`.
- Names: nested types are PascalCase path joins (`server.database`
  → `ServerDatabase`); fields keep ACTIVE names with `json`
  (Go) / `serde` (Rust) tags preserving them; Go fields export by
  capitalizing; collisions after normalization gain `_`.
- OpenAPI output is the JSON Schema inside a 3.1
  `components.schemas.<Name>` fragment (`--name`, default
  `Config`).
