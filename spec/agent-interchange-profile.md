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

- `version` — envelope schema version (major.minor.patch); majors 1 and
  2 are implemented (v2 adds `receipt` and `sender`).
- `capability` — the sandbox profile the sender evaluated under
  (`strict`/`data`/`agent`/`trusted`).
- `tainted` — whether the payload structurally contains secrets.
- `payload` — value tree or content-addressed references.
- `budget` — optional report (usage and headroom by limit).
- `receipt` — v2 required audit record (outcome, failure class
  [required when failed], diagnostics, result, schema version, profile,
  mode, resolved budget); it is present for failed attempts too (L-T06).
- `sender` — v2 optional identity `{ id, key?, signature? }`; a present
  key/signature pair is Ed25519 over the bound result hash plus the
  canonical receipt digest.
- `result` — content hash of the redacted payload. In v1 it binds the
  payload value tree only, so receipt comparison must compare the
  audit-trail fields (schema version, profile, budget, mode) alongside
  it (LANG-F-009). In v2 it binds payload + schema version + profile +
  resolved budget + mode + capability (`envelope::bound_content_hash`);
  a clean v2 result is required and must equal `receipt.result`.
- `diagnostics` — stable codes and paths; never values.

## 3b. Interchange-safe documents (LANG-F-007)

An interchange document (a Data Mode document with an `envelope` table) must
be **self-contained**: no `env.*` reads, no `include` statements, and no
profile-dependent values. Those three resolve against the RECEIVER's ambient
state, so a sender's `port = env.PORT ?: 8080` becomes 1111 on one host and
2222 on another, and `secret(env.TOKEN)` acts with the receiver's secret
under the sender's intent. Senders pin values (evaluate locally, then send
literals) before transmission.

Enforcement: the sender-side lint L017 reports each `env.*` read, include,
and profile in an interchange document, and the receiver gate refuses a
non-self-contained envelope with E036 before evaluation. A document that
carries no `envelope` table is not an interchange document and is unaffected.

## 4. Receiver rules

The receiver gate runs before evaluation on the default path: unknown
envelope major, capabilities outside the host's advertised set, and
missing/unparseable `version` are rejected. It is part of evaluation
(`Engine::envelope_gate`, default-on with the shipped policy: majors 1
and 2
and every tool this library ships), refuses with `E033` (version) and
`E034` (capabilities), and is fatal in strict and resilient modes alike.
`tainted` is cross-checked against the evaluated payload's structural taint:
a mismatch is a diagnostic (E035, fail-closed under strict). Structural
taint covers Secret-kind values, tainted blobs, any string the receiver's
redaction pass would redact, any table KEY whose text is material (a key
is data too), and any payload path the export renders as
material — so a plain value used as a `secret(...)` locator (a `let`, a
table field, an interpolation result, an env lookup) counts when the
payload carries a copy of it, and a selected profile override that
inherits materiality counts even when the replacement value carries no
material text (an integer at a material path). A missing, non-boolean, or
unverifiable claim (no payload member) also fails closed with E035: an
envelope that cannot be checked never passes.
`--sandbox` and `capability --check-doc` remain the explicit forms; a
host narrows the offered set with `EnvelopeGate::with_tools` and disables
the gate only for documents it already trusts.

1. Reject unknown envelope majors and unknown capabilities.
2. Honor taint: never render a payload containing secrets without
   redaction, and never copy secret material into logs.
2b. **Authorities (L-P35).** A v2 envelope's `capabilities` is the
   authority set it grants the receiver: `evaluate`, `read-secrets`,
   `inspect`, `apply`, `sign`. A receiver evaluates under the intersection
   of the declared set and its own held set, and refuses a declared set it
   does not hold (E049). The declared set must not exceed the sender's
   held set; the receipt's `attenuation {from, to, by}` must narrow
   (`to == capabilities`, `capabilities ⊆ from`) and is required for a
   clean outcome. Shape defects are E042; authority denials are E049. A
   receiver that must not accept the v1/local bridge enables
   `EnvelopeGate::with_required_capabilities` and refuses an envelope that
   omits the field.
3. Budgets are LOCAL. The envelope's `budget` field is a REPORT only
   (usage and headroom for audit/replay diagnostics); a receiver's ceilings
   always come from its own sandbox profile and are never read from the
   envelope. A sender cannot raise or lower a receiver's ceiling. If a
   ceiling-carrying field is ever wanted, it must be a separate, explicitly
   named field (`budget_limits`) with the rule that the effective ceiling is
   the MINIMUM of the local profile and the declared limits (LANG-F-005).
4. Preserve content-addressed identity: a hash names the same bytes on
   every host; do not re-encode a blob under a different media type.
5. Reject a document that claims Data Mode constructs without the
   marker (lint L010 is the sender-side check; capability refusal is
   the receiver-side gate).

## 4b. Cone fetches use demand-driven evaluation (LANG-C-013)

`eval --path a.b` prints the demanded cone, and it walks only that cone:
parse, then the whole-document static phase (schedule, shape scan,
reference-aware DP bounds, and every safety gate - all still
whole-document and fail-closed), then a value walk materialised for the
cone's closure, then redact and project. The printed value equals the
projection of the full value, and evaluation and validation diagnostics
are scoped to the closure: they are the full run's records restricted to
the demanded set, an ordered subsequence of the full run's.

`P` is a dotted path through the document root (`a.b`). A spelling the
cone resolver does not accept - an indexed path such as `arr[1]`, or a
top-level `let` binding name - keeps the pre-L-P40 whole-document walk:
the projection still yields the correct value, so for those spellings
the printed value is unchanged and only the reported cost model and
charge describe the whole document rather than a cone.

This is the LANG-C-013 fix: an undemanded pathological subtree can no
longer deny service to a demanded cone. A cone never hides a *static*
budget trip - the whole-document parse caps, static shape caps, and the
whole-graph DP total and depth run first and fail closed - but the
dynamic accountant that guards the walk charges only the demanded
closure, so an undemanded dynamic amplifier does not abort the fetch.

`--whole-document` on `eval` and `inspect` selects the pre-L-P40
behavior: walk the whole document while still printing the cone (the
compatibility door, for callers that need the whole-document
diagnostics). Whole-document surfaces keep whole-document semantics
regardless: `lock`, `convert`, `doc`, `gate`, envelopes, and
`debug --replay` all hash and diagnose the whole document.
`--audit-trail` on `eval` is one of them: it forces the
whole-document walk so the audit's hash and its `diagnostics` describe
one reproducible whole-document run, while `--path` still projects the
printed value. The engine capability lives in `active-core` and is
consumed by the RPC/Infinity layer; it lands with the L-P40 item
(`audit-pack/L-P40-DESIGN-R2.md`) and ships in the 2.0.0 line.

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

1. Identity is the `blake3:` digest alone, matched case-insensitively on
   input and emitted in canonical lowercase (LANG-C-016: uppercase hex is a
   common sender habit, and treating it as malformed made the receiver
   silently re-transfer every blob). Size and media type are
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
