# Goal-directed repair engine (TOOLING §7, Phase R)

The repair engine turns the suggester's mechanical directives into a
closed loop: **plan** ordered safe steps, **apply** them by prepending
directives to the source and re-running the real parser and evaluator,
then **re-plan** until the document is clean, the loop is stuck, or the
step budget is exhausted. It never patches the value tree directly, so
what a repair produces is exactly what a hand-written directive would.

The browser and partial-apply surface on top of this engine is
specified separately in `spec/repair-ui.md`.

## 1. Goals

| Goal | Optimizes |
|---|---|
| `validate` (default) | reach a clean document with the fewest steps |
| `preserve-key` | keep the named paths byte-identical while repairing around them |
| `minimize-diff` | prefer steps whose edit is smallest at the unstable path |

`--preserve <path>` is a subtree contract: a step is skipped when its
target is the preserved path, an ancestor of it, or a descendant of it.
Preservation applies to every goal; `preserve-key` simply requires at
least one path.

## 2. Plan

A plan is a `Vec<PlannedStep>` in application order:

```json
{"kind": "fill", "path": "server.port", "directive": "@patch server.port = 8080",
 "safe": true, "status": "applied", "side_effects": ["default from schema"]}
```

- `kind` is one of `fill` (schema default), `rename` (did-you-mean),
  `patch` (mechanical edit), or `remove` (dead directive cleanup).
- `safe` is the suggester's threshold decision. Only safe steps are
  applied automatically; an unsafe step stays in the plan with its
  reason so an agent or human can choose.
- `status` is `applied` or `skipped { reason }`. Skipped steps are part
  of the record: a plan that silently dropped a step would misreport
  why the document stayed dirty.
- `side_effects` lists what the step changes beyond its own path
  (schema-default fills, renames that move a subtree). A step with
  side effects is never silently applied to a preserved subtree.
- Steps are bounded: `--max-steps` (capped at 64) selects how many
  applied directives the loop may use. A larger repair is split across
  runs rather than run unbounded.

Planning is deterministic for a fixed source, engine configuration
(profile, environment, resilient mode), and budget: directives are
emitted in diagnostic order and ties break on path. The planner runs
under the caller's analyzed profile so planning output cannot reveal a
value the run itself redacts.

## 3. Apply

Each step prepends its directive to the current source text, re-parses
with the same include and trust options, and re-evaluates with the same
engine configuration. A directive that does not parse, or that changes
nothing, is reported rather than retried in place. Applied directives
accumulate in reverse order so the newest edit is applied last, which
keeps composed renames and fills stable.

### 3.1 Atomic commit and receipt (L-P41)

`active repair` is validate-then-commit. The planner only selects steps it
has already re-evaluated (`Applied` + `safe`); before any write, the
composed source is re-parsed and re-evaluated (`source_verdict`).
`--output` is published through the atomic same-directory temp-and-rename
path: a refusal or a graceful abort leaves the destination byte-identical
and no `.tmp` file behind (a hard kill can leave a uniquely named temp,
never a partial destination). A partial selection (`--apply-step` /
`--cherry-pick`) commits exactly the selected steps and reports that
subset's own verdict; it warns that the plan is not clean instead of
claiming one. An in-place commit (`--output` == input) re-reads and
compares the input immediately before publishing; the window between that
check and the rename remains for an external writer that ignores the
command (no portable compare-and-swap exists), and the receipt's hashes
record what was verified.

`--receipt <path>` writes an L-T06 envelope v2 (atomically), on success
and on refusal:

- payload (a committed run only): `{goal, preserve?, steps,
  document_clean, source_sha256, result_sha256}`. Each step is `{kind,
  path, status, side_effects?, proof?}`; directives and values never
  appear (a directive can carry a value).
- `receipt.applied`: the committed (or refused/attempted) set, the same
  structural entries; `receipt.document_clean`: the resulting document's
  verdict, independent of the tool outcome (a committed partial repair
  can be clean=false).
- refusal: `outcome: failed`, `failure_class: validation` (or `other`),
  a null payload, no output written, and the receipt still records the
  attempted set.
- the output is published before the receipt. If the receipt cannot be
  written after a committed output, the command reports it and exits
  nonzero (the commit stands; its evidence is missing).

## 4. Terminal states

| State | Meaning |
|---|---|
| `clean: true` | no error- or warning-severity diagnostic remains |
| `stuck: true` | H006 — the same `(code, path)` set returned after another step, including an A→B→A oscillation over the run's history |
| `unrepairable: true` | H008 — remaining diagnostics have no safe mapping, or evaluation fails fatally before a step can apply |

`final_codes` carries validation-level outcomes as `CODE@path` and fatal
loop-terminal errors as bare codes (`E019`, parse codes); consumers
match on the code prefix. `findings` holds the terminal H006/H008 (and
any other loop diagnostics) as ordinary redacted `Diagnostic` values.

Both terminal states are first-class: an agent loop stops on `stuck` or
`unrepairable` instead of retrying blind, which is what keeps a repair
loop bounded and predictable.

## 5. Safety

- Repaired source can contain authored secret literals, so the repair
  model (`browser_model`) never carries `repaired_source`; the
  `--format json` plan carries a copy with known material redacted, and
  only `--output` writes the unmodified source, to a caller-chosen file.
- Diagnostics in the plan and browser model are value-free for secrets.
- Preserved subtrees are never touched, including through a rename
  that would move an ancestor.
- The planner and reducer never read host environment values; an
  environment read is a reference that evaluation resolves under the
  caller's engine, exactly as the ordinary run does.

## 6. Tests

`active-core/tests/repair.rs` drives the shipped planner and engine:
single- and multi-step plans, safe/unsafe classification, skipped-step
reasons, side-effect listings, goal behavior (validate, preserve-key,
minimize-diff), stuck detection including oscillation, unrepairable
outcomes, and deterministic plans. `active-cli/tests/cli.rs` covers the
CLI contract (`plan`, `--apply-step`, `--cherry-pick`, `--browser`,
`--output`) and the value-free browser model.
