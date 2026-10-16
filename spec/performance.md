# Performance & Incremental Evaluation (C-055, P-053)

Targets: 10 000-node configs evaluate under 50 ms p99 on
reference hardware; small edits re-evaluate only the affected
sub-DAG; memory stays linear in the IR. Continuous tracking via
criterion benches and the CodSpeed CI plugin.

## 1. Benchmarks (`benches/eval.rs`, criterion)

- `parse_10k`: lex + parse a generated 10 000-node document
  (50 tables × 200 chained keys).
- `eval_10k`: full evaluation of the same document.
- `incremental_leaf_edit`: full evaluate, change one leaf,
  re-parse, `evaluate_incremental` on the reused engine.
- `eval_small`: a 2-key document (`a = 1`, `b = a`; baseline
  noise floor).

Run with `cargo bench`; tracked continuously by the CodSpeed
GitHub workflow (`.github/workflows/codspeed.yml`), which fails
on significant regression vs the base branch.

## 2. Incremental evaluation (`Engine::evaluate_incremental`)

`evaluate()` is untouched (always full — every existing caller
keeps exact behavior). `evaluate_incremental(&self, module,
memo: &mut IncrementalMemo)` reuses fingerprints memoized from
previous calls in `memo` (one memo per document for long-lived
hosts; sharing a memo across engines is supported — the env
test does exactly that):

- Fingerprint per node: structural hash over the node tag, its
  literal data, the environment values it reads, and its
  dependencies' fingerprints (arena-indexed cache filled during
  the topo walk — no recursion, no pre-pass, no per-edge map
  lookup). Same fingerprint ⇒ same value: the engine's env
  snapshot and all dependency values feed the hash
  (`Engine::evaluate` never reads ambient state, §1
  hermeticity), so rotation invalidates exactly the affected
  cone. Profile selection, resilient mode, and budgets do NOT
  travel in the hash — they form the config token, which
  clears the store on any change (correctness over
  cleverness).
- The walk still visits every node in topological order, but
  fingerprint hits skip evaluation and serve the memoized value
  to dependents. Misses evaluate and store.
- Served diagnostics are re-anchored: memo entries hold
  diagnostics (with positions) from an earlier parse, so the
  hit path rewrites each served diagnostic's span/file from
  the current module's tables for the hitting node. Required
  for "same input → same diagnostics" (the W001 stale-span
  bug: a moved unresolved identifier otherwise reports its old
  line). Sound because every diagnostic the walk emits is
  node-local; post-walk transforms (profiles, patches,
  validation) re-run every call rather than serving memoized
  diagnostics.
- Fingerprint keys are salted per memo (`memo_salt`, getrandom
  with a fixed fallback): keys are never a stable cross-process
  oracle for short secret-derived values, and any collision
  search restarts per memo. Output determinism is unaffected —
  the salt only names internal keys. The 64-bit width stays
  load-bearing (a collision would serve a stale value): see
  the threat-model note (spec/threat-model.md, memo entries).
- Telemetry per call: `last_eval_count` (actually evaluated) and
  `last_memo_hits`. Tests assert a leaf edit recomputes O(depth)
  nodes, not N.
- Memory: the store is capped at `max(1024, 4 × arena nodes)`
  entries per memo and cleared past the cap — linear in the
  document, never growing with edit history. Clearing only
  loses hits; the bound is a pure function of counts, so
  behavior stays deterministic. Engines are cheap to drop;
  long-lived servers (the language server) reuse one memo per
  document to compound the wins (integration point, not yet
  wired — recorded here so the next LS pass can adopt it
  without redesign).

## 3. Measured numbers (reference hardware, release profile)

Recorded 2026-10-16 (Ryzen local dev machine, Windows;
criterion means, 100 samples each):

- parse 10k: ~47 ms · eval 10k full: ~25 ms (p99 < 50 ms ✓,
  ~2× headroom)
- incremental leaf edit (novel value, full-depth invalidation):
  ~31 ms wall; 401 of 29 951 walked nodes re-evaluate (1.3% —
  the 200-deep edited chain plus its two container tables;
  `leaf_edit_recomputes_only_the_affected_chain` pins the exact
  coverage `hits + evals` across both calls)
- structural sharing: the 9 950 identical `1` literals across
  the 50 chains evaluate once and hit thereafter; an edit that
  aliases a sibling chain (0→1) re-evaluates a single node
  (the root table) with byte-identical output
  (`edit_aliasing_an_existing_value_serves_shared_memo`)
- end-to-end `solid eval` (release CLI, pretty JSON to file):
  ~120 ms ≈ 36 ms startup/parse-input + 47 ms parse +
  25 ms eval + ~10 ms serialize; `solid check` (no serialize):
  ~111 ms. The in-process criterion numbers above are the
  C-055 acceptance basis; CLI startup is one-time process cost.
- memory: linear by construction (at most one memo entry per
  distinct fingerprint; steady-state re-evaluation allocates no
  new entries — `last_eval_count == 0` in
  `steady_state_reevaluation_serves_everything_from_memo`)

Optimization notes (2026-10-16): the first incremental cut was
slower than full eval (36.6 vs 25.5 ms) — a SipHash + `format!`
fingerprint pre-pass over the whole graph. Fixed by (1) an
inline FNV-1a mixer with zero-allocation structural hashing
(`FpHasher`/`fp_value`; 64-bit width is load-bearing — a
collision would serve a stale value), (2) fusing the pre-pass
into the topo walk with an arena-indexed cache (plain
`evaluate()` allocates nothing new), (3) hashing `BinOp`/`UnOp`
discriminants and `Unit` directly (added a `Hash` derive to
`Unit`). Result: −21% on the incremental path with no behavior
change (full-suite green, byte-identical trees).

Review round (2026-10-16, BYOK proxy): one Critical (W001
stale spans served from the memo after a re-parse — fixed by
re-anchoring served diagnostics to the current module, proven
by `moved_ident_serves_the_current_span` failing without the
fix), one High (unbounded memo growth across document versions
— fixed by the deterministic cap-and-clear), plus salt,
`Table::len` in the hash, value-compared config token, and six
new tests. Full triage record in
docs/Memories-and-Observations.md.

## 4. Testing contract

- 10k-node document evaluates correctly (values spot-checked).
- Incremental test: full count vs leaf-edit count, memo bounds,
  secret/env/profile invalidation cases.
- Benches compile and run (`cargo bench --no-run` in CI at
  minimum); codspeed workflow present.
