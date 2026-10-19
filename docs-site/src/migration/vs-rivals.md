# Active vs CUE, Nickel, Pkl, and Starlark

Honest deltas against the serious alternatives, stated as what Active
refuses. No strawmen: each of these is well built for its home ground.

## CUE: unification vs explicit assignment

CUE unifies values with solver semantics: constraints merge, defaults
resolve, and the engine decides what holds. Active assigns explicitly
with no solver to learn: a binding means exactly its right-hand side.
CUE ships no secret-taint model; Active taint survives generation,
evaluation, and export. CUE constraints are powerful; Active pairs
constraints with budgets so a hostile input dies fast instead of
solving long.

## Nickel: lazy contracts vs strict determinism

Nickel evaluates lazily with a contract system and record merging, aimed
at the Nix corridor. Active evaluates strictly and deterministically with
explicit bindings and first-class budgets: no laziness surprises, no
merge-order puzzles, and every resource dimension capped.

## Pkl: remote imports vs hermetic refusal

Pkl is schema-first with class amendments and remote (HTTPS) imports.
Active refuses ambient network I/O entirely: includes and env flow only
through explicit host callbacks, keeping evaluation hermetic by
construction. A config that fetches the network during evaluation cannot
be reviewed, cached, or replayed; Active makes that inexpressible.

## Starlark: general computation vs total budgets

Starlark is deterministic yet computationally general (functions, loops).
Active refuses general computation, which is what makes budgets total
and agent output reviewable: every Active document terminates in bounded
time and space, with no recursion limits to tune and no halting questions
to dodge.

## The pattern

Each rival is more powerful along one axis. Active trades that power for
properties agents and reviewers can rely on: explicit meaning, taint that
holds, budgets that always hold, diagnostics with codes and spans. Pick
the power when you need computation; pick Active when the config must
never lie.
