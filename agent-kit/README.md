# Active Agent Kit (CHECKLIST 2.019-2.021, PROPOSAL §4)

Turns Active into a drop-in agent capability: machine-readable tool
descriptors, versioned prompt packs, conformance vectors, the WASM
module, reference loops, and an evaluation harness.

## Layout

- `capability.schema.json` — every tool an agent may call (`check`,
  `eval`, `format`, `convert`, `graph`, `generate`,
  `schema-introspect`), with input/output shapes. Versioned with the
  workspace; consumers must reject unknown major versions.
- `reference-loops/{python,typescript,go}/` — the full repair cycle on
  the real path: check (diagnose) → resilient eval (repair) → re-emit
  via `active import` → check again (validate) → final redacted eval.
  Budgeted iterations, stuck detection, code-only secret-safe logs.
- `vectors/` — loop evaluation tasks: `repair-fill` (repairs in 2
  iterations), `clean-pass` (clean on first check), `broken-parse`
  (correctly unrepairable), `format-broken` (T003, converged),
  `requires-missing` (T004, converged), `unknown-format` (schema
  bug, converged), `descriptions-clean` (metadata ignored, clean
  on first check).
- `eval/run.py` — runs every available loop over every vector and
  reports first-attempt success, mean iterations to clean, code
  histogram, budget adherence, and secret safety. Exit 0 only when
  every expectation holds. Required CI check
  (`.github/workflows/conformance.yml`).
- `eval/test_capability.py` — pins the capability descriptor
  authority (major version 1, complete 7-tool surface).
- WASM module: `../active-wasm/js/active-shim.js` plus the release
  `.wasm` (zero imports, plain `WebAssembly.instantiate`).
- Prompt packs: `../prompts/` (versioned v1.1.0, tasks t1-t10, validation
  runner).

## Verification status

Harness proof: `python agent-kit/eval/run.py` — all three loops pass
with identical per-task verdicts, iterations, and codes
(`tasks_improved` 4, mean 1.67 iterations to clean), `broken-parse`
is unrepairable in all three, and no secret material appears in any
log. Prompt pack proof: `prompts/runner/validate.py` round
`phase2-e7` — 10/10 first-attempt on both backends, zero flakes.
