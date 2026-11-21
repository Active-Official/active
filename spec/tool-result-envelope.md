# Secret-safe tool-result envelope (normative, v1 and v2)

**Back-reference:** GAP-CLOSURE §2.9; L-T06. **Status:** normative for
the 1.x/2.x lines. This is the transport-agnostic result shape agents
exchange when plain JSON cannot carry taint, budgets, receipts, sender
identity, or content-addressed identity.

## 1. Minimum contents

| Field | Kind | Meaning |
|---|---|---|
| `version` | string `major.minor.patch` | Envelope schema version. Majors 1 and 2 are implemented; unknown majors are rejected |
| `capability` | string | Sandbox profile used for evaluation (`strict`/`data`/`agent`/`trusted`) |
| `tainted` | boolean | The payload structurally contains a secret |
| `payload` | value or blob reference | The result itself |
| `budget` | optional object | `{nodes, depth, keys, value_size, total_size, headroom}` as observed |
| `diagnostics` | optional array | `{code, severity, path, span}` - never values |
| `repair` | optional plan | Multi-step plan (`RepairOutcome`) when the result needs work |
| `receipt` | **v2 required** object | Audit record for this result, including adversarial outcomes |
| `sender` | optional v2 object | `{ id, key?, signature? }` sender identity |
| `result` | optional string | Content hash. v1 binds the payload value tree only; v2 binds payload + schema version + profile + resolved budget + mode + capability. **Required for a clean v2 outcome** and must equal `receipt.result` |

### 1.1 Receipt (v2)

`receipt` carries the same evidence an `--audit-trail` file records, so
a parse failure or budget trip is exchangeable even when there is no
payload:

