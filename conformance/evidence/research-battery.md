# Research battery and corpus summary (EVID.001)

Captured 2026-10-22 on the Master Program tree. Counts are directory facts,
reproducible with the commands below.

## Research battery

- **1,998 research-battery tests** in `active-core/tests/research_battery.rs`
  (`RB-xxx`), each driving the shipped path `parse`/`parse_file` →
  `Engine::evaluate` → JSON export and asserting diagnostic classes plus
  positive-control neighbours. Rule: a red test fixes the code, never the
  expectation.
- Index and lineage: `docs/research-test-index.md` (YAML/TOML/JSON failure
  lineages, SyntaxErrorOS batches, historical failures).

## Ported corpora

| Corpus | Files in tree | Driven by |
|---|---|---|
| toml-test | 538 (case + expectation pairs) | `toml_text_to_active` |
| JSONTestSuite | 587 | `json_text_to_active` |
| yaml-test-suite | 814 | `yaml_text_to_active` |
| Active spec pairs | 13 `.active` + sidecars | `corpus.rs` |
| Negative fixtures | 31 | rejection suites |

## Fuzzing and memory safety

- 19 seed inputs under `fuzz/corpus/{lexer,parser,interpolation}` plus
  proptest invariants (`fuzz.rs`, `proptest_invariants.rs`): no panics,
  termination, determinism, secret non-leakage.
- Miri (nightly) over the trace and blob units: clean (`miri.log`).

## Cross-implementation proof

- `conformance/manifest.toml`: 16 cases, driven by `harness/run.py` over the
  reference, C ABI, WASM, and binding paths (`harness.json`).
- Differential gate (`harness/diff_vectors.py`): ordered codes, span tuples,
  severities, refused flags, and redacted JSON identical across the C ABI,
  Python, and JS embeddings (`DIFFERENTIAL-OK`).
- Secret non-leakage: any raw fixture secret in any captured output fails the
  whole run; `harness.json` records `secret_leak: false`.

## Reproduce

```powershell
cargo test -p active-core --test research_battery
python conformance/harness/run.py --out conformance/evidence/harness.json
python conformance/harness/diff_vectors.py
cargo +nightly miri test -p active-core --lib trace::
cargo +nightly miri test -p active-core --lib blob
```
