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
| `output-schema.log` | `python conformance/evidence/output_schema_check.py` | `1647A4418EF38296` | regenerated for L-P16 (2026-11-18); 11/11 outputs (diff, plan, and plan-receipt added) |
| `catalog-diff.log` | `python conformance/evidence/catalog_diff.py` | `D5A909141B29AC94` | byte-identical to the `e06df6d` capture; 89/90/0 |
| `agentkit-eval.json` / `agentkit.log` | `python agent-kit/eval/run.py --out conformance/evidence/agentkit-eval.json` | `1B40F36277FE5158` | byte-identical to the `204af76` capture; `ok: true` |
| `container-harness.json` / `.log` | the `docker run` command in the log header (image `active-all-toolchains:phase2`) | `03D5A0D4E121D5C7` (`.json`), `E98CD9F475419C0A` (`.log`) | regenerated at `195b589` (2026-11-26); 28/28 pass, 0 skipped |
| `gates.log` | full gate battery (build, fmt, clippy, test, wasm) | - | regenerated; 3,250 tests, all exit 0 |
| `harness-tests.log` | `test_run.py`, `test_diff_vectors.py`, `run.py --self-check`, `test_capability.py`, `test_short_circuit.py` | - | regenerated |
| `miri.log` | `cargo +nightly miri test -p active-core --lib trace::` and `--lib blob` | - | regenerated; 3/3 and 3/3 |
| `binding-proof.log` | `python bindings/python/test_active.py` | - | regenerated; 10/10 |
| `ci.log` | `gh run list` / `gh run view` on both remotes | - | updated with each pushed tree's verdicts |
| `packaging-audit.log` | `pwsh -NoProfile -File conformance/evidence/packaging_audit.ps1` | - | regenerated; `PACKAGING-AUDIT: PASS` |

The deterministic rows above were captured before this record (their
last-touching commits are named); the re-run at `1fdeaf5` produced the
same bytes, which is why no new commit appears for them. Every one of
these checks also runs in CI Conformance on both remotes at each push
(`harness.json`, `codegen_check.py`, `output_schema_check.py`, and the
agent-kit gate), so the results are re-validated independently of the
local captures.

## Evidence refresh 2026-11-17 (final head)
The agent-kit evaluation snapshot now records its provenance: `head`
(`git rev-parse HEAD`) and `ran_at` (UTC). Re-run at `22c4247`:
`python agent-kit/eval/run.py --out conformance/evidence/agentkit-eval.json`
-> `head` 22c4247, `ok: true`, 5 tasks improved, mean 1.5 iterations, cone
agreement true. The deterministic fields are unchanged from the previous
capture; the timestamp intentionally varies per run, so byte-identity
checks compare the report body, not `ran_at`.

## Evidence refresh 2026-11-24 (L-P35 envelope schema)
The L-P35 repair envelope gained the top-level `profile`/`capabilities`
grant and `receipt.attenuation`; `docs/schemas/repair-receipt.json` was
updated to match. `output-schema.log` was regenerated so the L-P41
repair-receipt checks, which are in the script but predated the last
capture, now appear: `python conformance/evidence/output_schema_check.py`
-> SHA-256 `3A0E25B011274F1E`, `OUTPUT-SCHEMA: OK (11 outputs + broken lint file)`.

## Evidence refresh 2026-11-26 (L-P40 demand-driven evaluation)
`195b589` (tree `f51afb7`) changed the evaluator: `Demand::{All, Cone(P)}`
and the three-class cost model (L-P40). Because the change touches core
evaluation, the zero-SKIP container harness was re-run end to end to prove
the container build still passes every language. The `docker run` in the
`container-harness.log` header was executed on the Windows host against the
mounted checkout:

    docker run --rm -e CARGO_TARGET_DIR=/tmp/target \
      -e ACTIVE_FFI_LIB=/tmp/target/release/libactive_ffi.so \
      -e ACTIVE_WASM_PATH=/tmp/target/wasm32-unknown-unknown/debug/active_wasm.wasm \
      -v "$PWD:/work" -w /work active-all-toolchains:phase2 \
      bash -lc "cargo build -p active-ffi --release && \
                cargo build -p active-wasm --target wasm32-unknown-unknown && \
                python3 conformance/harness/run.py --out /work/conformance/evidence/container-harness.json"

`conformance/evidence/container-harness.json` -> `ok: true`,
`secret_leak: false`, `parity: []`, 28/28 `pass`, 0 skipped, 0 failed.
SHA-256 `03D5A0D4E121D5C7` (`.json`), `E98CD9F475419C0A` (`.log`). The
`.log` header now names `195b589` (tree `f51afb7`); its body is that run's
raw output.
