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
- `min_reader` is enforced at read time: a reader refuses a document whose
  `min_reader` exceeds the reader's version (T007; a host may declare its
  reader version with `Engine::reader_version` / `--reader-version`,
  otherwise the running library is the reader).
- `min_writer`: oldest writer whose output it still accepts.
- All three are optional; string literals must be `major.minor.patch`
  with all-numeric parts — anything else is E020. A type expression
  under the same name (e.g. `version = String(...)`) is an ordinary
  field, so existing schemas keep working.
- First wins on repetition, like every other schema argument.
- Includes: metadata follows local-wins — absent local keys inherit the included document's values.

**Reader floors are enforced (LANG-B-021; the earlier advisory-only default
was superseded).** A host that declares its reader version
(`--reader-version` on the CLI, `Engine.reader_version` in the library)
gets a hard refusal: a document whose `min_reader` is above it fails with
**T007** before any evaluation work. A host that declares nothing *is* the
running library, so the library's own version is the reader version and the
same refusal applies. The floors are also a publication-time contract
checked by `active compatibility` against a peer's `version`. The C ABI v1
carries no explicit host version, so an FFI host evaluates with the running
library as its reader; a host that embeds newer semantics should pin a
compatible library version until the ABI grows an explicit reader-version
setter.


Per-field removal schedule:

```active
schema = {
  old_api = String(removed_in = "1.3.0"),
}
```

- `removed_in` takes a version string; non-strings and malformed versions are E020. It renders back through `active fmt` and the inspector.
- A field whose `removed_in` version is less than or equal to the document
  schema's own `version` and which is still present in the document is
  REJECTED on the evaluation path (T008), not only in the offline
  `active compatibility` diff (LANG-F-008): a removal schedule that exists
  only in a planning tool cannot retire fields from the wire.

## 2. Compatibility semantics (SC.006)

`active compatibility old.active new.active` compares the two schema blocks:

| Change | Code | Severity | Rationale |
|---|---|---|---|
| Added field without default (and non-Null) | C040 | error | Old documents fail the new schema |
| Added field with default or Null type | C040 | info | Old documents still validate under resilient evaluation; strict mode still requires the value explicitly (T004) |
| Removed field | C041 | error | Old readers/writers lose data |
| Removed field with `removed_in` ≤ new version | C041 | info | Removal was scheduled |
| Type changed | C042 | error | Existing values may fail |
| Field became required (default removed) | C042 | error | Old documents omitting the field fail |
| Blob `max_size` lowered or newly pinned | C042 | error | Existing payloads may fail |
| Blob `media_type` changed or newly pinned | C042 | error | Existing payloads mismatch |
| Array element type/field/bounds changed | C041/C042 | error | Compared at `path[]` (nested arrays at `path[][]`) |
| Disjunction branch removed | C042 | error | Narrows the accepted set: an old value that used the branch fails (L-P10) |
| Disjunction branch added | - | - | Widens the accepted set; branch order is not a change (branches compare as a sorted set) |
| Disjunction or leaf default changed (L-P10 entries) | C042 | error | The default is part of the contract; ordinary-field defaults keep their existing required/default finding instead of a duplicate |
| New `version` < old `version` | C043 | error | Versions only move forward |
| New `min_reader` > old `version` | C044 | error | Old readers are dropped |
| New `min_writer` > old `version` | C044 | error | Old writers are dropped |
| Root-schema `check` rule added, removed, or changed (compared as authored text; whitespace-only edits also report) | C045 | error | Cross-field behavior changes; documents that passed may now fail |
| Table `closed` flag: open → closed | C046 | error | Keys outside the declared set now fail T005; documents that added them fail the new schema (L-P11) |
| Table `closed` flag: closed → open | C046 | error | The "no extra keys" promise is withdrawn; readers that relied on the closed key set can be misled even though the accepted value set grows (L-P11) |
| Root-schema `closed` flag changed (either direction) | C046 | error | The same promise change applies to the document's top-level keys; the finding path is `<root>` |
| Disjunction branch table `closed` flag changed (either direction), including tables nested inside a branch | C046 | error | The same promise change inside a `OneOf` branch; the branch is paired with its counterpart (closedness is not a branch removal), and the message names the branch by an order-independent type descriptor |


**Closing and opening are both breaking (L-P11).** Preference for closing is
straightforward: the new schema rejects keys the old one accepted. Opening
is the recorded trap: the accepted value set grows, so it looks like a
loosening, but `closed = true` is a promise consumers may switch on. A
reader that assumed the key set was exhaustive becomes unsound when the
promise is withdrawn. Compatibility therefore reports both directions as
error severity and never as silent, and the disjunction widening rule
(a removed branch is compatible when a new branch strictly widens it)
requires the closed flag to be equal: a widening that adds or withdraws a
promise is not a widening.

The scope is every table reachable as a schema row, the document root, an
array element table, or a disjunction branch (including tables nested
inside a branch, even under a nested disjunction) - at any depth. A branch
that also changes a constraint reports C046 and C042 independently, so a
simultaneous tightening never hides the promise change.

Findings are deterministic (source order; numeric version compare) and value-free (type names and versions only). Any error-severity finding fails the command closed with E040.

`active diff old.active new.active` (L-P52) assembles the whole review
picture from the same engine: the semantic value diff (added, removed,
and changed paths with both type names), these compatibility findings,
and the dependents of every changed path, resolved to its longest
resolvable dotted prefix (bounded, with `impact_truncated` when the cap
is hit). The comparison runs on the
redacted trees: material values are never compared, so `behaviour:
identical` means every compared value is equal and a secret or blob
path always reports changed with a `***SECRET***` echo; a
diff can never confirm a guess. The two documents' material sets are
unioned before redaction, so a value that only one side classifies as
secret is redacted on both. Paths are redacted segment by segment.
Both documents parse and evaluate strictly: any error or warning
refuses with its stable codes and a nonzero exit (the `check`/`eval`
convention). Schema findings do not gate; `active compatibility` remains
the fail-closed schema check. Its JSON shape is
pinned by `docs/schemas/diff.json`; as a CLI tooling surface it carries
CLI and output-schema coverage rather than a cross-embedding vector.

## 3. Diagnostic family (SC.007)

C040–C046 findings plus the E040 breaking-change summary. Canned guidance lives in `explain_code`; the codes are pinned in `docs/diagnostics.md` and the assist coverage test. Unknown-reader situations (unparseable versions) fail safe: scheduled-removal requires both sides to parse, otherwise the removal is breaking.

**End of schema-evolution.md — SC.005/SC.006/SC.007 complete.**
