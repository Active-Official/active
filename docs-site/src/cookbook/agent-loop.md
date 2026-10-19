# Agent loop: generate, check, repair, repeat

The canonical loop for agents producing Active. Copy it verbatim into
automation; every step below is a real command with a real exit code.

```text
generate Active
  -> active check file.active            (exit 0 means done)
  -> on nonzero: read code + span, repair that location only
  -> repeat, max 5 repair iterations, 60s wall timeout
  -> active eval file.active             (only after check is green)
```

## Strict profile mapping (G-007)

Default `active check` is the strict profile: warnings fail the run and no
healing applies, so agents face every issue their output created.
`--resilient` enables medium-confidence healing repairs for interactive use;
automation stays on default check. Budget flags (`--max-nodes`,
`--max-wall-ms`, and siblings) tighten limits per run; agents pass the same
budgets the host enforces so a bad generation dies fast instead of hanging
a worker.

## Repair rules for agents

1. Repair only the spanned location; never rewrite the whole file (patches
   beat rewrites; rewrites reintroduce fixed bugs).
2. Respect `@ai_immutable`: a patch touching it fails E019 by design.
3. Never invent colon-mapping or implicit booleans; unquoted values never
   coerce, and the loop must not "fix" a W001 by quoting what should be a
   real reference.
4. Secrets stay `secret(...)`; interpolation of a secret fails S004, which
   is the language protecting the log, not an error to route around.
5. Cap iterations: after 5 failed repairs, escalate to a human with the full
   diagnostic transcript instead of thrashing.

## Worked repair (from Kit C)

```text
temperature = 5.0
# [Error] T003: schema violation at 'agent.temperature': above maximum
# fix: temperature = 0.7 -> active check exits 0 -> active eval prints JSON
```
