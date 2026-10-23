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
  nodes = 42,
  diagnostics = ["T004@host"],
  result = "sha256:<hex of canonical redacted JSON>",
  outcome = "clean",
}
```

- `nodes`: IR arena size (evaluation scale proxy).
- `diagnostics`: sorted, deduped `CODE@path` items (bare codes for
  fatal loop-terminal errors, same convention as the repair plan's
  `final_codes`).
- `result`: `content_hash` of the evaluated value — computed over the
  canonical redacted form, so raw secrets can never enter the log.
- `outcome`: `clean` or `failed` (mirrors the process exit contract).

## 2. Properties

- **Re-validatable:** the audit file passes `active check` cleanly
  (no trailing commas, no unbound markers).
- **Value-free:** only counts, codes, hashes, and metadata cross the
  boundary. Payload and secret bytes never appear (enforced by the
  failing-if-leaking audit test).
- **Available on failure too:** validation failures still emit the
  audit with `outcome = "failed"` and the collected codes. Fatal
  engine errors (no value produced) carry no audit trail.

## 3. Agent use

Agents archive one audit file per consequential evaluation for
replay and review: same source + budget + profile + mode ⇒ same
content hash. The fingerprint memo (PE.003) and the audit record
agree by construction (same redaction, same canonical form).

**End of audit-trail.md — AUD.001 complete.**
