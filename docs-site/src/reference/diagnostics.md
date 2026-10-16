# Diagnostics index

Every diagnostic carries a stable code, a precise span, an
actionable message, and (for repairs) a confidence score. The
living catalog is `docs/diagnostics.md`; the bands:

- **`E0xx` syntax** — the file cannot parse (unclosed strings,
  bad indentation, bad headers). Fix the text.
- **`S0xx` semantic** — parses but cannot mean (cycles `S001`,
  unresolved names, bad includes). Restructure.
- **`H0xx` healing** — repairs applied or proposed, each with
  confidence: syntactic (`H001`–`H004`, the safest apply even in
  strict as Info — and strict still exits non-zero on warnings),
  key repair (`H005`, always a warning when it renames), coercion
  (`H007`). Strict attaches suggestions without mutating;
  resilient applies + tells.
- **`T0xx` type/constraint** — schema violations after evaluation
  (`T002` type, `T003` constraint, `T004` missing, `T005`
  unexpected key). Fix the value or the schema.
- **`W0xx` warnings** — profiles, fills, deprecations. Collected
  alongside success in resilient mode; any warning fails strict.
- **`X0xx` budgets** — node count, wall-clock, depth, array length,
  key totals exceeded. Always fatal, always first.

Confidence policy: ≥ 0.95 auto-applies (silently only for safe
syntactic repairs — renames always warn), 0.85–0.94 applies
with a warning, 0.70–0.84 applies only under resilient mode, below
0.70 never. Exit codes follow severity: any Error exits 1, in both
modes (resilient still fails on errors — it collects warnings and
succeeds around them, but violations stay errors).
