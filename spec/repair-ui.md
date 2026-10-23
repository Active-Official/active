# Repair browser and partial application (TOOLING §7)

The repair surface is headless: one JSON model, explicit apply
semantics, and terminal states shared with `active repair`.

## 1. Browser model (REPAIR-UI.001)

`active repair --from <file> --browser` prints:

```json
{
  "diagnostics": [{"code", "severity", "message", "path", "span"}],
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

## 2. Apply semantics (REPAIR-UI.003)

| Flag | Applies |
|---|---|
| (none) | every safe planned step; writes only when the result is clean |
| `--apply-step N` | safe steps with plan index <= N |
| `--cherry-pick <prefix>` | safe steps whose target path starts with the prefix |

Partial application composes the selected directives in plan order and
prepends them to the original source (the same composition the loop
uses). `--output` writes a partial result with a warning when the plan
is not yet clean; an empty selection exits 1. `stuck` (H006) and
`unrepairable` (H008) remain first-class fields in the plan.

## 3. Agent-kit compatibility

Reference loops consume the same diagnostics and codes; `final_codes`
uses the `CODE@path` convention, so a loop can branch on the same
terminal states without parsing prose.

## 4. Tests

Core: browser model pairs diagnostics with the plan and never includes
source; `apply_selected` honors index and prefix bounds and leaves the
source untouched for an empty selection. CLI: `--browser` shape,
`--apply-step`, and `--cherry-pick` against the real binary.
