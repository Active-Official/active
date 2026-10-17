# Multi-region setup

Base values plus per-region overlays in a single file. Profiles
deep-merge tables and last-win scalars; select with
`active_profile` (checked in) or `--profile` (per deploy):

```active
{{#include ./multi-region.active}}
```

`ACTIVE eval --profile us-east` previews the region without editing.
Note the output keeps the file's own `active_profile` key (`"eu-west"`)
alongside the us-east values: the key records what the file declares,
the override only changes which profile merges. Automation should
read values, not that key (the lockfile records the effective
profile separately).
Genuine conflicts (same key, incompatible types across overlays)
report `W003` diagnostics instead of silently picking one.
