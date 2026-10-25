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
  parse = { resilient = false, allow_parent_escapes = false },
  nodes = 42,
  diagnostics = ["T004@host"],
  result = "sha256:<hex of canonical redacted JSON>",
  outcome = "clean",
}
```

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
- `parse`: `{ resilient, allow_parent_escapes }`, the parse/evaluation
  mode the audited run used. Matching these on replay is part of the
  contract.
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
replay and review: same source + budget + profile + mode ⇒ same
content hash. The fingerprint memo (PE.003) and the audit record
agree by construction (same redaction, same canonical form).

**End of audit-trail.md — AUD.001 complete.**
