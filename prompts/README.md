# ACTIVE AI Prompt Pack v1.1.0 (P-052)

Versioned system prompts and few-shot examples that teach coding
agents to emit idiomatic ACTIVE, respect `@ai_immutable` and
constraints, and prefer patches over whole-file rewrites.

## Contents

- `VERSION`: pack version (`1.1.0`); every prompt file carries
  the same version in its header comment.
- `v1/system-generator.md`: write new ACTIVE from requirements.
- `v1/system-editor.md`: edit documents (minimal diffs, patch
  answers, immutable refusal).
- `v1/system-explainer.md`: answer language questions with exact
  diagnostic codes.
- `tasks/`: the fixed validation suite (`t1`-`t14`, TOML:
  system, user, optional document, machine-readable checks).
- `runner/validate.py`: first-attempt harness (no retries):
  sends each task to each backend, extracts the fenced document,
  checks it with the real `ACTIVE` binary, writes
  `results/round-<stamp>.json`.
- `results/`: measured runs (committed evidence).

## Validation bar (C-054)

≥80% first-attempt success on each of two backends:

- `copilot`: GitHub Copilot CLI (agentic framework) via BYOK.
- `direct`: raw OpenAI-compatible chat API (embedded-agent
  integration shape).

Backends used here are local and free; the model is disclosed in
each results file. A third-party hosted framework needs API keys
absent from this machine. That limitation is recorded, not
hidden. Re-run with `python3 prompts/runner/validate.py`.

## Measured baselines

- `results/round-master-full.json` (pack 1.1.0, tasks `t1`-`t13`,
  model `muse-spark-1.3-contributor` on both backends): copilot
  13/13 first-attempt (100%), direct 11/13 (85%). Both clear the
  ≥80% bar; the direct-backend misses are recorded verbatim in
  the results file for audit.
- `results/round-master-t14.json` (pack 1.1.0, tasks `t1`-`t14`,
  model `muse-spark-1.3-contributor` on both backends): copilot
  14/14 first-attempt (100%), direct 12/14 (86%). New task `t14`
  (partial-eval cones) passes on both backends.
- Tasks `t11` (blob), `t12` (typo-fill repair), `t13` (Data Mode),
  and `t14` (partial-eval cones) cover the Master Program
  additions (CA, R, M, PE); every new code family and construct
  ships with a prompt-pack task.

## Evidence policy

Results JSON commits model responses verbatim for audit. Tasks
use fake credentials only (vault paths like `prod/db/password`,
never real secrets), so committed outputs carry no live secrets
by construction. Keep it that way when adding tasks.

## Use

Paste a system file plus your request into any agent, or point an
agent framework at it. Demand the ```active fence contract in your
own wrapper prompts. Every check in `tasks/` assumes it.
