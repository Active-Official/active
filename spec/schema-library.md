# Standard Schema Library (C-062, P-061)

Versioned, reusable schemas and include fragments for the five
most common configuration shapes. Distributed as `schemas/` in
the repository (and in every release archive); consumed with a
literal include — no registry, no download step, paths below are
relative to the including file:

```active
include "schemas/http-server.active"
```

## 1. Layout and versioning

- One file per domain: `http-server`, `postgres`, `observability`,
  `feature-flags`, `ai-agent` (each `<name>.active`).
- The library versions WITH the language (same workspace version):
  schemas may use any shipped schema feature, and the library test
  suite (`tests/schema_library.rs`) pins every file against good
  and bad sample documents on both modes.
- Additive changes only within a minor line: new optional fields
  (with defaults) are fine; renaming, removing, or tightening a
  field requires a major version and a cookbook migration note.

## 2. Authoring rules (every file follows them)

- Closed tables at every level (`closed = true`): typos fail
  loudly with `did you mean` instead of drifting.
- Every field carries a descriptive plain comment, which the C-048
  docgen renders into reference pages. (`# @intent` stays on value
  bindings: annotations attach statement-level only, and schema
  fields never take them — spec/annotations.md.)
- Fields with a sane universal value declare a `default`, so a bare
  `include` plus an empty document still evaluates in resilient
  mode (defaults fill with `W005`).
- Fields with no sane default (passwords, hostnames, model names)
  stay required: including without setting them reports `T004`
  naming the key — the error IS the documentation.
- No secrets inside the library (no `secret(...)` literals, no
  example tokens): `Secret`-typed fields accept the operator's
  tainted values and redact like any other secret.
- Units where dimensions exist (`Duration` timeouts, `Bytes`
  sizes): never bare integers for measurable quantities.

## 3. Conformance of the library itself

`tests/schema_library.rs` drives the real path (`parse_file` →
strict/resilient `evaluate`) on committed fixtures under
`tests/schemas_lib/`: each schema gets a good document (strict
passes, values asserted) and a bad document (strict fails with the
pinned codes). A third test renders every schema through the C-048
docgen (`collect_items` + `doc_to_markdown`) and asserts each
field name appears — the reference pages in the docs site can
never drift from the enforced contract.
