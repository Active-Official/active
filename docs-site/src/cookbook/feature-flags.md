# Feature flags

Flags are typed data, not stringly comments. A closed table rejects
typos: strict errors with a suggestion (`did you mean …?`), and
under resilient mode a confident match renames the key with an
`H005` warning. Ranges keep rollouts sane:

```active
{{#include ./feature-flags.active}}
```

Roll out by editing `rollout_pct`; the schema guarantees 0–100.
Retire a flag by deleting its key — `ACTIVE why` shows what reads
it, so cleanup is a query, not an archaeology dig.
