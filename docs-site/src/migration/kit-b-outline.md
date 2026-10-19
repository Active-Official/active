# Migration Kit B outline: feature flags and multi-environment overlays

Full write lands in Phase 1 (L-011). This outline freezes the shape so the
complete kit follows it exactly.

## Story

Before: one YAML file per environment plus brittle anchors, or a flags
service with duplicated stanzas. A flag flipped in staging but not prod and
nobody could diff intent. After: one Active source of truth with profiles
per environment and structural patches per override; `active impact` shows
which keys a change touches before it ships.

## Files (to be written)

- `kit-b-before-staging.yaml`, `kit-b-before-prod.yaml` (duplicated flags,
  one drifted value planted and documented)
- `kit-b-after.active` (single source: `[flags]` table plus
  `profile "staging"` / `profile "prod"` blocks; must eval clean
  hermetically under docs_site rules)
- This file becomes `kit-b-feature-flags.md` with: side-by-side diffs,
  `active check` / `active eval` / `active impact` commands, the planted
  drift presented as the worked mistake (expected diagnostic or impact
  output quoted verbatim from the binary), silent-to-explicit checklist
  (anchor merge silently wins vs patch conflicts error E019; duplicated
  files drift vs single source with profiles), differential mode section.

## Acceptance (L-011)

Stranger migrates the two YAML files to the Active source using only this
kit; docs_site suite stays green; mdbook builds.
