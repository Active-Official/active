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

Tainted blobs use the `active-blob/v2` domain:
`BLAKE3("active-blob/v2" || 0x00 || len(salt) || salt || len(media) || media || len(payload) || payload)`
where `salt` is a per-document salt. Untainted blobs keep the v1 domain.
Dual-hash transition vectors cover both domains during the migration.

Reference default for the salt: it is derived from the document's shape
(node kinds, table keys, array lengths, identifiers, and scalar literals,
with secret and blob payloads hashed by presence only), so identities stay
reproducible across runs and across embeddings - the determinism contract
and the cross-embedding differential gate both depend on that. What this
buys: the v2 domain binds each identity to one document, so identities from
different documents are unlinkable and an identity seen in isolation cannot
be matched against a precomputed table of low-entropy guesses from a
different document. Documents generated from the SAME template share a salt
(the shape is the same), so their identities are linkable to each other;
use `Engine::blob_salt` when that matters. What it does
NOT buy: confidentiality against someone who already holds the document -
they can recompute the derived salt and test payloads offline. A deployment
that needs that property sets `Engine::blob_salt` to a per-process random
value; the identity is then non-reproducible by design (like the wall clock)
and must not be pinned in an audit or lock artifact.

Reference pseudo-code:

```
digest = BLAKE3("active-blob/v1" || 0x00 || le64(len(media)) || media || le64(len(payload)) || payload)
```

### 1.1 Document salt (v2, G20)

The per-document salt is itself a BLAKE3 hash over a fixed, versioned
byte layout, so every embedding derives the same value for the same
document:

1. Domain `active-doc-salt/v1` (no trailing separator byte; the shipped salt hashes the domain text directly, verified against the 0018 vector).
2. The arena in node order. For each node, its index as `u64`
   little-endian, then one tag byte:
   `L` literal, `T` table, `A` array, `S` secret presence,
   `B` content-addressed presence, `I` ident, `R` ref, `E` env,
   `P` interpolate.
3. Per-tag payloads, all length-prefixed or fixed-width as stated:
   - Int, Float (raw IEEE-754 bits), and DurationMs: `u64` little-endian;
   - String and Secret: UTF-8 byte length only (no bytes);
   - Quantity: the unit suffix bytes;
   - Blob: the media-type bytes and one taint flag byte;
   - tables and arrays: their child count;
   - idents/env names/refs: the referenced index or name length, never
     secret material.
4. No host path, timestamp, process id, or environment value enters the
   salt; secret and blob payload bytes are represented by presence only.

