# Partial Evaluation & Incremental Memo (normative)

**Version:** 1.0
**Back-reference:** PROPOSAL §2.2–§2.3; CHECKLIST Phase PE (PE.001–PE.006); GAP-CLOSURE §2.6.
**Status:** Normative. This document fixes the demand-path surface, the fingerprint definition, and the memo contract.

---

## 1. `eval --path` (PE.001)

`active eval --from doc.active --path server.host` emits the value at
the demanded cone (dotted/index path language shared with `@patch`:
`a.b[0].c`). Unknown or malformed paths fail closed naming the path.

Demand is an explicit input (L-P40 §4–§5). `--path` selects the
demand `Cone(P)`; `--whole-document` selects `Demand::All`, the
pre-L-P40 whole-tree walk, and is the compatibility door on `eval`,
`inspect`, and `budget`. Semantics, stated precisely:

- The WHOLE-document STATIC phase always runs first and fails closed:
  parse caps (E029), include caps (E028), cycles (G16), static shape
  (X001–X005), the whole-graph DP bound (the static X006 ceiling), and
  the whole-arena safety gates (reader floor, envelope, capability,
  purity, sandbox). A document the full run refuses statically is
  refused for every cone, with the same code and ordering. A cone
  never hides a static budget trip.
- The DYNAMIC accountant is demand-scoped: the value walk materialises
  only the cone's closure (the demanded subtree, its ancestors, and
  the transform, check, and envelope inputs it reads), and the
  accountant is seeded with that closure's DP total instead of the
  whole graph's. An undemanded pathological subtree cannot deny
  service to the demanded cone (LANG-C-013); measured growth inside
  the cone still charges X006 at the point a full run would.
- The cone's diagnostics are a subsequence of the full run's, in the
  same topological order.
- Projection equality: the emitted cone value equals the projection of
  the full value at `P`, with the same `content_hash`. A demanded
  subtree evaluates to the same value whether or not its siblings were
  materialised.
- Secret seeds are closure-scoped over every `secret(...)` locator:
  each `secret(...)` locator subtree is a closure member, so the walk
  seeds the runtime text the locator computes even when the secret's
  own path is undemanded. A demanded value that copies that text is
  redacted exactly as its projection; a cone never prints material the
  full run redacts.
- Blob payloads follow the usual materialisation flag: without
  `--materialise-blobs` every blob stays an identity, so undemanded
  subtrees serialize small by construction.
- Limitation: the closure resolver narrows the demand for table paths
  only. An indexed path (`a.b[2]`) does not resolve to a cone, so the
  run falls back to `Demand::All` — the charge is whole-document while
  the emitted value is still the correct projection at `P`. A
  top-level `let` name is not a document key and is not projectable by
  `--path`; referencing it from a document key reads it.
- The FFI equivalent is `active_value_project(value, path, len)`:
  JSON for the cone, or null with `active_last_error` set.
- Agents explore large documents through cones by default;
  reference loops may fetch a cone instead of the whole tree
  (agent-kit documents the pattern).

## 2. Fingerprint definition (PE.002)

Incremental memo key = hash of:

1. Source text (exact bytes),
2. Environment snapshot (the closed map the engine read),
3. Active profile selection (document key or `--profile` override),
4. Budget configuration (all seven limits, by value),
5. `materialise_blobs` flag,
6. Mode (Config vs Data — a Data evaluation must never serve a
   Config memo's warning-free diagnostics, and vice versa).

Canonicalisation: sources hash as given (no normalization — different
bytes are different documents by the one-file→one-meaning rule);
environment maps hash sorted by key; per-node structural fingerprints
hash expression shape, literal data, env values actually read, and
already-computed dependency fingerprints topologically with a
per-memo salt; blobs hash by identity (hash + size + taint), never by
payload bytes. Any configuration change clears the store before the
walk, so a flag flip can never serve a stale identity-only tree as
materialised.

## 3. Public incremental memo API (PE.003)

`Engine::evaluate_incremental(&module, &mut IncrementalMemo)`:
same fingerprint → microsecond reuse of the stored tree plus its
diagnostics. The store is salted per memo, bounded (eviction only
loses hits, never correctness), and carries `last_eval_count` /
`last_memo_hits` telemetry. The language server keeps one memo per
open document: repeat analyses of an untouched buffer are cache hits
instead of full re-walks.

## 4. Content-addressed integration (PE.004)

- Memo fingerprints hash blobs by IDENTITY (hash + size + taint),
  never by payload bytes: same identity ⇒ same value, and no secret
  or payload material ever enters a memo key.
- Hash short-circuit ("I already possess hash H") rides on identity
  equality: constant-time hash comparison replaces structural walks
  for blob-heavy documents.

## 5. Vectors (PE.005)

Covered in-repo: demanded cones (nested, indexed, missing-path
errors), the cone-never-hides-a-static-trip rule, the cone-spares-an-
undemanded-amplifier rule, cone diagnostics as the full run's ordered
subsequence, memo hit/miss telemetry, materialise-flag cache
separation, blob payload charging per alias. The conformance cases
`0075-cone-static-array-trip`, `0076-cone-spares-secret-amplifier`,
and `0077-cone-suppresses-undemanded-schema-trip` carry the demand
axis; the reference core test is their single consumer, because only
it honours `path`/`budget` (the Python diff harness, the WASM runner,
and the shim embeddings skip them wholesale, L-P40 §2.4). Projection
is enforced at the value level by the engine tests
(`cone_value == project(full_value, P)`), so the shared Tier-1 harness
still verifies the identical tree it already verifies.

**End of partial-eval.md — PE.001–PE.006 complete.**
