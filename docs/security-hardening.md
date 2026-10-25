# Security hardening record (review rounds 12-54)

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
| Secret leakage through exports | env-backed copies, profile-selected values, `let`-bound computed secrets, blob payload bytes, provenance env values | one redactor seeded from authored literals, the evaluator's material node values, and material env values; every exporter/inspector/trace/lock/audit absorbs it before rendering |
| Secret leakage through tools | inspect wrappers, debug steps, LSP hover, graph/viz taint, audit/replay hashes | identity-only step and inspect rendering, path-aware hover, seeded taint queries, canonical hashing |
| Availability | quadratic redaction, work-budget over-redaction, per-alias payload charging, trusted-budget passes | Aho-Corasick batches with match cursors, per-batch retirement, a taint probe that skips alias copy accounting, caller-honoring budgets |
| Fidelity | longest-at-equal-start semantics, marker opacity, prefix preservation, UTF-8 | differential tests against a naive leftmost-longest reference, marker-split scanning, cursor reuse |
| Contract | `tainted` flags, audit/replay/lock agreement, error codes | flags derived from the same decision as the value; one canonicaliser for audit, lock, and replay |
| Bounds and determinism | fail-open matcher builds, profile truncation, zero/unlimited limits, i64 document limits | linear fallback, fail-closed marking, per-field limit validation, deterministic fold/batch state |

## Invariants held

- No raw secret or `payload_b64` reaches an output surface; the only
  payload emitter is the explicit `eval --materialise-blobs` path, which
  refuses a secret-bearing payload with B003.
- Redaction is linear per pattern batch and never marks a benign value
  wholesale.
- A bounds trip fails closed (marker), never open.
- Repeated exports are byte-identical; audit, lock, and replay hash the
  same canonical tree.

## Evidence

- `conformance/evidence/review-final.log` - final `CONFIRMED CLEAN`
  review over the complete goal diff.
- `conformance/evidence/gates.log` - Rust gates on the final tree.
- `active-core/tests/secrets.rs`, `inspect.rs`, `debug.rs`, `blob.rs`,
  `graph_queries.rs`, `viz.rs`; `active-cli/tests/cli.rs`;
  `active-lsp/tests/tooling.rs` - the regression tests added by the loop.
