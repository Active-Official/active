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

- `examples/agent-interchange/tool-result.active` is a valid envelope;
  `active-core/tests/agent_interchange.rs` parses, evaluates, and
  asserts the fields plus redaction.
- A secret-bearing envelope is covered by the same test: the evaluated
  value carries `***SECRET***`, never the material.
- The capability handshake is already gated by
  `active capability --check-doc` and the CAP phase tests.
