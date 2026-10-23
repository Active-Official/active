# Active Observability (CHECKLIST 3.005, PROPOSAL section 5.2)

Tracing is opt-in and disabled by default. When enabled, the engine
records phase events carrying counts and diagnostic codes only.
Values, sources, paths, spans, and secrets never enter an event by
construction: the event struct has no fields for them, and the
recorder only reads `Diagnostic.code`.

## Core tracing points

Enable per thread with `active_core::trace::set_tracing(true)` and
drain with `take_trail()`. Turning tracing off discards the buffer.
The CLI exposes this as `--trace` on `eval` and `check`, printing one
JSON object per line on stderr:

```json
{"phase":"eval_start","nodes":23,"diags":0,"codes":[]}
{"phase":"eval_done","nodes":19,"diags":1,"codes":["T004"]}
```

Phases today: `eval_start` (arena node count at entry) and `eval_done`
(result node count plus the full diagnostic code list). Failed
evaluations record only `eval_start`: there is no result to count.

Overhead when disabled is one thread-local flag check per evaluation.
The trail buffer lives thread-locally and is bounded by two events per
`evaluate` call; long-running hosts should drain between documents.

## Agent-loop metrics (CHECKLIST OBS.001, PROPOSAL §5.2)

The opt-in CLI trace plus the agent-kit eval report compose the
agent-loop metrics surface — no new event kinds needed:

- **Iteration counts**: `agent-kit/eval/run.py` records
  `mean_iterations_to_clean` and per-task iteration logs.
- **Budget headroom**: Data Mode W013 warnings fire at 80% of each
  nonzero limit; hard ceilings trip with X-codes either way.
- **Diagnostic histograms**: `--trace` JSONL `codes` arrays plus the
  eval harness `codes` histogram per loop; the repair planner's
  `final_codes` give per-run histograms for multi-step loops.
- **Audit trail**: `active eval --audit-trail FILE` (spec
  `audit-trail.md`) persists counts, codes, and the redacted content
  hash as a re-validatable Active document.

The four-field `TraceEvent` shape stays closed (adding a field
requires an RFC, a spec update here, and a test asserting the new
field cannot carry values) — see "Secret and source guarantee"
below.

## Binding-level hooks

Bindings already observe everything the core emits: diagnostic codes
per check/eval call, iteration counts in agent loops, budget outcomes
as terminal diagnostics. The reference loops log codes and counts only
(see `agent-kit/reference-loops/`); that discipline is the binding
contract, not a suggestion.

## Secret and source guarantee

- No trace event, binding log line, or harness report field carries a
  value, a source fragment, or a secret. The shared conformance
  harness fails the run if the raw fixture secret appears in any
  runner output.
- `active_core::trace::TraceEvent` has exactly four fields
  (`phase`, `nodes`, `diags`, `codes`); adding a fifth requires an
  RFC, a spec update here, and a test asserting the new field cannot
  carry values.
