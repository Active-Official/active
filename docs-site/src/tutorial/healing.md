# Healing and modes

The healer repairs in three ordered phases with a published
confidence policy: ≥ 0.95 auto-applies (silently only for safe
syntactic repairs — key renames always warn), 0.85–0.94 applies
with a warning, 0.70–0.84 applies only under resilient mode, below
0.70 never.

1. **Syntactic** — unclosed strings, missing/trailing commas,
   mismatched delimiters (`H001`–`H004`; the safest apply even in
   strict mode as Info, and strict still exits non-zero on
   warnings).
2. **Semantic keys** — unknown keys matched against schema fields
   with Levenshtein + phonetic + keyboard scoring: suggestions from
   0.70 up, renames at 0.85+ under resilient mode only (`H005`).
3. **Types** — safe string coercions (`"5s"` → duration) under
   resilient mode (`H007`); sentinel words (`"unlimited"`) are
   recognized by intent in strict messages but never mapped — the
   schema language designates no sentinel values. Schema defaults
   fill missing keys (`W005`).

Every repair is a structured diagnostic with the original span, the
repaired text, a confidence score, and a safe/auto flag that editors
(and the LSP `apply healing` action) consume. Strict mode reports
suggestions without mutating the tree; resilient mode heals and
tells you exactly what it changed.
