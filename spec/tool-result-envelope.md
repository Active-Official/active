# Secret-safe tool-result envelope (normative, v1)

**Back-reference:** GAP-CLOSURE §2.9. **Status:** normative for the 1.x
line. This is the transport-agnostic result shape agents exchange when
plain JSON cannot carry taint, budgets, or content-addressed identity.

## 1. Minimum contents

| Field | Kind | Meaning |
|---|---|---|
| `version` | string `major.minor.patch` | Envelope schema version; unknown major is rejected |
| `capability` | string | Sandbox profile used for evaluation (`strict`/`data`/`agent`/`trusted`) |
| `tainted` | boolean | The payload structurally contains a secret |
| `payload` | value or blob reference | The result itself |
| `budget` | optional object | `{nodes, depth, keys, value_size, total_size, headroom}` as observed |
| `diagnostics` | optional array | `{code, severity, path, span}` — never values |
| `repair` | optional plan | Multi-step plan (`RepairOutcome`) when the result needs work |
| `result` | optional string | Content hash of the redacted payload (`sha256:...`). `result` binds the payload value tree only. It does not bind the schema version, the profile, the budget, or the mode semantics. Receipt comparison must compare the audit-trail fields (schema version, profile, budget, mode) alongside `result`; comparing `result` alone over-trusts the receipt (LANG-F-009). |

## 2. Rules

1. **No raw secrets.** The envelope never carries secret material on any
   path: payloads render through the redactor, diagnostics are
   value-free for secrets, and repair plans omit source text.
2. **Honor taint.** A receiver that displays or logs a `tainted: true`
   payload must preserve redaction; copying secret material into a log
   is a protocol violation.
3. **Budgets are local.** The `budget` field is a REPORT of usage and
   headroom, never a ceiling: evaluation under the envelope uses the
   RECEIVER's own sandbox profile, and a ceiling is never read from the
   sender's envelope, raised, or lowered by it (LANG-F-005). A future
   ceiling-carrying field must be separately named and the effective ceiling
   is the minimum of the local profile and the declared limits.
4. **Version gate.** A missing or non-parseable `version` is rejected;
   an unknown major is rejected. The gate runs before evaluation on the
   default path (spec/agent-interchange-profile.md §4): `E033` for the
   version, `E034` for capabilities outside the receiver's offered set,
   fatal in strict and resilient modes alike.
5. **Content addressing.** A blob payload is identified by
   `blake3:<hex>`, size, and media type; receivers may short-circuit on
   identity equality.

## 3. JSON mapping

When a host cannot speak Active, the same fields map to JSON with these
invariants:

- Secrets → `"***SECRET***"` (never a value, never a partial hash of a
  low-entropy secret).
- Blobs → `{"$active.blob": {"hash": "blake3:…", "size": N,
  "media_type": "…", "tainted": bool}}`.
- Durations → `{"ms": N}`; quantities → `{"<suffix>": N}`.
- Table key order is source order; consumers must not rely on it for
  identity (use `result`).

The mapping loses no taint, hash, or budget metadata.

## 4. Example

`examples/agent-interchange/tool-result.active` is a conforming
envelope: Data Mode, declared capabilities, version, taint summary,
payload, and budget report. A secret-bearing variant is exercised by
`active-core/tests/agent_interchange.rs` and must evaluate with
`***SECRET***`.

## 5. Relationship to the interchange profile

The interchange profile (`spec/agent-interchange-profile.md`) describes
the *document* contract for long-lived state; this envelope describes
the *result* contract for one tool call. Both share taint, budget, and
content-addressing rules, and both reject unknown majors.
