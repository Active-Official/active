# How the Execution Budget Works (C-037)

This is the definitive answer to "how does the execution budget
work". ACTIVE stays loop-proof and CI-safe through **two
complementary halves**: static parser caps that reject pathological
*shape*, and a runtime accountant that aborts pathological
*execution*. Both halves are deterministic except wall-clock (noted
below), and exceeding any limit is a **hard error** — fatal in
strict and resilient modes alike, because budgets protect the
process, not the document.

## 1. The two halves

| Half | Where | What it caps | Failure mode |
|---|---|---|---|
| Static caps | Parser (per file, at parse time) | Expression nesting (64, E004), block nesting (64, E011), schema-type nesting (64, E004), include depth (32 files, E016) | Diagnose with a span and **recover** (substitute null / skip body) — parsing always terminates |
| Runtime accountant | Evaluator (`Engine::budget`) | Nodes visited, value nesting depth, array length, total keys, wall-clock time | **Abort** with X001–X005 — no value tree is returned |

The halves compose: anything parseable is statically shallow, so the
evaluator's walk is a finite DAG traversal whose cost is linear in
the arena — the accountant then guards *scale* (a 50 MB monster
config fails fast with an X-code instead of eating CI) and
*pathological dynamics* (regex backtracking, merged nesting deeper
than any single file allows).

## 2. The seven runtime limits

`Engine` carries a public `Budget` (all fields `0` means unlimited —
the embedder accepts the risk explicitly):

