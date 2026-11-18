# Audit Trail as Active Data (normative)

**Version:** 1.0
**Back-reference:** PROPOSAL §5.1; CHECKLIST AUD.001; GAP-CLOSURE §2.5.
**Status:** Normative. This document fixes the audit emission surface.

---

## 1. Emission

`active eval <file> --audit-trail <out.active>` writes a secret-redacted
evaluation log that is itself a valid Active document:

```active
audit = {
  tool = "active",
  version = "1.4.0",
  source = "svc.active",
  mode = "config",
  profile = "production",
  budget = { nodes = 100000, depth = 256, array_len = 100000, keys = 100000, wall_ms = 100, value_size = 10000000, total_size = 100000000 },
  parse = { resilient = false, allow_parent_escapes = false, materialise_blobs = false },
  include_resolutions = [{ path = "base.active", resolved = "base.active", sha256 = "sha256:<hex>", source = "lock-path" }],
  nodes = 42,
  diagnostics = ["T004@host"],
  result = "sha256:<hex of canonical redacted JSON>",
  outcome = "clean",
}
```

- Every evaluation ATTEMPT writes one when `--audit-trail` is requested,
  including failures that never produced a tree (LANG-F-010): a parse
  failure records the parse diagnostics and `failure_class = "parse"`; a
  budget trip records its carried warnings and `failure_class = "budget"`;
  a strict validation refusal records the validation diagnostics and
  `failure_class = "validation"`. `result` is `null` in all of them.
- `failure_class`: `parse` | `budget` | `validation` | `other`, present only
  on a failed attempt - it names which stage refused, so a dispute has
  evidence instead of an empty result.
- `peer`: optional sender identity supplied by the transport (`--peer`). It
  is omitted for local runs; a receiver records it so a rejected peer
  document is attributable.
- `nodes`: IR arena size (evaluation scale proxy).
- `mode`: the resolved mode (`config` or `data`), recorded after
  includes and the root marker are resolved.
- `profile`: the effective profile selector, when one applies —
  either the literal `active_profile` value or a `--profile`
  override. The field is omitted when no selector is active. It is
  recorded so `active debug --replay --profile <name> --expect
  <result>` can reproduce the hash.
- `budget`: the resolved resource limits `{ nodes, depth, array_len,
  keys, wall_ms, value_size, total_size }`. Replay must be given the
  same limits to reproduce `result`.
- `parse`: `{ resilient, allow_parent_escapes, materialise_blobs }`,
  the parse/evaluation mode the audited run used. Replay must match
  `resilient` and `allow_parent_escapes`; the hash itself is canonical
  across `materialise_blobs` (payloads are stripped).
- `include_resolutions`: one entry per resolved include (L-P03/L-P06),
  `{ path, resolved, sha256, source }`. `path` is the spelling the
  document used, `resolved` is the identity recorded when a lock was
  active (root-relative when the root directory is known, so a shareable
  receipt does not name the author's home directory; an include outside
  the root uses `..` segments and stays absolute only when the two share
  no root or the parse has no root), `sha256` is the digest of the bytes
  read, and `source` is `inline` | `lock-path` | `lock-content` |
  `unpinned`. The field is omitted when the document has no includes.
  It records what was verified, not merely what was read.
- Limits are stored as Active integers, so any limit above
  `i64::MAX` is refused before the audit is written (use `0` for
  unlimited).
- `diagnostics`: sorted, deduped `CODE@path` items (bare codes for
  fatal loop-terminal errors, same convention as the repair plan's
  `final_codes`).
- `result`: `content_hash` of the evaluated value — computed over the
  canonical redacted form (materialised, redacted, payload-free), so
  raw secrets can never enter the log and `--materialise-blobs` cannot
  change the identity. On a failed evaluation with no value, `result`
  is `null` (see §2).
- `outcome`: `clean` or `failed` (mirrors the process exit contract).

## 2. Properties

- **Re-validatable:** the audit file passes `active check` cleanly
  (no trailing commas, no unbound markers).
- **Value-free:** only counts, codes, hashes, and metadata cross the
  boundary. Payload and secret bytes never appear (enforced by the
  failing-if-leaking audit test).
- **Available on failure too:** validation failures still emit the
  audit with `outcome = "failed"` and the collected codes.
- **Null result on failure:** when strict validation fails before
  producing a value, the audit is still written with `outcome =
  "failed"` and `result = null`. `null` is a valid Active value, so the
  audit stays re-validatable; consumers must treat `null` as "no value
  was produced" and never as an empty document hash.
- **Fatal engine errors** (parse failures, budget trips, and any error
  that produces neither a value nor validation diagnostics) carry no
  audit trail.


## 3. Agent use

Agents archive one audit file per consequential evaluation for
replay and review: same source + budget + profile + mode + parse flags
⇒ same content hash. The fingerprint memo (PE.003) and the audit record
agree by construction (same redaction, same canonical form).

**End of audit-trail.md — AUD.001 complete.**
