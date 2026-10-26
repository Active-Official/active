# Verification evidence (Master Program close-out)

Captured on the final tree (2026-10-26) by re-running the
battery below. The commit that lands these files carries the exact
tree; re-run the commands from the repo root to reproduce any row.

| File | Verification step | Pass criterion in file |
|---|---|---|
| `harness.json` / `harness.log` | Step 1: shared harness + differential gate (Windows) | `ok: true`, `secret_leak: false`, `parity: []`, Tier-1 all pass |
| `container-harness.json` / `.log` | Step 1b: zero-SKIP proof (all-toolchain container) | `ok: true`, 28/28 `pass`, 0 skipped, 0 failed, `parity: []` |
| `binding-proof.log` | Step 2: Tier-1 fresh-consumer proof | `BINDING-PROOF: ALL PASS` (codes, resilient eval, T001 refusal, redaction) |
| `codegen.log` | Step 3: multi-target codegen validators | `CODEGEN: ALL PASS` (jsonschema/tsc+zod/gofmt+go vet+validator/python, determinism, stamps) |
| `cli-launch.log` / `cli.log` | Step 4: CLI launch smoke | Full demo + complete transcripts, T001 refusal, `deterministic: True`, trace events |
| `gates.log` | Step 5: full workspace gates | `BUILD_EXIT=0`, `FMT_EXIT=0`, `CLIPPY_EXIT=0`, `TEST_EXIT=0`, `WASM_EXIT=0` |
| `ci.log` | Step 6: real CI verdicts | Live `gh run list` for the pushed HEAD on origin+official |
| `review-final.log` | Step 7: fresh review (BYOK xhigh) | all 12 round-73 findings FIXED, zero open findings |
| `agentkit.log` / `agentkit-eval.json` / `agentkit-eval.log` | Step 6: agent-kit evaluation | `ok: true`, `tasks_improved` 5, mean 1.5 iterations, cone agreement true |
| `catalog-diff.log` | Error-code catalog diff (`catalog_diff.py`) | 89 emitted, 90 catalogued, 0 missing (E009 intentionally unassigned) |
| `harness-tests.log` | Harness enforcement unit tests | 25/25 run.py + 14/14 diff_vectors + capability 4/4 |
| `miri.log` | Miri (nightly) over `active-core` trace + blob units | 3/3 trace + 3/3 blob pass, no UB (master program EVID.001) |
| `research-battery.md` | Research battery + corpus summary | 1,998 RB tests, ported corpora counts, fuzz seeds, secret proofs |

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
