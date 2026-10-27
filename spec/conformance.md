# Conformance Suite & Reference Implementation (C-063, P-062)

## 1. Declaration

The Rust implementation in this repository
(`Active-Official/active`, workspace root) is declared the
**reference implementation for the 1.x conformance suite**: the
compatibility target every other implementation proves against.
The 1.0 declaration is recorded in `spec/readiness-1.0.md`; later
disputes use the RFC process (docs/governance.md).

## 2. The suites

Two suites serve different jobs and must both stay green:

- `tests/spec/` is the **value corpus**: 13 pairs of
  `<name>.active` + `<name>.expected.json` — operator precedence (01),
  unit normalization (02), interpolation (03, 12), env defaults
  (04), secret redaction (05), sibling resolution (06), arrays of
  tables (07), duration arithmetic (08), parser resilience (09 —
  pins the §4.1 statement-level skip quirks of spec/grammar.md),
  dual-syntax brace/indent equivalence (10–11), and multi-line
  strings (13). Each pair asserts an ACCEPTED document's evaluated
  value tree, byte for byte.
- `conformance/manifest.toml` is the **cross-embedding suite**
  (21 cases today, including negative and host-feature pins)
  driven by `conformance/harness/run.py` across the reference,
  C ABI, WASM, and binding paths. Its normalization contract is
  `conformance/NORMALIZATION.md`; the differential gate
  (`conformance/harness/diff_vectors.py`) is the strictest proof and
  runs 19 of the 21 (cases 0008 and 0011 are pinned because their
  include/profile host paths are not portable). Blob (0017-0018),
  mode (0019-0020), and capability-shape (0021) vectors carry the
  newer surfaces through the same gate.

### 2.1 Harness contract (what "passing" means)

The reference runner is `spec_corpus_matches_expected_values`
(`active-core/tests/corpus.rs`); an alternative implementation
passes by reproducing it exactly:

1. Parse `<name>.active` (default options, filename as source name).
2. Evaluate with a default engine on an empty env snapshot
   (resilient mode, empty profile override, shipped budgets):
   hermetic by construction, so ambient values cannot leak into
   conformance results on any machine. Cases needing environment
   use `?:` defaults.
3. Assert zero diagnostics. (Diagnostics-code conformance is
   explicitly out of scope for 1.0: codes are pinned by the
   workspace unit tests, not by sidecars. Negative documents are
   covered by `tests/negative/`, which asserts rejection without
   pinning exact trees.)
4. Serialize the value tree with the canonical JSON mapping below
   and compare exact bytes after CRLF normalization and trimming
   (the reference runner trims one trailing newline; sidecars are
   stored that way).

### 2.2 Canonical JSON mapping

- Tables → objects in **source binding order** (order is
  significant: implementations must preserve it, not sort).
- Integers → bare JSON numbers; floats use serde_json's shortest
  round-trip form (integral floats keep their `.0`).
- Strings → JSON strings with standard escaping; durations and
  quantities normalize to their value objects per the converter
  contract (spec/converters.md).
- Secrets → the exact string `"***SECRET***"` (audited: raw values
  never appear).
- Blobs → `{"$active.blob": {hash, size, media_type, tainted}}`
  (spec/blob.md §5); a tainted identity redacts `media_type`, and
  `payload_b64` appears only under an explicit materialisation.
- Arrays → JSON arrays in order; null → `null`; booleans as-is.

### 2.3 Suite versioning

The suite versions with the language (same workspace version).
Adding cases is additive and always allowed; changing an expected
tree requires a minor/major version and a changelog entry, because
it redefines the language. Cases never test unspecified behavior —
if two reasonable implementations could differ, the spec (not the
sidecar) decides, and the case is rewritten or dropped.

## 3. Running it

```bash
cargo test -p active-core --test corpus spec_corpus
cargo test -p active-core --test conformance   # suite metadata + determinism pins
```
`active-core/tests/conformance.rs` additionally pins: every `.active` has a
parseable `.expected.json` sidecar (no orphans either direction),
the declaration in §1 is present in this file, and every case
evaluates byte-identically twice in a row (determinism is part of
conformance).
