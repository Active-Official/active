# Repair browser and partial application (TOOLING §7)

The repair surface is headless: one JSON model, explicit apply
semantics, and terminal states shared with `active repair`.

## 1. Browser model (REPAIR-UI.001)

`active repair --from <file> --browser` prints:

```json
{
  "diagnostics": [{"code", "severity", "message", "path", "span", "file"}],
  "plan": {"goal", "steps", "clean", "stuck", "unrepairable",
           "final_codes", "findings"}
}
```

- The model never includes `repaired_source`: an authored literal
  secret lives in source text and a display/log model must not echo it.
- Diagnostics are value-free for secrets (T002 renders `got secret`);
  plain authored values may appear in schema messages, as everywhere
  else.
- `plan` is the shipped `RepairOutcome`, so the browser and the CLI can
  never disagree.

Filters (REPAIR-UI.002) select which diagnostics the model shows:

| Flag | Keeps |
|---|---|
| `--filter-code CODE` (repeatable) | diagnostics with that stable code |
| `--filter-severity LEVEL` (repeatable) | diagnostics at that severity (`error`, `warning`, `info`, `hint`) |
| `--filter-path PREFIX` (repeatable) | diagnostics at that path or below it (segment boundary, never a sibling whose name starts with the same text) |

Filters intersect: supplying more than one narrows the list. An empty
filter keeps everything, and an unknown severity is a usage error. The
plan is never filtered: its step indexes are what `--apply-step` and
`--cherry-pick` address, so hiding steps would change apply semantics.
`--filter-*` requires `--browser`.

## 2. Apply semantics (REPAIR-UI.003)

| Flag | Applies |
|---|---|
| (none) | every safe planned step; writes only when the result is clean |
| `--apply-step N` | safe steps with plan index <= N |
| `--cherry-pick <prefix>` | safe steps whose target path is `<prefix>` or lies below it (a sibling whose name merely starts with the same text is not selected) |

Partial application composes the selected directives in plan order and
prepends them to the original source (the same composition the loop
uses). `--output` writes a partial result with a warning when the plan
is not yet clean; an empty selection exits 1. The report describes the
subset that was written: `clean` and `final_codes` come from evaluating
that source (the loop's conventions: parse failures carry bare codes,
evaluation diagnostics carry `CODE@path`, and only error/warning
severities count), and the full plan's `stuck`, `unrepairable`, and
`findings` are cleared because they describe a trajectory the subset did
not run. `stuck` (H006) and `unrepairable` (H008) remain first-class
fields of a full plan.

## 3. Step proof sketches (L-P53)

Every applied step carries a `proof` object: the mechanical check the
loop ran against the evaluation that followed the step. It is a
*sketch*, not a formal proof (L-P50 is the formal track): three claims,
each checked by re-evaluation.

```json
"proof": {
  "addresses": ["T004@port"],
  "removes": ["T004@port"],
  "introduces": [],
  "changes_elsewhere": [],
  "verified": true,
  "reason": null,
  "checked_paths": 2
}
```

- `addresses` names the blocking diagnostics (`CODE@path`) at the step's
  own path, before the step. A rename's endpoints both count.
- `removes` is the set of blocking codes that disappeared; `introduces`
  the set that appeared. A verified step removes at least one of its
  `addresses` and introduces none: clearing some other diagnostic is not
  this step's proof.
- `changes_elsewhere` names evaluated paths outside the step's blast
  radius (its path plus rename endpoints, and any container the step had
  to create or grow) whose value changed. Both trees are **redacted**
  under the same profile the planning evaluation used before they are
  compared, so material never enters the comparison. The comparison is
  bounded: at most 65,536 visited values, 512 levels of depth, and 16
  reported paths. Exceeding a bound fails the proof instead of
  truncating it (a bound failure reports no partial list).
- `verified` is true only when every condition held; `reason` states the
  first that did not. A step whose re-evaluation could not run (the
  repaired source does not parse, or evaluation fails fatally) carries
  `verified: false` with that reason: unproven, never promoted.
- The check uses the same inputs as the repair itself: the caller's
  engine (budget, profile override, environment) and the same include
  context, so the proof is about the document being repaired.
- The record holds codes, paths, and counts only, never a value.
  Material is opaque: two secret values are never compared, and two
  tainted blobs compare by identity, so a proof cannot confirm a guess.
- A skipped step and a plan that applied nothing carry no proof. A
  partial application clears the proof of every step it demotes to
  skipped, because the check describes a document the run did not write.
- The check runs under the same parse and evaluation inputs as the
  repair itself: the caller's engine (budget, profile override,
  environment), the same include policy, and the same strict parse
  options (no CLI limit overrides the loop does not apply). A proof about
  a different document is not a proof.

The plan record (the JSON output and the browser model, which embeds the
same `RepairOutcome`) is the repair's durable receipt: an approver
archives it and sees the verification, not just the suggestion.

## 4. Agent-kit compatibility

Reference loops consume the same diagnostics and codes; `final_codes`
uses the `CODE@path` convention for evaluation diagnostics, so a loop can
branch on the same terminal states without parsing prose. A blocking
diagnostic with no path keeps the bare code, and parse failures report
bare codes; both match the loop's own convention. The step budget is
capped at 64 applied steps at the library boundary (the CLI rejects a
larger `--max-steps`), so no caller can request an unbounded plan.

## 4. Tests

Core: browser model pairs diagnostics with the plan and never includes
source; `apply_selected` honors index and prefix bounds (path-boundary
prefixes, never a sibling name) and leaves the source untouched for an
empty selection; every applied step's proof is checked against the
after-state, a shadowed change elsewhere fails it, the wrong diagnostic
removed fails it, a rename may move its endpoints, a container the step
creates is in scope, material stays opaque (a swapped tainted blob is
still detected by identity), and the bounds fail instead of truncating.
CLI: `--browser` shape, `--apply-step`, `--cherry-pick`, the `proof`
object in `--format json` plus the `proof:` line in `--format text`, and
a partial run clearing demoted proofs and the full plan's terminal state.
