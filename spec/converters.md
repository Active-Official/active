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
- DurationMs(ms) → `{"$active.duration": {"ms": ms}}` (pinned by
  `tests/spec/08_duration_arithmetic.expected.json`). The bare `{"ms": ms}`
  form was byte-identical to a plain table, so the export was not injective
  (LANG-F-002); import recognises the wrapper and re-emits `5000ms`, and a
  bare `{"ms": N}`-shaped object imported as a table raises W016 naming the
  ambiguity (LANG-F-001).
- Quantity(n, unit) → `{"$active.quantity": {"unit": <suffix>, "value": n}}`.
  The wrapper is reserved and identifier-safe: the old bare `{suffix: n}` form
  was table-shaped, and suffixes such as `%`, `/s`, and `req/s` are not
  identifiers, so import mangled the unit and lost the type. Import recognises
  the wrapper and re-emits the typed literal, so the mapping is a fixed point
  for every quantity kind (LANG-B-019). `%` is stored normalized (0.25) and
  re-emitted as `25%`.
- Array/Table → JSON array/object, recursively.
- Secret(_) → `"***SECRET***"` (never the value, C-036).
- Blob → `{"$active.blob": {hash, size, media_type, tainted}}`, the
  same nested wrapper in YAML and TOML (`[f."$active.blob"]`). A
  tainted identity redacts `media_type`; `payload_b64` appears only
  through the explicit materialisation renderers
  (`value_to_json_materialised` and the YAML/TOML `*_materialised`
  forms), never through the generic serializers. When a payload is
  retained, every renderer publishes the identity VERIFIED from the
  media type and bytes (`blob::verified_identity`), not a
  caller-supplied hash beside bytes it does not describe.

The JSON export is **layer-flattened by design**: profile overlays and
patches are merged into one tree (Profile priority wins; no W003 on a
type change under L-P09),
and no export carries a record of which layer wrote a value - a receiver must
not have to re-implement merges. Provenance is available on the inspection
surface instead: `active inspect <file> <path>` reports `layer`
(`profile:<name>` or `patch:<op>`) for a path a merge or patch wrote, and
omits it for base-document values (LANG-F-014). Merge semantics never change;
only the bookkeeping is new.

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
- DurationMs(ms) → the reserved wrapper table
  `{ "$active.duration" = { ms = N } }` (TOML has no duration type). The
  same shape JSON uses, so TOML import recognises it and restores the typed
  value instead of a string (LANG-B-018).
- Quantity(n, unit) → the reserved wrapper table
  `{ "$active.quantity" = { unit = <suffix>, value = N } }` (LANG-B-019).
- Secret → `"***SECRET***"`.
- Null has no TOML form: null-valued keys are OMITTED with one
  W006 warning each (`null has no TOML representation; omitted
  'path'`); null array elements omit the same way. Data loss is
  announced, never silent.
- NaN/inf floats emit TOML `nan`/`inf`/`-inf`.
- Mixed-type arrays have no TOML form at all: the export fails
  hard naming the array path (durations, quantities, and blobs count
  as tables for this check; secrets and strings count as strings;
  nulls don't count). A silent empty document would be worse than an
  error.

Failures (unrepresentable shapes beyond Null) are hard errors, not
silent drops.

**TOML export is type-preserving (LANG-B-018).** The wrappers above are
recognised by `toml_text_to_active` (`json_value_to_active`'s wrapper
rules), so export → import → evaluate is a fixed point: `t = 5s` exports
as `{ "$active.duration" = { ms = 5000 } }` and imports back as a
duration, not a string. The *original spelling* is not preserved (a
duration normalizes to milliseconds, a quantity to its canonical suffix),
exactly as JSON and YAML behave; only Active source keeps the authored
unit. The earlier one-way magnitude-string form was removed: an importer
cannot tell a typed string from data, and a reserved wrapper needs no
guess.

## 4. Reverse direction (JSON / YAML / TOML → ACTIVE)


**Non-finite floats never cross a converter.** `inf`, `-inf`, and `NaN` are rejected at parse (E023 for an overflowing literal), at arithmetic (T001), and at resilient coercion (the T002 stands). No exporter maps a number to `null`; the JSON model's inf→null behaviour is never used (LANG-B-009). Should a host ever hold one (the exporter arm is defense in depth), the JSON export tags it as `{"$active.float": "inf"|"-inf"|"nan"}` - an object, so it cannot collide with `null` or with the string `"inf"` - and `content_hash` hashes that tagged form (LANG-F-003). Import recognises the tag and records W010 (no Active literal exists).

Typed values (durations, quantities, blobs) are exported with `$active.*` wrappers and recognised on import; a bare `{"ms": N}`-shaped object imported as a table emits a loud diagnostic (W016) naming the ambiguity (LANG-F-001/F-002).

**Duplicate keys on import.** YAML rejects duplicate mapping keys (the YAML specification makes them an error); the failure names the rule. JSON keeps the last value, because the JSON document model has no duplicate concept to report (LANG-B-012). Neither format silently drops data in the other's way.

**Extreme floats import as long decimals (LANG-B-028).** Active has no exponent notation, so the shortest *parseable* spelling of a subnormal (5e−324) or a maximal value is its full decimal expansion (≈320 digits). The importer emits exactly that shortest exponent-free form, and the value round-trips bit-for-bit; ordinary magnitudes stay short (`1.5`). A compact `5e-324` spelling would require exponent literals in the grammar, which is a language feature, not an import choice.

**Keys are ASCII identifiers.** Active identifiers are ASCII-only, so the
reverse converters sanitize every foreign key: characters outside
`[A-Za-z0-9_]` become `_`, a leading digit gains `_`, and reserved words gain a
trailing `_`. Sanitization is announced (W007). Two distinct source keys can
collapse to the same shape (`café` and `caf_` both become `caf_`); the later key
is suffixed (`caf__2`, `caf__3`, ...) so **no value is lost**, and the collision
is announced (W007) on both value and schema imports (LANG-B-008).
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

A **static default** is a literal or an all-literal interpolation
(`default = "literal-\${x}"`, where the escaped dollar makes the text
literal). Both count as a default on every surface: JSON Schema/
OpenAPI `default`, the `required` flag in `schema inspect` and
`inspect --path`, docgen's `Default` line, and the fill planner. A
dynamic default (an env read, a reference, arithmetic) is not static
and counts as required.

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
| OneOf(A, B) | `oneOf: [...]` (no `type`; each branch is a full schema) | `A \| B` | refused | refused |

- **Disjunctions (L-P10).** JSON Schema and OpenAPI render each branch
  with its full schema (bounds, patterns, nested tables), plus the
  entry's static default. TypeScript renders a union; Python renders
  `Union[...]` (with `Union` added to the typing import); Zod renders
  `z.union([...])`. Go and Rust **refuse** (`Err`, not a lossy
  `interface{}`/`serde_json::Value`): neither renderer emits an
  untagged union, and a generated contract that silently widened the
  field would be a lie. Regenerate JSON Schema or TypeScript for a
  schema that contains a disjunction.

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
- Cross-field `check` rules (L-P19) have no representation in JSON
  Schema, OpenAPI, TypeScript, Go, or Rust serde: the generators leave
  them in the source and do not invent a runtime validator. `active
  inspect` lists the rules so a consumer can see what the generated
  schema omits; the check is enforced by `active check`/`eval`.
