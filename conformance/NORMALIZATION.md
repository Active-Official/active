# Conformance Normalization Rules (normative, versioned)

**Version:** 1.0
**Back-reference:** PROPOSAL §2.3; CHECKLIST Step B.001
**Enforced by:** `conformance/harness/diff_vectors.py` (differential gate),
`conformance/harness/run.py` (`verify_parity`, `classify`, `finalize_ok`),
and `conformance/harness/test_run.py` (23 unit tests pinning the rules).

Two reports match if and only if they agree under every rule below. Any
divergence fails the gate; the harness exits nonzero and CI fails the PR.
On spans: strict equality holds **inside the differential gate**, which
compares structured spans tuple-for-tuple across the C ABI, Python, and
JS embeddings. The reference manifest suite (`conformance.rs`) still
records spans advisory per G-017 and asserts codes plus values; the
differential gate is the stricter superset, and Tier-2 smoke runners
assert codes plus values only. No path anywhere treats span *drift*
as a pass: where spans are surfaced they must match exactly.

## 1. Diagnostic codes

- Each embedding reports an **ordered list** of codes, one per diagnostic
  item, in core emission order. Recovery can legitimately emit one code
  twice (e.g. E004 for depth on open and close brackets); duplicates are
  significant and must not be collapsed when comparing embeddings.
- Against the committed expectations (`conformance/errors/<id>.json`),
  codes compare as a **set view**: sorted and deduplicated. This matches
  the reference core suite's parse-refusal branch (`conformance.rs`,
  which sorts and dedups there; other branches compare ordered and
  unnormalized) and keeps historical expectations stable. Expectation
  files are stored sorted by convention; the differential normalizes
  only the live side.
- Rule: cross-embedding equality is per-item and ordered; expectation
  equality is sorted-set. A mismatch on either axis fails the run.

## 2. Span tuples

- Each diagnostic item carries a span as `[line, column, end_line,
  end_column]` (1-based, inclusive start, exclusive end) or `null` when
  the core left it spanless (e.g. S001 cycles).
- Spans compare **tuple-for-tuple, in order**, across all embeddings.
  `null` equals only `null`; a span never equals `null`.
- Spanless-vs-spanned drift is a failure, not an advisory note.

## 3. Severities

- Compared as the core severity strings: `"error"`, `"warning"`,
  `"info"`, `"hint"`, in item order, across all embeddings.

## 4. Refused flag and values

- `refused` is a boolean per case and must agree across embeddings.
- When refused, no embedding may return a value.
- When not refused, redacted JSON values must be deeply equal
  (key-order-insensitive object comparison). Redaction happens in core
  before comparison; the independent guarantee is the post-compare blob
  scan (§6), which would catch any raw marker the embeddings agreed on.
- A refused case with zero codes, or a clean case with codes, fails.

## 5. Expectations and pins

- Expected codes live in `conformance/errors/<id>.json`; expected values
  in `conformance/expected/<id>.json`. Missing expectation files mean
  "no codes" and "no value" respectively — never a free pass on values
  the embeddings disagree about.
- `[[pins]]` in `conformance/manifest.toml` lists cases no shipped
  binding surface can exercise (include-loader registration, CLI
  `--profile` override). Pinned cases run in the reference suite only;
  the differential logs the pin with its reason instead of comparing.

## 6. Secrets (fail-closed)

- The raw fixture marker must appear in **no** captured output on any
  path. The scan runs on raw output *before* redaction-scrubbing, so a
  scrubbed log cannot hide a leak. Any hit fails the whole run,
  including on paths that would otherwise count as skipped.

## 7. SKIP, timeouts, missing binaries

- `SKIP` counts as skipped only on a clean (exit 0) run with no leak.
  A SKIP line plus a nonzero exit is a failure, never a silent pass.
- A missing toolchain binary (exit 127, no SKIP line) counts as skipped
  openly; Tier-1 paths must still pass (see §8).
- Any timeout is a failure, even when the partial output happens to
  match a tally. Outer path invocations budget 900 s in the harness
  runner; the differential gate's inner Node probe budgets 300 s.
  Either expiring fails the run.

## 8. Tier-1 must-pass rule

- The reference, C ABI, WASM, Go, Python, JS/TS, go-native, and
  differential paths must report `pass`. A skipped or missing Tier-1
  path fails the run: there is no parity proof without it.
- Tier-2 smoke paths may skip when their toolchain is absent and execute
  when present; a Tier-2 failure still fails the run.

## 9. Versioning this document

- Bump `Version` above on any rule change and record the reason in the
  commit message. Rules are code-reviewed like code: the differential
  gate, the harness unit tests, and this document must agree, and any
  of the three drifting fails review.