| Field | Code | Default | Rationale |
|---|---|---|---|
| `max_nodes` | X001 | 100,000 | Arena-length pre-check: the walk visits each node once, so this bounds total eval work. 10× headroom over the C-055 10k-node perf target |
| `max_depth` | X003 | 256 | Reference-aware value nesting depth (see §2b). Single files cap at 64, but composed includes and reference chains can stack deeper |
| `max_array_len` | X004 | 100,000 | Longest single array. Billion-laughs-class inputs die here with a span |
| `max_keys` | X005 | 100,000 | Total **value-table** keys across the arena (schema tables excluded: metadata with their own parser-side limits) |
| `max_wall_ms` | X002 | 100 | Wall-clock backstop (P-033's 50–100 ms). Guards what counts cannot: pathological dynamics and unknown-unknowns. (Patterns specifically cannot burn wall-clock: the regex engine is automata-based, linear-time by construction.) |
| `max_value_size` | X006 | 10,000,000 | Materialized value size in data units (see §2b). Kills exponential reference expansion before anything materializes |
| `max_total_value_size` | X006 | 100,000,000 | Cumulative materialized size across evaluation: the DP total seeds it, and every post-walk clone (patch sets, profile merges, default fills) charges against it. Caps the sum where per-node bounds cap each clone |

Static shape checks (nodes, arrays, keys) run **before** the
walk as an iterative arena scan — deterministic, span-attributed to
the offending node (arrays) or global (nodes, keys), zero walk
overhead. Wall-clock samples during the walk (every 1024 nodes),
during validation (every 64 nodes), around every constraint check,
and once more at the end of `evaluate()` (small documents take zero
walk samples). The sampling — not the limit — is what keeps C-055
green.

### 2b. Reference-aware materialization bounds

`let` references clone values: `a1 = [a0, a0]` doubles `a0`'s
materialized size at every level, so thirty innocent-looking lines
can demand 2³⁰ values. No shape scan sees this (the arena stays
small and shallow) and no walk sample lands (few nodes visited).
The defense is a **dynamic-programming bounds pass** over the
topological order — dependencies first, so every bound is final
when read:

- `size[node]` bounds materialized data units: scalars count 1
  (plus string bytes, which are source-proportioned), composites
  count 1 plus their children, and each `Ref` costs its target's
  full bound (the clone!). Interpolation sums its parts.
- `depth[node]` bounds value nesting through references: tables
  and arrays add one over their children (references followed),
  everything else contributes nothing.
- Both saturate at `limit + 1`, so the numbers stay tiny and the
  pass stays linear even for exponential shapes; memoization makes
  shared subtrees O(1).
- Tripping either aborts with the offending node's span — before a
  single oversized value materializes. The DP total additionally
  seeds a cumulative account: patch sets charge measured clones plus
  path length, profile merges charge the merged subtree, default
  fills charge each inserted default — so many individually-legal
  clones still trip `max_total_value_size`. Operator-controlled
  dynamic values (environment, secrets) count 1 in the DP but charge
  actual-minus-bound deltas per clone during the walk, so large
  dynamic strings cannot hide behind static bounds either.

Patch edits compose arena values at paths afterwards, so a
conditional post-patch result-depth walk (iterative, only when
patches ran) closes the remaining gap. Profile merges replace
key-wise and cannot deepen, so they need no re-check.

## 3. Why wall-clock is special

Counts are deterministic: same input always trips (or clears) the
same count limit at the same node. Wall-clock is **intentionally
excluded** from the determinism contract — a loaded CI runner and an
idle laptop disagree about milliseconds. It is a backstop, not a
verdict: set `max_wall_ms: 0` to disable it wherever determinism
must be total (conformance runs do this). Never assert wall-clock
trips in tests except through decade-margin inputs (evil regexes
that take seconds against a millisecond limit).

## 4. Hard-error semantics

Budget violations return `EvalError::BudgetExceeded`, carrying the
first-tripped kind, the offending node's span when attributable,
and its file. There is no resilient downgrade: an over-budget
evaluation returns no tree in either mode. The CLI prints the X-code
like any error and exits 1. Normal diagnostics already collected
(warnings etc.) are discarded with the tree — the budget error
replaces them, because continuing would defeat the protection.

## 5. Tuning

- Library: set `engine.budget` fields before `evaluate()`; read
  `Budget::default()` for the shipped profile — or start from a
  named preset: `Budget::strict()` (== default), `Budget::interactive()`,
  `Budget::trusted()` (see table).
- CLI: `eval`, `check`, and `convert` accept `--budget-profile
  strict|interactive|trusted` (replacing every per-field limit) or
  the individual `--max-nodes`, `--max-depth`, `--max-array-len`,
  `--max-keys`, `--max-wall-ms`, `--max-value-size`, and
  `--max-total-value-size` flags (each defaulting to the shipped
  profile; `0` disables). Absurd durations use
  `checked_add` (degrading to no deadline where the platform would
  overflow) instead of panicking.
- Performance work (C-055) tunes these numbers with measurements,
  never hunches: if the 10k-node benchmark regresses, the limits
  move only with a recorded rationale.

### Named presets (CHECKLIST F.005; supersedes G-010 rows)

Hosts must never invent unsafe limits: start from one of these rows
and tune with measurements. WASM embeddings run the shipped hard
budgets (no per-call budgets on that path — DERIVED Phase-2
decision) with the §6 wall-clock delta in force; reference agent
loops additionally enforce iteration budgets (`--max-iters` with
stuck-loop detection).

| Preset | Nodes | Depth | Array / Keys | Wall ms | Value / Total | Use |
|---|---|---|---|---|---|---|
| `strict` (== default) | 100,000 | 256 | 100,000 | 100 | 10M / 100M | The reference: CI and serverless loops |
| `interactive` | 1,000,000 | 512 | 1,000,000 | 2,000 | 100M / 1G | Human present, bigger local files |
| `trusted` | 10,000,000 | 2048 | 10,000,000 | 30,000 | 1G / 10G | Operator-trusted input at production scale |

`0` (unlimited) is never a preset: embedders accept that risk
field-by-field, never by profile name. Depth scales modestly
because shape protection is barely negotiable even for trusted
input; wall-clock on `trusted` is generous because batch jobs own
their machines, while `interactive` stays short because a human is
waiting.

## 6. What budgets do NOT do

- They do not cap scalar or environment string length separately:
  strings charge bytes against `max_value_size` / `max_total_value_size`
  (§2b), so length is governed proportionally and linearly, and a separate
  length knob would duplicate that dimension. 200KB–1MB scalars and 100KB
  environment values are ordinary within-budget inputs (decision memo
  2026-10-19: no amplification mechanism exists for strings, and the
  environment operator is a trusted actor per spec/threat-model.md §2).
- Platform delta (wasm32-unknown-unknown): `Instant` aborts on that target,
  so `max_wall_ms` degrades to disabled there (`Budget::wall_supported()`
  returns false). Termination still holds without it: count, depth, array,
  key, and value-size budgets bound the walk, and the `regex` crate matches
  in linear time, so no wall-clock backstop is needed for termination.
  Hosts on that target enforce their own timeouts (Worker terminate
  pattern) and must not promise in-module wall-clock enforcement.
- Platform delta (Miri): the interpreter's mock clock advances with
  interpreted work rather than real time, so any wall budget would trip
  spuriously; `Budget::wall_supported()` returns false under `cfg(miri)`
  and the remaining budgets still bind execution. Wall-clock enforcement
  is a host-time property and is covered natively, never under Miri.
  Stress inputs are scaled down under `cfg(miri)` (same code paths,
  smaller documents) because per-byte shadow-memory cost turns linear
  scans into multi-hour runs; native runs keep full sizes.
- They do not replace the parser caps (different half, different
  codes, recover-vs-abort semantics).
- They do not make validation cheaper than linear (validation walks
  what evaluation already bounded).
- They do not sandbox I/O: evaluation is hermetic by construction
  (no shell/network/filesystem in the grammar), so there is nothing
  to sandbox — budgets bound time and space only.
