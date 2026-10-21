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
  (correctly unrepairable).
- `eval/run.py` — runs every available loop over every vector and
  reports first-attempt success, mean iterations to clean, code
  histogram, budget adherence, and secret safety. Exit 0 only when
  every expectation holds.
- WASM module: `../active-wasm/js/active-shim.js` plus the release
  `.wasm` (zero imports, plain `WebAssembly.instantiate`).
- Prompt packs: `../prompts/` (versioned v1, tasks t1-t8, validation
  runner).

## Verification status

Harness proof: `python agent-kit/eval/run.py` — all three loops pass,
`clean-pass` resolves in 1 iteration, `broken-parse` is unrepairable in
all three, no secret material in any log.
