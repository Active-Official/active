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
| Static caps | Parser (per file, at parse time) | Input bytes (1 MiB, E029), arena nodes (1,000,000, E029), parse wall clock (2 s, E029), expression nesting (64, E004), block nesting (64, E011), schema-type nesting (64, E004), include depth (32 files, E016), include fan-out (256 resolutions / 16 MiB, E028) | **Fail closed** on the three work bounds (no tree is returned); diagnose with a span and **recover** on the shape bounds (substitute null / skip body) — parsing always terminates |
| Runtime accountant | Evaluator (`Engine::budget`) | Nodes visited, value nesting depth, array length, total keys, wall-clock time | **Abort** with X001–X006 — no value tree is returned |

The parser is recursive descent, so reaching the 64-level expression limit
uses several stack frames per level. The shipped CLI and language server run
parsing on a dedicated 32 MiB stack thread, which keeps the documented
limits reachable in unoptimized builds; embedders calling the library
directly should provide at least 4 MiB of stack (the reference tests and
release builds stay within that).

The halves compose: anything parseable is statically shallow, so the
evaluator's walk is a finite DAG traversal whose cost is linear in
the arena — the accountant then guards *scale* (a 50 MB monster
config fails fast with an X-code instead of eating CI) and
*pathological dynamics* (regex backtracking, merged nesting deeper
than any single file allows).

## 1b. The parse-phase work bounds (LANG-C-003)

Parsing runs before evaluation and before any runtime limit exists, so it
carries its own three bounds (`ParseOptions`). They are **hard ceilings
with a fail-closed exit**: when one trips, the partial arena is discarded,
the module carries exactly one E029, and nothing is evaluated. Handing a
truncated tree to evaluation would report phantom syntax errors for a
document whose only problem is size.

| Field | Default | Checked | Trip message |
|---|---|---|---|
| `max_input_bytes` | 4,194,304 (4 MiB) | String entry: `source.len()` before lexing. File entry (`parse_file`, `parse_file_with_trust`): file length from metadata **before** the read. Includes: metadata before the read, per file. Every embedding inherits it (CLI `--max-input-bytes`, FFI `active_parse_limits_set`, WASM and the converters call the same parse entry; the LSP adds its own 4 MiB document cap with a visible message). | `parse budget exceeded: input is <n> bytes; the limit is <n>` |
| `max_nodes` | 1,000,000 | Arena length, once per statement and per array element | `parse budget exceeded: node limit <n> reached` |
| `max_wall_ms` (base) | 100, plus 30 ms per KiB of charged input | Lexer loop every 2,048 iterations; statement loop every 16 statements; once per include | `parse budget exceeded: the <n> ms parse wall budget was reached` |

Design rules:

- **The size cap is the primary bound and it is deterministic.** Parsing
  is linear in input bytes, so capping bytes caps total parse work: the
  same document is rejected by the same bound on every machine, and the
  metadata check means an oversized file is never allocated. 4 MiB is
  more than an order of magnitude above the largest fixture in this
  repository (~250 KB) and leaves room for the 1 MB scalar literals §6
  keeps ordinary, plus the rest of a document around them.
- **The node cap is the deterministic backstop for expansion.** Includes
  compose up to 16 MiB of source into one arena, so a small root can
  still describe a large tree. 1,000,000 nodes is ten times the default
  evaluator `max_nodes` (100,000): a document that would be rejected by
  evaluation anyway never gets built.
- **The clock is a backstop against superlinear shape, not the routine
  bound.** It is excluded from the determinism contract for the same
  reason as X002 (§3). The budget is `base + 30 ms per KiB of charged
  input` - the root source plus every included source read so far, so an
  include graph is one parse whose bytes extend the budget they consume.
  The deadline therefore scales with the document: 122,980 ms for a
  maximum-size 4 MiB root, 4,900 ms for a 160 KB one. The rate is
  calibrated against the slowest shipped shape rather than the average:
  table-dense documents (one enclosing-field-map snapshot per child
  table) measure 18.6 ms/KiB unoptimized at 5,000 tables on the reference
  machine, while flat keys, strings, arrays, and interpolation-heavy input
  measure 0.7-2.0 ms/KiB. 30 ms/KiB is ~1.6x the slowest measured cost and
  an order of magnitude above the rest, so a *linear* parse of a legal
  document never trips the clock on any build, while a superlinear shape
  fails within a small multiple of its legitimate cost. Raise the base
  when you raise `max_input_bytes` for faster hardware expectations; the
  per-KiB allowance follows the size automatically. Generated documents with tens
  of thousands of tables exceed the shipped rate today; raise
  `--parse-wall-ms` (or `ParseOptions::max_wall_ms`) for them.
- **Clock-less targets have no parse clock.** `wasm32-unknown-unknown`
  and miri return no deadline (`Budget::wall_supported()` is false), and
  the size and node caps still bound the parse there. Termination never
  depends on the clock.

Tuning:

- Library: set the three `ParseOptions` fields; `0` disables a bound
  (the embedder accepts the risk explicitly). `ParseOptions::default()`,
  `::strict()`, and `::resilient_mode()` share the shipped values.
- CLI: `--max-input-bytes <BYTES>` and `--parse-wall-ms <MS>` (the base)
  are global flags on every subcommand (defaults above; `0` disables).
  Raise `--max-input-bytes` for generated input larger than 4 MiB; the
  wall allowance follows the size.
- FFI (C ABI v1): `active_parse_limits_set(max_input_bytes, max_wall_ms)`
  sets the same two bounds process-wide for every later parse; `0` keeps
  the shipped default, and the C boundary deliberately has no "unlimited".
