# Schema Evolution & Compatibility (normative)

**Version:** 1.0
**Back-reference:** PROPOSAL §2.4; CHECKLIST Phase SC (SC.005–SC.007); GAP-CLOSURE §2.7.
**Status:** Normative. This document fixes the metadata syntax, the compatibility semantics, and the C04x/E040 diagnostic family.

---

## 1. Metadata syntax (SC.005)

Reserved string keys inside the schema block declare the document's contract:

```active
schema = {
  version = "1.2.0",
  min_reader = "1.0.0",
  min_writer = "1.0.0",
  host = String,
  port = Int,
}
```

- `version`: this document's schema version (`major.minor.patch`).
- `min_reader`: oldest reader that can still process it.
- `min_writer`: oldest writer whose output it still accepts.
- All three are optional; string literals must be `major.minor.patch`
  with all-numeric parts — anything else is E020. A type expression
  under the same name (e.g. `version = String(...)`) is an ordinary
  field, so existing schemas keep working.
- First wins on repetition, like every other schema argument.
- Includes: metadata follows local-wins — absent local keys inherit the included document's values.

Per-field removal schedule:

```active
schema = {
  old_api = String(removed_in = "1.3.0"),
}
```

- `removed_in` takes a version string; non-strings and malformed versions are E020. It renders back through `active fmt` and the inspector.

## 2. Compatibility semantics (SC.006)

`active compatibility old.active new.active` compares the two schema blocks:

| Change | Code | Severity | Rationale |
|---|---|---|---|
| Added field without default (and non-Null) | C040 | error | Old documents fail the new schema |
| Added field with default or Null type | C040 | info | Old documents still validate |
| Removed field | C041 | error | Old readers/writers lose data |
| Removed field with `removed_in` ≤ new version | C041 | info | Removal was scheduled |
| Type changed | C042 | error | Existing values may fail |
| Field became required (default removed) | C042 | error | Old documents omitting the field fail |
| Blob `max_size` lowered or newly pinned | C042 | error | Existing payloads may fail |
| Blob `media_type` changed or newly pinned | C042 | error | Existing payloads mismatch |
| Array element type/field/bounds changed | C041/C042 | error | Compared at `path[]` (nested arrays at `path[][]`) |
| New `version` < old `version` | C043 | error | Versions only move forward |
| New `min_reader` > old `version` | C044 | error | Old readers are dropped |
| New `min_writer` > old `version` | C044 | error | Old writers are dropped |

Findings are deterministic (source order; numeric version compare) and value-free (type names and versions only). Any error-severity finding fails the command closed with E040.

## 3. Diagnostic family (SC.007)

C040–C044 findings plus the E040 breaking-change summary. Canned guidance lives in `explain_code`; the codes are pinned in `docs/diagnostics.md` and the assist coverage test. Unknown-reader situations (unparseable versions) fail safe: scheduled-removal requires both sides to parse, otherwise the removal is breaking.

**End of schema-evolution.md — SC.005/SC.006/SC.007 complete.**
