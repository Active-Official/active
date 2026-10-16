# Schema library

The standard schema library (`schemas/`, versioned with the
language) ships reusable schemas and include fragments. Pull any
file in with a literal include — relative to the including
document (vendor `schemas/` next to your configs; no registry, no
download step), e.g. `include "schemas/http-server.solid"`.

| File | Covers | Required (no default) |
|---|---|---|
| `http-server.solid` | `host`, `port`, `tls`, `timeout`, `max_requests` | — (all defaulted) |
| `postgres.solid` | `host`, `port`, `database`, `user`, `password`, `pool` | `host`, `database`, `user`, `password` |
| `observability.solid` | `level`, `metrics`, `sample_rate`, `endpoint` | `level` |
| `feature-flags.solid` | `flags` of `{name, enabled, rollout}` | flag entries |
| `ai-agent.solid` | `model`, `temperature`, `max_tokens`, `retries` | `model` |

Every file follows the library rules (`spec/schema-library.md`):
closed tables, a descriptive comment on every field, defaults where
universal (`T004` naming the key otherwise), units for measurable
quantities, and no secrets inside — `Secret`-typed fields accept
your tainted values and redact like any other secret. Each schema
is validated by the test suite against good and bad sample
documents, and the field lists above are pinned by a docgen test
that renders every schema to Markdown: the table cannot drift
from the enforced contract.
