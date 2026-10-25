# Security hardening record (review rounds 12-71)

This record summarizes the independent review loop that hardened the
Master Program tree before release. It complements
`spec/secrets.md`, `spec/blob.md`, and the completion record.

## Method

- Reviewer: DeepSeek V4.1 Flash through the Copilot CLI BYOK route
  (CommandCode), `--reasoning-effort max`. The Copilot subscription
  quota was exhausted for the review window; the routing is documented
  in the review skill and never carries credentials into the repo.
- Loop: after every fix round the reviewer re-ran, confirmed the previous
  findings, and searched for new ones. Each real finding was reproduced,
  fixed at the cause, and pinned with a regression test that drives the
  shipped path (CLI command, exporter, inspector, trace, FFI/WASM entry).
- Gates after every round: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, plus the wasm target check.
- Final verdict and log: `conformance/evidence/review-final.log`.

## Finding classes and resolutions

| Class | Examples | Resolution |
|---|---|---|
| Secret leakage through exports | env-backed copies, profile-selected values, profile-authored literals, `let`-bound computed secrets, blob payload bytes, provenance env values | one redactor seeded from authored literals, the evaluator's material node values, profile-authored literals, and material env values; every exporter/inspector/trace/lock/audit absorbs it before rendering |
| Secret leakage through tools | inspect wrappers, debug steps, LSP hover, graph/viz taint, audit/replay hashes, reduction bundles | identity-only step and inspect rendering, path-aware hover, seeded taint queries, canonical hashing, evaluator-seeded duplicate/comment redaction |
| Availability | quadratic redaction, work-budget over-redaction, per-alias payload charging, trusted-budget passes | Aho-Corasick batches with match cursors, per-batch retirement, a taint probe that skips alias copy accounting, caller-honoring budgets |
| Fidelity | longest-at-equal-start semantics, marker opacity, prefix preservation, UTF-8 | differential tests against a naive leftmost-longest reference, marker-split scanning, cursor reuse |
| Contract | `tainted` flags, audit/replay/lock agreement, error codes | flags derived from the same decision as the value; one canonicaliser for audit, lock, and replay |
| Bounds and determinism | fail-open matcher builds, profile truncation, zero/unlimited limits, i64 document limits | linear fallback, fail-closed marking, per-field limit validation, deterministic fold/batch state |

## Late rounds (60-71)

The tail of the loop moved from "a value reaches an output" to "the model
is consistent and bounded on every transform":

- Post-transform profile seeding: the node at each material path is
  resolved after every profile and patch, with cycle-safe memoization and
  deterministic root order; a computed selector seeds its maintained path
  records so a sibling patch cannot erase a surviving value.
- Visibility: export visibility is filtered by the evaluated export paths
  (not binding names or failed Sets); a computed selector drops every path
  a possible profile could override.
- Exact vs substring: whole-value matching at any length, substring
  spreading at four bytes and up, any-length authored-prose redaction.
  The exact set reaches export, inspect, T006, constraint sources, docgen,
  LSP, blob identity/payload, JSON redaction, and debug duplicates.
- Diagnostics: B004/T003/T004, T006 sources and actuals, L005 reasons, and
  parse-time E0xx messages are value-free or redacted with the engine
  environment; T004 keeps a resolvable `requires` target readable.
- Generators: enum members, defaults, patterns, formats, and media types
  are withheld from executable slots when material rather than replaced
  with a marker, so generated artifacts stay valid.
- Planning and repair: one analyzed profile per pass (linear), built from
  the engine's profile override and environment; a fill that would copy
  material is withheld, and directives keep their structural paths.
- Debugger: malformed input is covered by literal-island runs,
  split-interpolation elision, malformed-string slices, bare-token runs,
  inter-token gaps, and a fail-closed sweep, while canonical env names and
  structural literals stay reproducible.
- Availability: iterative toposort replaced a recursive cycle check,
  interpolation nesting charges the depth budget, and long alias chains
  fail closed at fixed work/byte/depth bounds instead of overflowing the
  stack.

The local working record with the full round-by-round reasoning is
`master-program/HARDENING-REVIEWS-12-71.md` (gitignored, never pushed).

## Invariants held

- No raw secret or `payload_b64` reaches an output surface in any tested
  path; reduction also redacts duplicate values, comments, and
  base64/hex encodings of a known secret. The only payload emitter is
  the explicit `eval --materialise-blobs` path, which refuses a
  secret-bearing payload with B003.
- Redaction is linear per pattern batch and never marks a benign value
  wholesale.
- A bounds trip fails closed (marker), never open.
- Repeated exports are byte-identical; audit, lock, and replay hash the
  same canonical tree.

## Evidence

- `conformance/evidence/review-final.log` - final verification round
  (71) verdict: `CLEAN`, over the complete goal diff.
- `conformance/evidence/gates.log` - Rust gates on the final tree.
- `active-core/tests/secrets.rs`, `inspect.rs`, `debug.rs`, `blob.rs`,
  `graph_queries.rs`, `viz.rs`; `active-cli/tests/cli.rs`;
  `active-lsp/tests/tooling.rs` - the regression tests added by the loop.
