# Machine-readable output schemas

Committed JSON Schemas (draft-07) for every stable emitter. CI validates
real CLI output against them (`conformance/evidence/output_schema_check.py`),
so a shape change fails the build.

| Schema | Command |
|---|---|
| `graph.json` | `active graph <file> --format json` |
| `taint.json` | `active viz taint --from <file>` |
| `budget.json` | `active viz budget --from <file>` / `active budget explain --format json` |
| `inspect-value.json` | `active inspect --from <file> --path <p>` / `active schema inspect --path <p>` |
| `mode.json` | `active mode --from <file> --format json` |
| `lint.json` | `active lint --format json` |
| `sarif.json` | `active lint --format sarif` (SARIF 2.1.0) |
| `repair-plan.json` | `active repair --from <file> --format json` |
| `debug-steps.json` | `active debug --from <file>` |

Rules:

- Additive fields are allowed only when the schema is updated in the
  same change; removing or retyping a field is breaking.
- Every document carries a `version` where the shape can evolve
  (`taint`, `budget`, `inspect-value`, `debug` wrapper); `graph` and
  `lint` are arrays/objects fixed by this contract.
- Secrets never appear in any of these outputs; the redaction policy is
  upstream of the shape.
