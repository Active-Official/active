# Migration Kit A: 12-factor service (YAML to Active)

A complete, copy-pasteable migration for a typical service config: environment
fallbacks, units, schema, and secrets. Files: `kit-a-before.yaml` (the YAML
you have) and `kit-a-after.active` (the Active you want). Both ship in this
directory; the `.active` file is executed by the docs test suite, so it can
never rot.

## Step 1: see the lies

```sh
active import -f yaml migration/kit-a-before.yaml
```

The import succeeds, and that is the point: the YAML parses fine while
meaning three wrong things. YAML 1.1 reads `country_enabled: NO` as boolean
`false` (verified against PyYAML); `timeout: 30` carries no unit; and
`db_password` travels in cleartext through every log and export. The Active
import keeps `NO` a string and warns W001 instead of folding it.

## Step 2: evaluate the after file

```sh
active check migration/kit-a-after.active
active eval migration/kit-a-after.active
```

`check` exits 0. `eval` prints the value tree with `timeout` normalized to
`{"ms": 30000}` and `db_password` rendered as `***SECRET***`. Host, port,
and pool fall back through `env.HOST ?:`, `env.PORT ?:`, `env.POOL ?:`, so
the file evaluates identically with an empty environment (twelve-factor
defaults, not accidents).

## Step 3: break it on purpose (common mistakes)

With the kit schema in place, these fail loud with spans:

```text
timeout = 30        # T002: schema violation at 'timeout': expected Duration, got number
port = 0            # T003: schema violation at 'server.port': below minimum
msg = "hi ${pw}"    # S004: secret used in unsafe context (pw is a secret)
```

Each diagnostic names the exact line and column. Fix the value, re-run
`active check`, repeat. This loop is the whole migration workflow.

## Silent-to-explicit checklist

| Before (silent) | After (explicit) |
|---|---|
| `NO` reads as `false` (YAML 1.1) | `NO` warns W001; write `false` or `"NO"` |
| `timeout: 30` unitless | `timeout = 30s` with `Duration` schema |
| Password in cleartext | `secret(...)` with `***SECRET***` exports |
| Missing env means crash or empty | `env.VAR ?: default` with schema minimums |
| Wrong port ships quietly | `Int(min = 1)` rejects `port = 0` with a span |

## Differential mode (prove the migration in CI)

Evaluate the hand-written Active against the YAML-derived import and diff
the value trees (modulo documented import normalization: key sanitization
with W007, datetimes as strings with W008, non-finite floats as null with
W010). Any other difference is a migration bug, not a dialect difference:

```sh
active import -f yaml kit-a-before.yaml > /tmp/before.active
active eval kit-a-before.active > /tmp/before.json
active eval kit-a-after.active > /tmp/after.json
# diff /tmp/before.json /tmp/after.json, allowing the documented mappings
```
