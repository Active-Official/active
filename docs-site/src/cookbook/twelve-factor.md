# 12-factor application

Twelve-factor config is environment by definition. ACTIVE reads it
from a snapshot: `env.NAME ?: default` resolves variables injected
at startup (never live I/O during evaluation), with written
defaults for local runs:

```active
{{#include ./twelve-factor.active}}
```

`DATABASE_URL` unset locally? The default applies and evaluation
stays deterministic. Set in production, the injected snapshot value
wins for that run. Secrets ride the same path via
`secret(env.TOKEN)` — tainted, redacted, never logged.
