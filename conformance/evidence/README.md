# Verification evidence (expansion goal close-out)

Captured on HEAD `b7f1bb2` (2026-10-21), committed so the close-out is
auditable from the repo itself rather than only from throwaway scratch.
To reproduce: run the same commands from the repo root; every log states
its own command at the top where applicable.

| File | Verification step | Pass criterion in file |
|---|---|---|
| `harness.log` | Step 1: shared harness + differential gate | `ok: true`, `secret_leak: false`, `parity: []`, `differential: DIFFERENTIAL-OK`, 10 pass / 18 skip / 0 fail |
| `binding-proof.log` | Step 2: Tier-1 fresh-consumer proof | `BINDING-PROOF: ALL PASS` with per-diagnostic span transcripts |
| `codegen.log` | Step 3: multi-target codegen validators | `CODEGEN: ALL PASS` with tsc/gofmt/go-vet/schema/python echoes |
| `cli-launch.log` | Step 4: CLI launch smoke | Full demo + complete transcripts, T001 refusal, `deterministic: True` |
| `gates.log` | Step 5: full workspace gates | 71 suites, 2691 passed, 0 failed, `fmt_exit=0`, `clippy done`, lsp 60 + redaction 5 inline |
| `agentkit.log` / `agentkit-eval.log` | Step 6: agent-kit evaluation | `ok: true`, every loop `tasks_improved: 1` with unassisted baselines |
| `catalog-diff.log` | Error-code catalog diff | 62/62 real codes covered, 0 missing |
| `harness-tests.log` | Harness enforcement unit tests | 23/23 pass (`python -m unittest conformance.harness.test_run -v`) |

Supporting scripts next to the logs (`codegen_check.py`) are the exact
runners used. CI runs all of the above on every push (see
`.github/workflows/conformance.yml`); green runs on this HEAD: Conformance
39608477192, CI 39608477150, CodSpeed 39608477124.
