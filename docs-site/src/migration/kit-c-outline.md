# Migration Kit C outline: AI agent and system-prompt configuration

Full write lands in Phase 1 (L-011). This outline freezes the shape so the
complete kit follows it exactly.

## Story

Before: a JSON blob (or Markdown with ad-hoc structure) holding model id,
temperature, tool allow-lists, and a secret someone pasted. No schema, no
immutability, redaction by luck. After: Active with a schema (temperature
bounds, tool enum), `@ai_immutable` on the model pin, secret redaction by
construction, and a repair loop the agent runs itself.

## Files (to be written)

- `kit-c-before.json` (untyped blob with one out-of-range temperature and
  one cleartext key, both planted and documented)
- `kit-c-after.active` (schema + `@intent` + `@constraint` + `@ai_immutable`
  + `secret()`; must eval clean hermetically under docs_site rules)
- This file becomes `kit-c-agent-config.md` with: side-by-side files, the
  agent repair loop worked end to end (generate, check, repair by code plus
  span, re-eval) with every diagnostic quoted verbatim from the binary,
  silent-to-explicit checklist (untyped blob vs closed schema; pasted secret
  vs taint; hand-edited pin vs `@ai_immutable`), differential mode section.

## Acceptance (L-011)

Stranger migrates the JSON blob using only this kit; an agent loop repair
demo runs green from the documented commands; docs_site suite stays green.
