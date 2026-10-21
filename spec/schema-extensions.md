# Schema Language Extensions — RFC (CHECKLIST 1.010, PROPOSAL §3.1)

Status designations: **Shipped** (in 1.4.0, do not redesign), **New**
(specified here, implemented under 1.011), **Deferred** (specified intent,
explicitly not built now, with reason).

## Shipped (frozen semantics, referenced not changed)

Ranges (`min`/`max` on numerics, durations, bytes), string lengths,
`pattern` (unanchored regex search, invalid pattern is a once-per-field
T003 schema bug), enums as positional string members, defaults as full
expressions (strict T004 vs resilient W005 fill), `closed = true` tables
(T005), key repair and string coercion wiring, secret field types with
taint interaction, span policy (value span preferred, schema span
fallback), value-free messages. All in `spec/validation.md`.

## New in this RFC

### 1. Format validators (New)

`String(format = "<name>")` where name is one of `email`, `uri`, `uuid`,
`semver`. Checked after `pattern`. Violation is T003
`does not match format '<name>'` (value-free). An unknown format name is a
schema bug: T003 `unknown format '<name>' in schema`, reported once per
field like an invalid pattern, constraint then skipped. Formats are
anchored full matches (unlike `pattern`). Rationale: service contracts
constrain identifiers far more often than free text; four formats cover
the observed majority; the registry stays closed so validators remain
total and deterministic.

### 2. Documentation metadata (New)

`description = "<text>"` accepted on any field type and on schema tables.
Stored on the field, ignored by validation, surfaced by `docgen` and the
future introspection API. `deprecated = "<reason>"` already exists on
fields; this RFC standardizes its message (`deprecated: <reason>`) and
requires docgen to render it. Non-string `description` is E020 at parse.
Rationale: contracts need prose where the contract lives; metadata must
never affect verdicts.

### 3. Enum member descriptions (New)

`Enum("a", "b", descriptions = ["first", "second"])`: parallel list,
length must equal member count else schema bug T003
`descriptions length must match Enum members`, reported once per field.
Descriptions are metadata only; membership semantics unchanged. Rationale:
backward compatible (existing enums parse untouched); numeric backing
values are Deferred (below) so no evaluation semantics change now.

### 4. Conditional presence (New)

`requires = "<dotted-path>"`: the field must be present when the named
sibling path is present *and enabled* (true, non-empty, non-zero;
a provided secret counts as enabled; `false`, `""`, `0`, empties, and
null do not trigger). Violation is T004 naming both paths. Rationale:
the smallest conditional that covers real contracts (TLS cert required
when TLS is on, replicas required when prod mode is on) without an
expression language in the schema. Full `when`/`then` expressions are
Deferred.

## Deferred with reason

- **Numeric backing values for enums**: changes evaluation semantics;
  needs its own RFC with codegen mapping per target. Intent kept.
- **Discriminated unions / oneOf with tag**: needs composition semantics
  first; scheduled after composition lands.
- **Full `when`/`then` expressions**: an expression language inside
  schemas risks budget and determinism review; `requires` covers the
  observed cases. Revisit with evidence.
- **Composition (`include`, `extend`, `override`, selective import)**:
  interacts with provenance, lockfiles, and multi-file spans; needs a
  dedicated design pass. Intent kept.
- **Closed-by-default**: would break every existing open schema and the
  frozen 1.0 surface. Deliberately not adopted; `closed = true` stays
  opt-in per table.
- **Additional codegen targets (Protobuf, Cap'n Proto, Avro)**: Phase 2/4
  per plan; JSON Schema, TypeScript, Go, Python first.

## Cross-cutting guarantees (all New features)

Deterministic order (schema field order drives everything), budgets apply
(defaults evaluate under the call budget; no new unbounded work),
diagnostic codes stable and never reused (T003 for constraint violations
and schema bugs, T004 for conditional presence), value-free messages
(values, bounds, patterns never echoed), secret interaction unchanged
(a `Secret` field accepts only secrets regardless of other args).
