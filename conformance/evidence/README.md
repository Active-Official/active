# Verification evidence (Master Program close-out)

Captured on the Master Program tree (2026-10-22) by re-running the
battery below. The commit that lands these files carries the exact
tree; re-run the commands from the repo root to reproduce any row.

| File | Verification step | Pass criterion in file |
|---|---|---|
| `harness.json` / `harness.log` | Step 1: shared harness + differential gate (Windows) | `ok: true`, `secret_leak: false`, `parity: []`, Tier-1 all pass |
| `container-harness.json` / `.log` | Step 1b: zero-SKIP proof (all-toolchain container) | `ok: true`, 28/28 `pass`, 0 skipped, 0 failed, `parity: []` |
| `binding-proof.log` | Step 2: Tier-1 fresh-consumer proof | `BINDING-PROOF: ALL PASS` (codes, resilient eval, T001 refusal, redaction) |
| `codegen.log` | Step 3: multi-target codegen validators | `CODEGEN: ALL PASS` (jsonschema/tsc+zod/gofmt+go vet+validator/python, determinism, stamps) |
| `cli-launch.log` / `cli.log` | Step 4: CLI launch smoke | Full demo + complete transcripts, T001 refusal, `deterministic: True`, trace events |
| `gates.log` | Step 5: full workspace gates | Every section `GATE: PASS` (workspace tests, fmt, clippy, packaging, capability, harness tests, secret scan: 0 raw hits) |
| `ci.log` | Step 6: real CI verdicts | Live `gh run list` for the pushed HEAD on origin+official |
| `review-final.log` | Step 7: fresh review (BYOK xhigh) | `CONFIRMED CLEAN`, no open findings |
| `agentkit.log` / `agentkit-eval.json` / `agentkit-eval.log` | Step 6: agent-kit evaluation | `ok: true`, `tasks_improved` 5, mean 1.5 iterations, cone agreement true |
| `catalog-diff.log` | Error-code catalog diff (`catalog_diff.py`) | 78 emitted, 79 catalogued, 0 missing (E009 intentionally unassigned) |
| `harness-tests.log` | Harness enforcement unit tests | 25/25 run.py + 14/14 diff_vectors + capability 4/4 |
| `miri.log` | Miri (nightly) over `active-core` trace + blob units | 3/3 trace + 3/3 blob pass, no UB (master program EVID.001) |
| `research-battery.md` | Research battery + corpus summary | 1,998 RB tests, ported corpora counts, fuzz seeds, secret proofs |

Supporting scripts next to the logs (`codegen_check.py`,
`catalog_diff.py`) are the exact runners used. CI runs harness, codegen,
and the agent-kit gate on every push and PR (see
`.github/workflows/conformance.yml`); green runs on the pushed tree are
recorded in the phase gates and the completion record.
