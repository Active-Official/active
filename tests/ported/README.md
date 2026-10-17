# Ported external corpora

External TOML / YAML / JSON test suites, adapted for Active. Every case
drives the SHIPPED importer (`toml_text_to_active`, `json_text_to_active`,
`yaml_text_to_active`) plus the real parser and evaluator through
`active-core/tests/ported.rs` — no re-implemented logic. Each case has one
expectation sidecar: `<stem>.expected.json` (Active canonical value tree)
or `<stem>.expected-diag.txt` (the exact pipeline stage that rejects it:
`IMPORT_HARD_ERROR`, `PARSE_ERROR`, `EVAL_ERROR`). Cases whose documented
behavior includes eval warnings carry `<stem>.expected-warnings.json`
(the exact eval-diag code multiset; absent means clean — e.g. W011 on
sanitization collisions).

## Suites

| Suite | Origin | License | Cases |
|---|---|---|---|
| `toml/` | BurntSushi/toml-test@ff49d10 | MIT, (c) 2018 TOML authors | 266 |
| `json/` | nst/JSONTestSuite@1ef36fa | MIT, (c) 2016 Nicolas Seriot | 318 (293 fixtures + 25 skips) |
| `yaml/` | yaml/yaml-test-suite@da267a5 | MIT, (c) 2016-2020 Ingy döt Net | 406 doc bodies |

TOML sources keep their relative paths flattened (`array/nested.toml` →
`array_nested.toml`) with `#` origin headers (valid TOML comments). JSON
sources are verbatim (JSON has no comments; origin lives here and in
`manifest.json`). YAML sources are the suite's document bodies with `#`
origin headers.

## Delta classes (`manifest.json` per case)

- `exact` (175 TOML, 93 JSON): Active agrees with the external oracle
  byte-for-byte after canonicalization.
- `adapted` / `adapted-active-semantics` (79 TOML, 2 JSON): Active semantics
  differ by design — W007 key sanitization (`-` → `_`, leading digits gain
  `_`, empty keys become `key`), W008 datetime stringification, W010
  non-finite→null. The committed `.expected.json` is the shipped pipeline's
  output, reviewed against the external oracle during porting.
- `active-import-semantics-pinned` (227 YAML): YAML document bodies with
  committed Active outputs (anchors/aliases resolve, timestamps stringify).
- `impl-defined-behavior-pinned` / `impl-defined-rejection-pinned` (5 + 17
  JSON `i_*`): JSONTestSuite's implementation-defined cases — Active's
  deterministic choice, pinned (huge exponents reject, underflows become
  `0.0`, lone surrogates hard-error, 500-deep nesting hits the recursion
  limit, BOM-prefixed docs reject).
- `reject-invalid-json-hard-error` (176 JSON `n_*`): malformed JSON the
  importer must refuse.
- `rejected-with-diagnostic` (12 TOML, 179 YAML): valid-in-source documents
  Active rejects with a recorded stage — TOML 1.1 features the vendored
  parser lacks (inline-table newlines), over-long integers (`E023`), YAML
  tags/directives serde_yaml refuses. Each names its stage.
- `skip-non-utf8-bytes-unrepresentable-as-str` (25 JSON): files that are not
  valid UTF-8. Rust `&str` is UTF-8 by construction, so these bytes cannot
  reach the converter through any public API — a principled skip, not a gap.

## Findings the ports already paid for

- Integer literal overflow evaluated to silent `0` (E023 + saturation fix,
  `active-core/tests/regressions.rs`).
- Float exponents emitted unparseable Debug scientific notation
  (`active-core/tests/convert.rs::exponent_floats_emit_parseable_plain_decimals`).
