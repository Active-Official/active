# From TOML

ACTIVE is a superset of TOML's shape with stricter evaluation.
Mechanical migration first, improvements after.

TOML in:

```toml
[server]
host = "localhost"
port = 8080
```

ACTIVE out (`ACTIVE import --from toml` produces the equivalent
source, or write it directly):

```active
{{#include ./toml.active}}
```

What changes on day one: nothing — tables, keys, and scalars map
1:1. What you gain on day two: references (`port = base + 80`
instead of repeating the number), units (`timeout = 30s` instead
of `timeout_seconds = 30`), closed-table schemas that reject typos,
and profiles that replace one-file-per-environment. Dotted headers
(`[server.tls]`) work in both.
