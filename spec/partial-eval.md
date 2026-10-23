# Partial Evaluation & Incremental Memo (normative)

**Version:** 1.0
**Back-reference:** PROPOSAL §2.2–§2.3; CHECKLIST Phase PE (PE.001–PE.006); GAP-CLOSURE §2.6.
**Status:** Normative. This document fixes the demand-path surface, the fingerprint definition, and the memo contract.

---

## 1. `eval --path` (PE.001)

`active eval --from doc.active --path server.host` evaluates the whole
document under the full budget and profile, then emits only the
demanded cone (dotted/index path language shared with `@patch`:
`a.b[0].c`). Unknown or malformed paths fail closed naming the path.

Semantics, stated precisely:

- The budget accountant protects the WHOLE graph first. Projection
  runs after a complete evaluation; a pathological undemanded subtree
  still trips its budget instead of hiding behind the cone.
- The cone selects what gets EMITTED. Blob payloads follow the usual
  materialisation flag: without `--materialise-blobs` every blob
  stays an identity, so undemanded subtrees serialize small by
  construction.
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
4. Budget configuration (all seven limits),
5. `materialise_blobs` flag,
6. Mode (reserved: joins the token when Phase M lands Data Mode).

Canonicalisation: sources hash as given (no normalization — different
bytes are different documents by the one-file→one-meaning rule);
environment maps hash sorted by key; budgets hash by value. Any
configuration change clears the store before the walk, so a flag flip
can never serve a stale identity-only tree as materialised.

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
errors), budget-still-guards-cone, memo hit/miss telemetry,
materialise-flag cache separation, blob payload charging per alias.
The shared Tier-1 harness needs no changes (projection is output
selection over the identical evaluated tree it already verifies).

**End of partial-eval.md — PE.001–PE.006 complete.**
