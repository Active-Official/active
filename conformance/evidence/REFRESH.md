# Evidence refresh record (2026-10-26)

At `1fdeaf5` (code tree `fcea0d3`) the local battery was re-run from the
repository root. Several artifacts are deterministic: they reproduced
byte-for-byte, so `git` records no change for them even though they were
re-run at this tree. This file is the record of that re-run and the
SHA-256 pin for each unchanged artifact.

| Artifact | Command | SHA-256 (first 16 hex) | Outcome |
|---|---|---|---|
| `harness.json` / `harness.log` | `python conformance/harness/run.py --out conformance/evidence/harness.json` | `6AB142EEBBF43C12` | byte-identical to the `e06df6d` capture; `ok: true`, `parity: []`, 10 pass and 18 toolchain-absent skips on this host |
| `codegen.log` | `python conformance/evidence/codegen_check.py` | `E5FC6797C82C14FA` | byte-identical to the `56ef612` capture; `CODEGEN: ALL PASS` |
| `output-schema.log` | `python conformance/evidence/output_schema_check.py` | `76F7893517DEB2C0` | byte-identical to the `ad77d31` capture; 9/9 outputs |
| `catalog-diff.log` | `python conformance/evidence/catalog_diff.py` | `D5A909141B29AC94` | byte-identical to the `e06df6d` capture; 89/90/0 |
| `agentkit-eval.json` / `agentkit.log` | `python agent-kit/eval/run.py --out conformance/evidence/agentkit-eval.json` | `1B40F36277FE5158` | byte-identical to the `204af76` capture; `ok: true` |
| `container-harness.json` / `.log` | the `docker run` command in the log header (image `active-all-toolchains:phase2`) | `C39ECE9F6039C86C` | regenerated; 28/28 pass, 0 skipped |
| `gates.log` | full gate battery (build, fmt, clippy, test, wasm) | - | regenerated; 3,250 tests, all exit 0 |
| `harness-tests.log` | `test_run.py`, `test_diff_vectors.py`, `run.py --self-check`, `test_capability.py`, `test_short_circuit.py` | - | regenerated |
| `miri.log` | `cargo +nightly miri test -p active-core --lib trace::` and `--lib blob` | - | regenerated; 3/3 and 3/3 |
| `binding-proof.log` | `python bindings/python/test_active.py` | - | regenerated; 10/10 |
| `ci.log` | `gh run list` / `gh run view` on both remotes | - | updated with each pushed tree's verdicts |

The deterministic rows above were captured before this record (their
last-touching commits are named); the re-run at `1fdeaf5` produced the
same bytes, which is why no new commit appears for them. Every one of
these checks also runs in CI Conformance on both remotes at each push
(`harness.json`, `codegen_check.py`, `output_schema_check.py`, and the
agent-kit gate), so the results are re-validated independently of the
local captures.
