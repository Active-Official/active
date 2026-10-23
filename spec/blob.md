# Content-Addressed Values + Blob (normative)

**Version:** 1.0
**Back-reference:** PROPOSAL §1.1; CHECKLIST Phase CA (CA.001–CA.013); GAP-CLOSURE §1.2.
**Status:** Normative. This document fixes the canonical byte representation (CA.001) and the hash function selection (CA.002).

---

## 1. Canonical byte representation (CA.001)

The identity of a blob is the BLAKE3-256 hash of exactly these bytes, in order:

1. Domain separator: the 15 ASCII bytes `active-blob/v1`, followed by one `0x00` byte.
2. `u64` little-endian: byte length of the media type string (UTF-8).
3. Media type bytes (UTF-8, e.g. `text/plain`). Must be non-empty; normalization is exact-match (no case folding, no parameter stripping — `Text/Plain` and `text/plain` are different types).
4. `u64` little-endian: byte length of the payload.
5. Payload bytes verbatim.

No field order ambiguity exists (fixed order), no string normalisation is applied (bytes are bytes), no unit canonicalisation applies (payloads are opaque). The same payload under a different media type is a different blob by construction.

Reference pseudo-code:

```
digest = BLAKE3("active-blob/v1" || 0x00 || le64(len(media)) || media || le64(len(payload)) || payload)
```

## 2. Hash function selection (CA.002)

**Selected: BLAKE3-256** (`blake3` crate 1.x). Rationale, recorded:

- 256-bit security with a modern, reviewed design; fixed output length keeps `Node`/`Value` layouts static (no length prefix needed).
- Fast on all Tier-1 host platforms (Rust, C, WASM, Go, Python, JS embeddings all have BLAKE3 bindings if they ever need to recompute; the reference implementation is this crate).
- Deterministic across platforms by specification (no seed, no key, no randomize flag).
- Documented fallback, never silently substituted: if BLAKE3 becomes unavailable on a target, the build fails (hard dependency) rather than downgrading to SHA-256. A future algorithm migration would be a new domain separator (`active-blob/v2`) with dual-hash transition vectors, not a silent swap.

Test vectors (payload `hello`, media type `text/plain`):

- Canonical input (hex): `6163746976652d626c6f622f7631000a00000000000000746578742f706c61696e050000000000000068656c6c6f`
- Implementations MUST assert the full 32-byte digest below (pinned in `active-core` unit tests; any change fails the gate):

```
blake3("active-blob/v1" || 0x00 || le64(10) || "text/plain" || le64(5) || "hello")
```

(The digest constant lives in code so the doc never drifts from the implementation: see `blob::CANONICAL_TEST_DIGEST`.)

## 3. Authoring surface

`blob(media_type, data)` where both arguments are string literals
(`let`-bound identifiers and key aliases are accepted at parse time
and shape-checked at evaluation):

```active
logo = blob("image/svg+xml", "<svg>...</svg>")
notes = blob("text/plain", "hello")
tainted = blob("text/plain", secret.env("TOKEN"))
```

- `blob` is a reserved keyword (like `secret`, `env`, `let`): it cannot be used as a bare key. Arity and shape errors are **B001**, never generic codes.
- Hashes never appear in ordinary Active source (only in evaluated JSON, lockfiles, and provenance).
- `data` may also be a `secret(...)` / `secret.env(...)` locator: the blob is then structurally tainted and can never be materialised (B003).
- Binary content today: embed as string data. A future `blob_file` (filesystem-backed) is explicitly deferred — it needs loader plumbing with trust verification, and string blobs cover config-scale payloads. Binary payloads that must stay byte-exact should use an escaping-safe ASCII form until `blob_file` lands.

## 4. Schema surface

`Blob` is a first-class schema type, bare or with constraints:

```active
schema = { logo = Blob, upload = Blob(media_type = "image/png", max_size = 262144) }
```

- `media_type` (optional string): exact-match expectation; mismatch is diagnostic **B004**.
- `max_size` (optional int): maximum payload bytes; breach is diagnostic **B002**.
- A `Blob`-typed field holding a non-blob value is a type error (T002 path unchanged).

## 5. Evaluated JSON / interchange shape (CA.006)

Blobs evaluate to an identity object; the payload is absent unless explicitly materialised and the blob is untainted:

```json
{ "logo": { "$active.blob": { "hash": "blake3:<64-hex>", "size": 5, "media_type": "text/plain" } } }
```

- Materialisation (`active eval --materialise-blobs`, or `eval --path` demanding the blob cone with the flag) adds `"payload_b64"`: base64 of the payload bytes. Tainted blobs fail the demand with **B003**; blobs without retained payload fail with **B005** — both fail closed rather than silently emitting identity. With `--path`, the demand applies to the emitted cone: undemanded subtrees stay identities and cannot fail the fetch (budgets still guarded the full walk).
- Payload bytes are charged against `max_value_size` / `max_total_value_size` at materialisation time; breach aborts with the standard **X006** (materialisation consumes the existing budgets, it does not bypass them).

## 6. Budget integration (CA.007)

The dependency-pass treats `ContentAddressed` as a leaf of known size (`size` bytes): no recursion, no expansion. Reference equality on blobs is constant-time hash equality. Expansion only happens at explicit materialisation, charged as above.

## 7. Taint integration (CA.008)

The structural taint bit travels with the blob on every export path (JSON identity, materialised JSON, lockfiles, provenance, graph queries, repair plans, audit trail): a tainted blob's identity object carries `"tainted": true`, and its payload is never emitted. Redaction obligations are identical to secrets: any raw tainted byte in any output is a gate failure.

Known limitation (documented, not hidden): the identity hash of a tainted blob is deterministic over the secret bytes, so a low-entropy secret is theoretically subject to an offline dictionary check by anyone who can already see the identity object. Mitigations in force: tainted payloads are never emitted anywhere, hashes never appear in source, and high-entropy secret material (tokens, keys) is not dictionary-enumerable. If a deployment needs identity-hiding for tainted blobs, the extension point is a per-document salt in the canonical bytes (domain `active-blob/v2`) — a deliberate future change, never a silent one.

## 8. Diagnostic family B00x (CA.010)

| Code | Meaning | Raised at |
|------|---------|-----------|
| B001 | Malformed `blob(...)` call (non-literal media type or data, wrong arity) | parse |
| B002 | Blob payload exceeds schema `max_size` | validation |
| B003 | Materialisation refused: blob is tainted | eval (`--materialise-blobs`) |
| B004 | Blob media type mismatches schema `media_type` | validation |
| B005 | Materialisation impossible: payload not retained | eval (`--materialise-blobs` / demanded cone) |

All messages are stable and value-free (sizes and media types are metadata, never secret material — but messages never include payload bytes regardless).

## 9. Lockfile / provenance

`active lock` records blob identities (`blake3:<hex>`, size, media type, tainted) alongside content. Recomputing a lock over an unchanged document yields byte-identical blob lines (determinism gate covers this).

**End of blob.md — CA.001/CA.002 complete.**
