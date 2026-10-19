# ACTIVE standard schema library (C-062, spec/schema-library.md)

Versioned with the language (same workspace version): `http-server`,
`postgres`, `observability`, `feature-flags`, and `ai-agent` schemas
with a descriptive comment on every field, closed tables, and
defaults where a universal value exists.

Consume by vendoring this directory next to your documents and
including downward (no flags, no escapes):

```active
include "schemas/http-server.active"
```

Fields with no sane default (passwords, hostnames, model names)
stay required and report `T004` naming the key until set. No
secrets live in this directory (enforced by test); `Secret`-typed
fields accept your tainted values and redact like any other secret.
