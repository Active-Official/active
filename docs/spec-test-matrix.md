# Spec → test matrix (test-supremacy goal)

Every `spec/*.md` chapter mapped to the test targets that pin it. A chapter
with no entry is a gap — there are none; gaps closed by this goal are marked
★. Oracles (`conformance.rs` declaration pin, `governance.rs` advisory pin)
are never edited to match changes.

// markdownlint-disable MD013

| Spec chapter | Test targets |
|---|---|
| `specification.md` §1 values/grammar surface | `corpus.rs`, `dual_syntax.rs`, `unit_roundtrip.rs`, `ported.rs` (fmt fixed-point + determinism over ported docs) |
| `specification.md` §1.3 integer range (E023) | ★ `regressions.rs` (E023 error, saturation recovery, unary-minus boundary, clean MAX) |
| `specification.md` §3.3 saturating arithmetic | ★ `regressions.rs` (int saturation, MIN/-1, neg, byte/quantity saturation, huge durations) |
| `specification.md` interpolation (S004) | `interpolation.rs`, `secrets.rs`, `fuzz.rs::interpolation_never_panics`, `fuzz/corpus/interpolation/` seeds |
| `grammar.md` EBNF + §4.3 codes | `dual_syntax.rs`, `diagnostics_spans.rs`, `healing.rs`, `corpus.rs` (negative), ported rejection stages |
| `grammar.md` E023 row | ★ `regressions.rs` + `docs/diagnostics.md` catalog row |
| `dual-syntax.md` | `dual_syntax.rs`, `format_roundtrip.rs`, `ported.rs` (brace+indent round-trip) |
| `formatter.md` | `format_roundtrip.rs`, `redaction_paths.rs` (locator fidelity), `active-fmt/tests/cli.rs` |
| `linter.md` (L001–L004+) | `lint.rs` incl. ★ chained/nested-use guard for the flat scan |
| `validation.md` / schema (T-codes) | `validation.rs`, `schema.rs`, `secrets.rs` (T002/T003 bound silence) |
| `annotations.md` (@-codes, immutability) | `annotations.rs` (incl. immutable-patch rejection) |
| `converters.md` (W-codes, redaction) | `convert.rs` incl. ★ exponent emission + ★ excerpt withholding, `redaction_paths.rs`, `ported.rs` (all three importers) |
| `converters.md` generators | `generate.rs`, `docgen.rs` |
| `docgen.md` | `docgen.rs`, `redaction_paths.rs::docgen_all_formats_redact` |
| `graph.md` | `graph.rs` (name-only labels), `redaction.rs` (snapshot/impact/why), CLI graph goldens |
| `budgets.md` (X-codes) | `budgets.rs`, `incremental.rs` (count/depth/size) |
| `lsp.md` | `active-lsp/tests/lsp.rs`, `redaction.rs` (C-036 sweep) |
| `packaging.md` | `packaging.rs` (names track the rename), `governance.rs` |
| `performance.md` | `benches/eval.rs` (7 benches incl. ★ fmt/lint/convert hot paths), `incremental.rs` (10k contract, memo shape) |
| `reproducibility.md` / lockfiles | `lock.rs` (manifest shape, hash stability, transitive taint) |
| `secrets.md` / C-036 | `secrets.rs` (taint, equality, Debug/Serialize, diagnostics audit), `redaction_paths.rs`, `redaction.rs`, `proptest_invariants.rs::secrets_never_leak_into_exports` |
| `threat-model.md` | `docs/threat-model-coverage.md` (item → test/residual), `sign.rs`, CLI keygen/sign/verify round-trip + `0600` test |
| `conformance.md` | `conformance.rs`, `corpus.rs` (spec pairs), `ported.rs` |
| `schema-library.md` | `schema_library.rs`, `tests/schemas_lib/` good/bad/bare/vendored |
| `readiness-1.0.md` | meta (C-090 record); exercised via the full gate, not unit-tested |
| diagnostics catalog (`docs/diagnostics.md`) | `diagnostics_spans.rs`, per-code tests above |

## Ported-corpus coverage (this goal)

- `tests/ported/toml/` — 266 toml-test cases through `toml_text_to_active`.
- `tests/ported/json/` — 293 JSONTestSuite cases through `json_text_to_active`.
- `tests/ported/yaml/` — 406 yaml-test-suite doc bodies through `yaml_text_to_active`.
- Fuzz/proptest: `fuzz.rs`, `proptest_invariants.rs` (incl. ★ sanitization-collision invariant), `fuzz/corpus/` seeds, `regressions.rs` minimized findings.
