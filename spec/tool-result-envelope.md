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
| `result` | optional string | Content hash of the redacted payload (`sha256:…`) |

## 2. Rules

1. **No raw secrets.** The envelope never carries secret material on any
   path: payloads render through the redactor, diagnostics are
   value-free for secrets, and repair plans omit source text.
2. **Honor taint.** A receiver that displays or logs a `tainted: true`
   payload must preserve redaction; copying secret material into a log
   is a protocol violation.
3. **Honor budgets.** Evaluation under the envelope uses the declared
   or profile budget; ceilings are never silently raised.
4. **Version gate.** A missing or non-parseable `version` is rejected;
   an unknown major is rejected.
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
