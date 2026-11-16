# RPC interop: three envelopes, distinct names, one mapping (LANG-F-011)

> Status: normative mapping owned by this repository (DEC-001 envelope
> cleanup). The RPC repository (`Active-Official/active-rpc`) mirrors this
> table in its `docs/spec/interop.md`; where the two disagree, this table
> governs the language-level fields and the RPC table governs transport
> framing.

## 1. Three envelopes, three names

The word "envelope" names three different shapes in this stack. They are
distinct types with distinct owners, and no document is a valid instance of
two of them:

| Name | Owner | Definition | What it carries |
|---|---|---|---|
| **DocumentEnvelope** | this repository | `spec/tool-result-envelope.md`, `spec/agent-interchange-profile.md` §3 | Language-level interchange: `version`, `capability`, `tainted`, `payload`, `budget` report, `diagnostics`, `result` hash |
| **ResultEnvelope** | this repository (`active-wasm`, `active-ffi`) | `active-wasm/src/lib.rs` (`envelope_ok`/`envelope_err`) | One evaluation's machine result: `ok`, `wall_supported`, `json`, `diagnostics`, `items` - JSON, never the Active binary form |
| **ARP1Envelope** | `Active-Official/active-rpc` | that repository's `spec/` | Transport framing: message id, method, version negotiation, lane budgets, errors |

The old habit of calling all three "the envelope" is what this document
removes: an implementer wiring Active into RPC needs to know which one it is
holding at each layer.

## 2. The decision: the DocumentEnvelope travels INSIDE ARP1 payloads

The shared contract is the `spec/tool-result-envelope.md` shape, extended
with framing fields (length, `content_hash`, declared budget). The document
envelope travels inside ARP1 payloads, not superseded by them: ARP1 frames a
message, and the DocumentEnvelope is the message's *content* when that
content is an Active document result. Superseding it would move taint,
budget reporting, and result identity into a transport that must not
reinterpret them.

Consequences, stated so they are not rediscovered:

- ARP1 framing errors are transport errors and never produce a DocumentEnvelope.
- A DocumentEnvelope inside a valid ARP1 frame is still gated by the
  receiver gate (`spec/agent-interchange-profile.md` §4: `E033`, `E034`,
  `E035`) before evaluation.
- The ResultEnvelope is what an in-process embedding returns (WASM/FFI); a
  host bridging to RPC converts it to a DocumentEnvelope, it is not sent raw.

## 3. Field-by-field mapping (DocumentEnvelope ↔ ARP1)

| DocumentEnvelope field | ARP1 counterpart | Rule |
|---|---|---|
| `version` (semantic envelope version, major gate `E033`) | ARP1 protocol version negotiation | Independent. ARP1 negotiates the transport; the document major gate runs only after ARP1 framing succeeds. Both must pass; neither substitutes for the other. |
| `capability` (profile name) | ARP1 capability table | Disjoint vocabularies (LANG-F-012). The document name is a sandbox profile name; ARP1 entries are transport features. A bridge declares its ARP1 features and its offered document tools separately; the document `active_capabilities` handshake (`E034`) is checked against the offered *tools* only. |
| `active_capabilities` (declared tools) | (none) | Checked against the receiver's offered tool set by the gate; ARP1 has no equivalent and must not widen or narrow it. |
| `tainted` (structural-taint claim) | (none) | Verified by the receiver against the evaluated payload (`E035`). Transport must not set, clear, or infer it. |
| `payload` | ARP1 payload body | The payload is opaque bytes to ARP1. Active semantics (units, secrets, blobs) live inside it. |
| `budget` (observed usage/headroom report) | ARP1 lane budgets | Different objects: the document `budget` is a REPORT of what evaluation used; ARP1 lane budgets are transport-side ceilings. A receiver's ceilings come from its own profile, never from either field (LANG-F-005). |
| `result` (payload content hash) | ARP1/observability correlation ids | Different identities: `result` identifies the payload value tree (and only that, LANG-F-009); correlation ids identify the request in transport traces. A receipt needs both. |
| `diagnostics` | ARP1 error channel | Language diagnostics (`E…`/`T…`/`W…`/`X…` codes) stay inside the DocumentEnvelope; ARP1 errors describe transport failures. A bridge must not map a document diagnostic to an ARP1 error code, because codes are stable API. |

## 3b. Three capability layers, three names (LANG-F-012)

"Capability" names three different authorities. They must never be presented
as one mechanism:

| Layer | Name | What it authorises | Where it lives |
|---|---|---|---|
| Transport | **RPC capabilities** | WHO may invoke which method on which module, with attenuation and revocation | ARP1 (the RPC repository) |
| Content | **document capability handshake** (`active_capabilities`) | WHICH host tools the document asks the receiver to run (checked against the receiver's offered tool set, `E034`) | The DocumentEnvelope / Data Mode root |
| Budget | **profile** (the envelope's `capability` field today) | WHICH budget preset applies (`strict`, `data`, `agent`, `trusted`) - a budget profile, never an authority | The envelope's `capability` field |

Rules:

- The document handshake is peer-blind: it cannot express who sent the
  document, attenuation, or revocation. It is not an RPC authorisation
  mechanism and must never be presented as one.
- The envelope's `capability` field names a *budget profile*; `agent` and
  `trusted` have identical tool lists and differ only in budget. It is
  renamed to `profile` in the v2 envelope to end the collision; receivers
  accept both spellings until then.
- RPC capabilities authorise the call and are decided by the transport host
  before a DocumentEnvelope is examined.

## 4. ResultEnvelope ↔ DocumentEnvelope

| ResultEnvelope field | DocumentEnvelope counterpart |
|---|---|
| `ok` | `diagnostics` empty and evaluation succeeded |
| `json` | `payload` (the JSON rendering of the evaluated tree) |
| `diagnostics` / `items` | `diagnostics` (same codes; the ResultEnvelope adds per-item spans) |
| `wall_supported` | no counterpart: it is an embedding capability flag (LANG-C-012) that a bridge must surface to its own host |

A host that has a ResultEnvelope and needs to transmit it wraps the payload
into a DocumentEnvelope with a version, the capabilities it used, and the
computed `result` hash; it does not forward the ResultEnvelope.
