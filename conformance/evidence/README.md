# Verification evidence (Master Program close-out)

Each artifact below names the tree and date it was captured at, and
`REFRESH.md` records the final re-run. Deterministic artifacts reproduced
byte-for-byte, so their last-touching commit predates the final tree even
though they were re-run there; `REFRESH.md` pins that with SHA-256. Rows
with an in-repo runner give the exact command; rows without one are
transcripts whose assertions are pinned by the named cargo or harness
tests. The final content tree was independently re-validated by the CI
Conformance job on both remotes (harness, codegen, output schemas,
agent-kit eval, and the editor smoke run on every push).

| File | Verification step | Pass criterion in file | Captured at |
|---|---|---|---|
| `harness.json` / `harness.log` | Step 1: shared harness + differential gate (Windows host) | `ok: true`, `secret_leak: false`, `parity: []`, Tier-1 all pass; 10 path groups pass and 18 are `skipped` because their toolchains are absent on this host (the container row is the all-language proof) | re-run at `1fdeaf5` with `python conformance/harness/run.py --out conformance/evidence/harness.json`; byte-identical to the `e06df6d` capture (`REFRESH.md`) |
| `container-harness.json` / `.log` | Step 1b: zero-SKIP proof (all-toolchain container) | `ok: true`, 28/28 `pass`, 0 skipped, 0 failed, `parity: []` | `1fdeaf5`, 2026-10-26 (image `active-all-toolchains:phase2`; command in the log header) |
| `binding-proof.log` | Step 2: Tier-1 binding proof | Python binding suite 10/10: structured codes and spans, resilient eval value, T001 refusal, redaction, determinism, stub/runtime parity, WASM resolution | `1fdeaf5`, 2026-10-26 (`python bindings/python/test_active.py`) |
| `codegen.log` | Step 3: multi-target codegen validators | `CODEGEN: ALL PASS` (jsonschema, tsc+zod, gofmt+go vet+validator, python import, determinism and version stamps) | re-run at `1fdeaf5` with `python conformance/evidence/codegen_check.py`; byte-identical to the `56ef612` capture (`REFRESH.md`) |
| `cli-launch.log` / `cli.log` | Step 4: CLI launch smoke | Demo transcripts, T001 refusal, deterministic output, trace events | 2026-10-21 transcript (tree `427806a`); every assertion is re-checked at HEAD by `active-cli/tests/cli.rs` and `active-cli/tests/loop_demo.rs` |
| `gates.log` | Step 5: full workspace gates | `BUILD_EXIT=0`, `FMT_EXIT=0`, `CLIPPY_EXIT=0`, `TEST_EXIT=0` (3,250 passed), `WASM_EXIT=0` | generated at `1b5ce29` (content tree `fcea0d3`), 2026-10-26; committed at `1fdeaf5` |
| `ci.log` | Step 6: real CI verdicts | Live `gh run list` for each pushed tree on origin+official | updated through `c7e4ec3` |
| `review.log` / `review-final.log` | Step 7: fresh review (BYOK xhigh) and round-by-round verification | zero open findings (12 round-73 findings and 7 round-77 findings, all fixed and verified) | verdicts at `1b5ce29`; the `review-final.log` header was corrected later (round range and tree lineage) with no verdict change |
| `agentkit.log` / `agentkit-eval.json` / `agentkit-eval.log` | Step 6b: agent-kit evaluation | `ok: true`, `tasks_improved` 5, mean 1.5 iterations, cone agreement true | re-run at `1fdeaf5` with `python agent-kit/eval/run.py --out conformance/evidence/agentkit-eval.json`; byte-identical to the `204af76` capture (`REFRESH.md`) |
| `catalog-diff.log` | Error-code catalog diff (`catalog_diff.py`) | 89 emitted, 90 catalogued, 0 missing (E009 intentionally unassigned) | re-run at `1fdeaf5`; byte-identical to the `e06df6d` capture (`REFRESH.md`) |
| `harness-tests.log` | Harness enforcement unit tests | 25/25 `test_run.py`, 14/14 `test_diff_vectors.py`, self-check 15/15, capability 6/6, interchange short-circuit 4/4 | `1fdeaf5`, 2026-10-26 |
| `miri.log` | Miri (nightly) over `active-core` trace + blob units | 3/3 trace + 3/3 blob pass, no UB (master program EVID.001) | `1fdeaf5`, 2026-10-26 |
| `output-schema.log` | Tooling JSON output schemas | 9/9 outputs validate, broken-lint control OK | re-run at `1fdeaf5` with `python conformance/evidence/output_schema_check.py`; byte-identical to the `ad77d31` capture (`REFRESH.md`) |
| `research-battery.md` | Research battery + corpus summary | 1,998 RB tests, ported corpora counts, fuzz seeds, secret proofs | captured 2026-10-22, committed 2026-10-23 (`204af76`) |

Supporting scripts next to the logs (`codegen_check.py`,
`catalog_diff.py`) are the exact runners used. CI runs harness, codegen,
and the agent-kit gate on every push and PR (see
`.github/workflows/conformance.yml`); green runs on the pushed tree are
recorded in the phase gates and the completion record.

## All-toolchain image (zero-SKIP proof)

The authoritative all-language environment is the owner-provided Docker
image `active-all-toolchains:phase2` (Ubuntu 24.04 plus every harness
toolchain: Rust/wasm32, Node, Go, Python/wasmtime, Java, Ruby, PHP, Perl,
R, Julia, Nim, GHC, OCaml, Elixir, Dart, .NET, Zig, LuaJIT, gfortran,
kotlinc, Swift, V). The 28/28 container report above was produced there,
including `julia: pass`.

- Archive: `G:\Docker-Images\active-all-toolchains.tar` (~3.2 GB)
- Restore: `docker load -i G:\Docker-Images\active-all-toolchains.tar`
- Re-save after changes:
  `docker save active-all-toolchains:phase2 -o G:\Docker-Images\active-all-toolchains.tar`

CI Linux runners cannot pull a local archive, so the workflow installs
the same toolchains. Those steps are **required** (never a silent SKIP):
Julia 1.11.5 and Nim 2.2.4 come from pinned official tarballs (the
runner's apt index has no Julia candidate and juliaup's metadata DB was
unreliable), Crystal and Dart use their distro apt repos, and every
install retries a transient mirror failure.

## Container harness artifacts

The zero-SKIP container build uses an isolated `CARGO_TARGET_DIR`. Set
`ACTIVE_FFI_LIB` and `ACTIVE_WASM_PATH` to the artifacts built in that
directory so the differential gate tests the same core the container
just built, never a stale in-repo `.so` or `.wasm`:

```sh
docker run --rm \
  -e CARGO_TARGET_DIR=/tmp/target \
  -e ACTIVE_FFI_LIB=/tmp/target/release/libactive_ffi.so \
  -e ACTIVE_WASM_PATH=/tmp/target/wasm32-unknown-unknown/debug/active_wasm.wasm \
  -v "$PWD:/work" -w /work active-all-toolchains:phase2 \
  bash -lc "cargo build -p active-ffi --release && \
            cargo build -p active-wasm --target wasm32-unknown-unknown && \
            python3 conformance/harness/run.py --out /work/conformance/evidence/container-harness.json"
```