- The parse bounds and the evaluator budget are separate: a document can
  parse cleanly and still fail X001. Sizing a host means choosing both.

## 2. The seven runtime limits

`Engine` carries a public `Budget`. A zero field means the shipped default,
not unlimited: `Budget::resolve()` replaces zeros with `Budget::default()`'s
value at every evaluation boundary, so no one disables a ceiling with a
zeroed struct by accident. Disabling a limit is explicit -
`Budget::unlimited()` (the `eval::UNLIMITED` sentinel, `u64::MAX`) - and the
same meaning holds on every surface: the library resolves zeros identically,
the FFI's `ActiveBudget` treats `0` as the shipped default (a test pins it),
and the CLI's `--max-*` flags use the default when absent and the sentinel
when the user passes an explicit `0`:

| Field | Code | Default | Rationale |
|---|---|---|---|
| `max_nodes` | X001 | 100,000 | Arena-length pre-check: the walk visits each node once, so this bounds total eval work. 10× headroom over the C-055 10k-node perf target |
| `max_depth` | X003 | 256 | Reference-aware value nesting depth (see §2b). Single files cap at 64, but composed includes and reference chains can stack deeper |
| `max_array_len` | X004 | 100,000 | Longest single array. Billion-laughs-class inputs die here with a span |
| `max_keys` | X005 | 100,000 | Total **value-table** keys across the arena (schema tables excluded: metadata with their own parser-side limits) |
| `max_wall_ms` | X002 | 100 | Wall-clock backstop (P-033's 50–100 ms). Guards what counts cannot: pathological dynamics and unknown-unknowns. (Patterns specifically cannot burn wall-clock: the regex engine is automata-based, linear-time by construction.) |
| `max_value_size` | X006 | 10,000,000 | Materialized value size in data units (see §2b). Kills exponential reference expansion before anything materializes |
| `max_total_value_size` | X006 | 100,000,000 | Cumulative materialized size across evaluation: the DP total seeds it, and every post-walk clone (patch sets, profile merges, default fills) charges against it. Caps the sum where per-node bounds cap each clone  It also charges **measured dynamic growth** (LANG-C-001): a doubling interpolation charges the string it builds, and every clone charges the target's real measured size above its DP bound, so a small document that amplifies an environment value into megabytes trips X006 deterministically instead of exhausting memory. |

Static shape checks (nodes, arrays, keys) run **before** the
walk as an iterative arena scan — deterministic, span-attributed to
the offending node (arrays) or global (nodes, keys), zero walk
overhead. Wall-clock samples during the walk (every 64 nodes, and
every 1 MiB of charged bytes), during validation (every 64 nodes),
around every constraint check,
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
  and arrays add one over their children, an interpolation adds one
  over the values it renders (references followed), everything else
  contributes nothing. A long `${…}` chain therefore trips X003
  instead of exhausting the process stack.
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

The deadline is sampled on **two** axes, whichever comes first
(LANG-C-006): every 64 visited nodes, and every 1 MiB of value bytes the
accountant charges (`WALL_SAMPLE_BYTES`). The node axis alone never fires
on the documents that need it most - an 8-level environment doubling has
27 nodes and materialises megabytes, so its only check used to be the
closing one, 6-17x past the limit. The byte axis also runs *before* the
clone that pays for it (a `Ref` charges its measured delta first), so a
trip stops the walk ahead of the allocation. Byte sampling changes no
limit value, only when the deadline is observed.

## 4. Hard-error semantics

Budget violations return `EvalError::BudgetExceeded`, carrying the
first-tripped kind, the offending node's span when attributable,
and its file. There is no resilient downgrade: an over-budget
evaluation returns no tree in either mode. The CLI prints the X-code
like any error and exits 1. Normal diagnostics already collected
(warnings etc.) are discarded with the tree, because continuing would
defeat the protection - with one exception: when a budget trip aborts
evaluation, any already-collected progressive warnings (W013) are
attached to the `BudgetExceeded` error (`EvalError::attached_warnings`,
printed by the CLI before the X-code), so the operator sees both the trip
and the warning that preceded it.

## 5. Tuning

- Library: set `engine.budget` fields before `evaluate()`; read
  `Budget::default()` for the shipped profile — or start from a
  named preset: `Budget::strict()` (== default), `Budget::interactive()`,
  `Budget::trusted()` (see table).
- CLI: `eval`, `check`, `convert`, and `debug` accept
  `--budget-profile strict|interactive|trusted` (replacing every
  per-field limit) or the individual `--max-nodes`, `--max-depth`,
  `--max-array-len`, `--max-keys`, `--max-wall-ms`,
  `--max-value-size`, and `--max-total-value-size` flags (each
  defaulting to the shipped profile; `0` disables). Absurd durations
  use `checked_add` (degrading to no deadline where the platform
  would overflow) instead of panicking. `debug --replay` honors the
  same limits: a wall-bounded preflight mirrors `eval` and always runs
  (a caller's `max_wall_ms: 0` still gets the preflight under the
  canonical budget), then two identical canonical step/hash passes run
  under the finite **canonical wall budget** (`eval::CANONICAL_WALL_MS`,
  30 s - the `trusted` preset's wall clock; count, depth, array, key,
  and size limits still bind). Canonical passes cannot inherit the
  caller's clock (a document that just passed could fail the second pass
  on load alone) and never run without one (LANG-C-008): the same finite
  deadline everywhere keeps the pass reproducible and bounded. The
  audit/lock canonical hashing passes and the language server's step
  surface use the same policy.
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
