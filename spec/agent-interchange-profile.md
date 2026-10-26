# Agent-to-agent interchange profile (normative, v1)

**Back-reference:** GAP-CLOSURE §2.1; PROPOSAL §1.1, §2.5; Data Mode
doctrine. **Status:** normative for the 1.x line.

## 1. Purpose

Define how agents exchange long-lived state, plans, world models, and
work products in Active with structural taint, hard budgets, and
content-addressed identity. Tiny ephemeral tool calls stay JSON; this
profile is for data that is stored, replayed, or handed to another
agent.

## 2. Required shape

An interchange document is a valid Active document with:

1. **`active_mode = "data"`** (explicit). Config Mode documents are not
   interchange envelopes.
2. **`active_capabilities`** declaring the tools the sender expects the
   receiver to offer. The receiver runs the handshake
   (`active capability --check-doc <file>`) and refuses unknown entries
   before evaluating.
3. **A versioned envelope** (major.minor.patch). Receivers reject an
   unknown major.
4. **Payloads above a locally chosen threshold are content-addressed**
   (`blob(...)`), and a sender may short-circuit with "I already possess
   hash H" using the identity fields (hash, size, media type).
5. **Secrets stay tainted**; every observation path redacts. A payload
   containing a secret is legal; a raw secret in an envelope is a
   defect.
6. **Budgets** are declared or inherited from a sandbox profile; hard
   ceilings are never disabled by the profile.

## 3. Envelope fields

```active
active_mode = "data"
active_capabilities = ["check", "eval"]

envelope = {
  version = "1.0.0",
  capability = "agent",
  tainted = false,
  payload = { ok = true, port = 8080 },
  budget = { nodes = 120, headroom = 0.88 },
  diagnostics = [],
  result = "sha256:…",
}
```

- `version` — envelope schema version (major.minor.patch).
- `capability` — the sandbox profile the sender evaluated under
  (`strict`/`data`/`agent`/`trusted`).
- `tainted` — whether the payload structurally contains secrets.
- `payload` — value tree or content-addressed references.
- `budget` — optional report (usage and headroom by limit).
- `diagnostics` — stable codes and paths; never values.
- `result` — optional content hash of the redacted payload (the audit
  trail's hash, so replay and audit agree).

## 4. Receiver rules

1. Reject unknown envelope majors and unknown capabilities.
2. Honor taint: never render a payload containing secrets without
   redaction, and never copy secret material into logs.
3. Honor budgets: evaluate under the declared or profile budget; never
   raise a ceiling silently.
4. Preserve content-addressed identity: a hash names the same bytes on
   every host; do not re-encode a blob under a different media type.
5. Reject a document that claims Data Mode constructs without the
   marker (lint L010 is the sender-side check; capability refusal is
   the receiver-side gate).

## 5. Non-goals

- No multi-agent orchestration protocol, transport, or scheduling.
- No replacement for JSON in small tool calls.
- No encryption: redaction is the mechanism; content addressing is
  identity, not confidentiality.

## 6. Conformance

- `examples/agent-interchange/` ships three envelopes (tool result,
  plan, world model). `active-core/tests/agent_interchange.rs` parses,
  evaluates, and asserts the required fields, Data Mode, the capability
  handshake for the named profile, and redaction for every file in the
  directory.
- A secret-bearing envelope is covered by the same test: the evaluated
  value carries `***SECRET***`, never the material.
- The capability handshake is already gated by
  `active capability --check-doc` and the CAP phase tests.
- The hash short-circuit has executable helpers plus a vector under
  `agent-kit/interchange/` (see §7).

## 7. Hash short-circuit ("I already possess hash H")

Content addressing makes transfer optional. A sender advertises the
identities it would otherwise transmit; a receiver answers with the
subset it already holds and requests only the rest.

**Message shape.** A request lists records, never payload bytes:

```json
{"needs": [
  {"hash": "blake3:<64 hex>", "size": 11, "media_type": "application/json"}
]}
```

A receiver replies:

```json
{"have": ["blake3:<64 hex>"], "want": [{"hash": "blake3:<64 hex>", "size": 11}]}
```

**Rules.**

1. Identity is the `blake3:` digest alone. Size and media type are
   transfer hints; they never change which bytes a hash names.
2. Taint never affects identity. A tainted blob whose bytes the
   receiver already holds still short-circuits; redaction applies to
   rendering, not to matching.
3. A malformed identity (wrong prefix, wrong length, non-hex) never
   matches. Fail closed: request the bytes.
4. A record is a leaf. Walking a payload for identities never opens a
   blob payload, so a receiver cannot be forced to materialise one by a
   short-circuit probe.
5. The `result` field of an envelope is the same canonical hash, so a
   receiver can compare results without a payload transfer.

**Reference implementation.** `agent-kit/interchange/short_circuit.py`
exports `normalize_identity`, `short_circuits`, and
`missing_identities`; `agent-kit/interchange/vectors/short-circuit.json`
pins a real identity from the shipped world-model example, and
`test_short_circuit.py` runs in CI.