| Field | Kind | Meaning |
|---|---|---|
| `outcome` | `"clean"` or `"failed"` | Required. Whether the attempt produced a usable result |
| `failure_class` | `"parse"`, `"budget"`, `"validation"`, `"other"` | **Required when `outcome = "failed"`**; validated when present |
| `diagnostics` | optional array | Codes and paths; never values |
| `result` | optional string | The v2 bound content hash; required for a clean outcome and equal to `envelope.result` |
| `schema_version` | optional string | The schema version that validated the payload |
| `profile` | optional string | The effective profile selector |
| `mode` | optional `"config"` or `"data"` | The evaluated mode (closed set). **Required for a clean outcome** |
| `budget` | optional object | The **resolved limits** used for evaluation (the envelope's top-level `budget` is observed usage/headroom). **Required for a clean outcome**, with all seven fields: `nodes`, `depth`, `array_len`, `keys`, `wall_ms`, `value_size`, `total_size`, each a positive integer |
| `diagnostics` | optional array of strings | Stable diagnostic codes (the audit file carries the full objects); never values |
| `include_resolutions` | optional array | `{path, resolved, sha256, source}` when includes were resolved |
| `applied` | optional array | L-P41: the commit set a repair receipt describes: `{kind, path, status, side_effects?, proof?}` entries. Paths, kinds, statuses, and proof codes/paths only — a repair directive can carry a value, so the directive text and every value are omitted. On a failed repair receipt it records the refused/attempted set |
| `document_clean` | optional boolean | L-P41: a repair receipt's verdict on the resulting document (independent of the tool outcome: a committed partial repair can be clean=false) |
| `closed_tables` | optional array | L-P11 audit of every closed schema table instance in a **clean** run: `{path, keys}` and exactly those two members. `path` is the dotted document path (`"."` is the root); `keys` are the key names present at that instance, sorted. Key names only — values and material never appear. Several structurally matching disjunction branches at one path record once. Absent when the schema declares no closed table or the outcome is failed |

A v2 envelope without `receipt` is refused with **E042**. A malformed
receipt (non-table, unknown `outcome`, unknown `failure_class`, a
non-`sha256:` result, or a malformed `closed_tables` member — not an array
of exactly `{path, keys}` tables with string names) is E042.
`closed_tables` is a sender claim bound by `receipt_digest` (and therefore
by a sender signature when one is present); the receiver gates its shape
and does not recompute it, because the envelope does not carry the schema
the claim was derived from. Because the claim is untrusted text, the
receiver refuses an audit entry that echoes text it already knows is
material (a producer redacts; a sender-side secret the receiver never saw
cannot be detected). Tools that log an envelope must treat every receipt
string as untrusted data, never as verified provenance.

### 1.2 Sender identity (v2)

`sender` names who produced the envelope:

- `id` - required, non-empty, control-free sender identity (an agent
  URI, a service name, a key id). It is an assertion unless `key` and
  `signature` are present.
- `key` - optional valid key id from the receiver's trust set.
- `signature` - optional Ed25519 signature, 128 hex characters, over
  `sender_message(id, key, version, result, receipt_digest)` with
  length-prefixed fields. `key` and `signature` are given together, and
  `receipt_digest` is the canonical digest of the full receipt
  (`envelope::receipt_digest`), so outcome, failure class, mode, profile,
  schema version, budget, and the closed-table audit cannot change after
  signing.

The signature is verified by the receiver policy step
(`envelope::verify_sender(public_hex, id, key, version, result,
receipt_digest, signature)`), not by the pre-evaluation gate. This binds
authorship to the exact payload and receipt: reusing the signature with
another result, receipt, sender, or key fails. A malformed sender block
is **E043**; an invalid signature is a receiver-policy refusal, not a
syntax error.

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
   ceiling-carrying field must be separately named and the effective
   ceiling is the minimum of the local profile and the declared limits.
4. **Version gate.** A missing or non-parseable `version` is rejected;
   an unknown major is rejected (majors 1 and 2 are implemented). The
   gate runs before evaluation on the default path
   (spec/agent-interchange-profile.md §4): `E033` for the version, `E034`
   for capabilities outside the receiver's offered set, fatal in strict
   and resilient modes alike.
5. **Receipts survive failure.** A v2 envelope reports parse, budget,
   validation, and evaluation failures with `receipt.outcome = "failed"`
   and a `failure_class`; a failed attempt is never an empty envelope
   (LANG-F-010).
6. **Content addressing and binding.** A blob payload is identified by
   `blake3:<hex>`, size, and media type; receivers may short-circuit on
   identity equality. In v2, `result` (and `receipt.result`) MUST be
   `sha256:` over the canonical (key-sorted) tuple
   `{payload, schema_version, profile, resolved budget limits, mode,
   capability}` (the helper is `envelope::bound_content_hash`). A clean
   v2 result is required and the two fields must be equal. A v1 `result`
   binds the payload only; comparing it alone over-trusts the receipt
   (LANG-F-009).
7. **Sender binding.** A sender identity names its producer; when
   `sender.key`/`sender.signature` are present, the receiver verifies
   them against the bound result and the canonical receipt digest before
   acting. Signatures bind bytes by construction; the sender block adds
   *who* and *which receipt*, and the receipt digest prevents post-signing
   changes to outcome/class/mode/profile/schema/budget.
8. **Post-evaluation verification (normative).** The pre-evaluation gate
   sees the source AST only. Profiles and patches can rewrite or inject
   `envelope`, so every embedding re-checks the evaluated tree
   (`envelope::check_evaluated`): version, capabilities, self-containment,
   v2 shapes, and the bound result. A mismatch is **E044**, fatal in
   strict and resilient modes alike. The CLI emitter (`active eval
   --envelope <out.json>`) writes the same shape for success and for
   parse/budget/validation failures (`payload = null`, failed receipt).

## 3. JSON mapping

When a host cannot speak Active, the same fields map to JSON with these
invariants:

- Secrets → `"***SECRET***"` (never a value, never a partial hash of a
  low-entropy secret).
- Blobs → `{"$active.blob": {"hash": "blake3:.", "size": N,
  "media_type": ".", "tainted": bool}}`.
- Durations → `{"ms": N}`; quantities → `{"<suffix>": N}`.
- `receipt` and `sender` are ordinary JSON objects with the fields in
  §1.1/§1.2; `diagnostics` are stable code strings in the envelope (the
  audit artifact carries the full code/path objects), never values.
- Table key order is source order; consumers must not rely on it for
  identity (use `result`).

The mapping loses no taint, hash, budget, receipt, or sender metadata.

## 4. Example

`examples/agent-interchange/tool-result.active` is a conforming v1
envelope. A v2 envelope adds the required receipt, the bound result, and
an optional sender identity:

```active
active_mode = "data"
envelope = {
  version = "2.0.0",
  capability = "agent",
  tainted = false,
  payload = { ok = true, port = 8080 },
  receipt = {
    outcome = "clean",
    result = "sha256:<bound hash>",
    mode = "data"
  },
  sender = { id = "agent://build-17" },
  result = "sha256:<bound hash>"
}
```

A secret-bearing variant is exercised by
`active-core/tests/agent_interchange.rs` and must evaluate with
`***SECRET***`. Conformance vectors 0039/0040 cover a clean v2 envelope
and a v2 envelope missing its receipt.

## 5. Relationship to the interchange profile

The interchange profile (`spec/agent-interchange-profile.md`) describes
the *document* contract for long-lived state; this envelope describes
the *result* contract for one tool call. Both share taint, budget, and
content-addressing rules, and both reject unknown majors.
