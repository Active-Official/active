# Verification evidence (expansion goal close-out)

Captured on HEAD `e57042f` (2026-10-22); the battery ran on the
identical code tree (`a30b2c9`, plus only the evidence files
themselves and a SPEC comment), then committed as `e57042f`.
To reproduce: run the same commands from the repo root.

| File | Verification step | Pass criterion in file |
|---|---|---|
| `harness.json` / `harness.log` | Step 1: shared harness + differential gate (Windows) | `ok: true`, `secret_leak: false`, `parity: []`, Tier-1 all pass |
| `container-harness.json` / `.log` | Step 1b: zero-SKIP proof (all-toolchain container) | `ok: true`, 28/28 `pass`, 0 skipped, 0 failed, `parity: []` |
| `binding-proof.log` | Step 2: Tier-1 fresh-consumer proof | `BINDING-PROOF: ALL PASS` (codes, resilient eval, T001 refusal, redaction) |
| `codegen.log` | Step 3: multi-target codegen validators | `CODEGEN: ALL PASS` (jsonschema/tsc+zod/gofmt+go vet+validator/python, determinism, stamps) |
| `cli-launch.log` | Step 4: CLI launch smoke | Full demo + complete transcripts, T001 refusal, `deterministic: True`, trace events |
| `gates.log` | Step 5: full workspace gates | Every section `GATE: PASS` (workspace tests, fmt, clippy, packaging, capability, harness tests, secret scan: 0 raw hits) |
| `ci.log` | Step 6: real CI verdicts | Live `gh run list` for HEAD on origin+official: Conformance/CI/CodSpeed all `success` |
| `review-final.log` | Step 7: fresh Copilot review (BYOK meta-muse xhigh) | `CONFIRMED CLEAN`, no open findings |
| `agentkit.log` / `agentkit-eval.json` / `agentkit-eval.log` | Step 6: agent-kit evaluation | `ok: true`, all loops identical per-task verdicts/iters/codes |
| `catalog-diff.log` | Error-code catalog diff | 62/62 real codes covered, 0 missing |
| `harness-tests.log` | Harness enforcement unit tests | 25/25 run.py + 14/14 diff_vectors + capability 3/3 |

Supporting scripts next to the logs (`codegen_check.py`) are the exact
runners used. CI runs harness, codegen, and the agent-kit gate on every
push and PR (see `.github/workflows/conformance.yml`); green runs on
this HEAD are recorded in CHECKLIST Phase gates (see
`C:\Users\Usuario\Downloads\active-phase2-plan\CHECKLIST.md`).