The salt feeds the `active-blob/v2` domain for tainted identities; an
untainted blob keeps the v1 identity and never uses it. A host-supplied
`Engine::blob_salt` replaces the derived value and is a documented
non-reproducible choice. Tainted-identity vectors pin the layout.

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
- An **empty media type is B001**, whether written as a literal (`blob("", "x")`) or resolved at evaluation (`let m = ""; blob(m, "x")`). The identity has no valid form without a media type.
- A blob whose **payload copies material** is tainted at construction, even when the call site is not structurally material: `x = secret("s3cret-payload")` followed by `blob("text/plain", "s3cret-payload")` yields `tainted: true` with no retained payload. Deciding this at construction keeps `eval`, `convert`, `doc`, `debug`, `inspect`, `lock`, and `audit` in agreement; an identity-only export cannot inspect payload bytes later.
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
{ "logo": { "$active.blob": { "hash": "blake3:<64-hex>", "size": 5, "media_type": "text/plain", "tainted": false } } }
```

- Materialisation (`active eval --materialise-blobs`, or `eval --path` demanding the blob cone with the flag) adds `"payload_b64"`: base64 of the payload bytes. The generic renderers (`value_to_json`, the sealed `RedactedValue`'s `Serialize`, and the plain YAML/TOML exporters) are identity-only; the explicit forms are `value_to_json_materialised`, `value_to_pretty_json_materialised`, `value_to_yaml_string_materialised`, and `value_to_toml_string_materialised`, each taking the sealed `RedactedValue`, so profile redaction is enforced by the type rather than by a calling convention. Tainted blobs (including a blob whose media type is material) fail the demand with **B003**; blobs without retained payload fail with **B005** — both fail closed rather than silently emitting identity. With `--path`, the demand applies to the emitted cone: undemanded subtrees stay identities and cannot fail the fetch (budgets still guarded the full walk).
- Payload bytes are charged against `max_value_size` / `max_total_value_size` at materialisation time; breach aborts with the standard **X006** (materialisation consumes the existing budgets, it does not bypass them).
- String interpolation of a blob renders the display form `$active.blob(blake3:<hex>)`; a tainted blob's interpolation is structural material and is redacted on every export path. The form is display-only: consumers read the identity object, not the interpolated string.

## 6. Budget integration (CA.007)

The dependency-pass treats `ContentAddressed` as a leaf of known size (`size` bytes): no recursion, no expansion. Reference equality on blobs is constant-time hash equality. Expansion only happens at explicit materialisation, charged as above.

## 7. Taint integration (CA.008)

The structural taint bit travels with the blob on every export path (JSON identity, materialised JSON, lockfiles, provenance, graph queries, repair plans, audit trail): a tainted blob's identity object carries `"tainted": true`, and its payload is never emitted. A media type that is itself material (for example an `env` lookup used as a secret locator) taints the identity as well, and a tainted identity renders `"media_type": "***SECRET***"`: the identity's media field is never allowed to carry material. Media-type matching is deliberately broader than ordinary text redaction: an exact match at any length or an occurrence of a material value at any length taints the identity. The whole media string is replaced and the payload withheld, so no unrelated text can be rewritten, while a media type embedding a short secret cannot be published. The four-byte substring floor in spec/secrets.md §2 governs data text that redaction could corrupt; it does not relax this identity rule. An export that must redact a blob whose IDENTITY is material (a material media type or a material retained payload) refuses to vouch for its hash: it publishes the synthetic `blob::redacted_identity()` digest instead of a possibly v1-guessable one. This holds regardless of the `tainted` bit, because a caller-supplied boolean is not proof that the hash was produced under the salted v2 domain. An untainted value promoted only by its serving path (a profile override) is withheld the same way. A tainted blob whose identity itself is clean (a payload secret with a benign media type) keeps its real salted identity; for that case the `tainted` bit is the evaluation-provenance claim. Redaction obligations are identical to secrets: any raw tainted byte in any output is a gate failure.

Tainted-blob identity is salted (LANG-C-010): tainted blobs hash under the
`active-blob/v2` domain with a per-document salt (section 1), so an identity
seen in isolation cannot be dictionary-checked without the document that
produced it. B002 reports a tainted breach without the payload size: the
message names the breach and the schema bound, never the length. Mitigations
in force alongside: tainted payloads are never emitted anywhere, hashes never
appear in source, and high-entropy secret material (tokens, keys) is not
dictionary-enumerable. A host-supplied random salt (`Engine::blob_salt`) is
available when identity must also be unpredictable across evaluations; it
trades reproducibility for that.

## 8. Diagnostic family B00x (CA.010)

| Code | Meaning | Raised at |
|------|---------|-----------|
| B001 | Malformed `blob(...)` call (non-literal media type or data, wrong arity) | parse |
| B002 | Blob payload exceeds schema `max_size` (a tainted blob's payload size is never echoed: the message names the breach and the bound only, LANG-C-010) | validation |
| B003 | Materialisation refused: blob is tainted | eval (`--materialise-blobs`) |
| B004 | Blob media type mismatches schema `media_type` | validation |
| B005 | Materialisation impossible: payload not retained | eval (`--materialise-blobs` / demanded cone) |

All messages are stable and value-free (sizes are metadata; an evaluated media type or schema bound may be material, so B004/B002 echo only values the schema spells as literals, with material text redacted, and every identity redacts a tainted `media_type` — but messages never include payload bytes regardless).

## 9. Lockfile / provenance

`active lock` records blob identities (`blake3:<hex>`, size, media type, tainted) alongside content. Recomputing a lock over an unchanged document yields byte-identical blob lines (determinism gate covers this).

**End of blob.md — CA.001/CA.002 complete.**
