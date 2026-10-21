# Schema library

The standard schema library (`schemas/`, versioned with the
language) ships reusable schemas and include fragments. Pull any
file in with a literal include — relative to the including
document (vendor `schemas/` next to your configs; no registry, no
download step), e.g. `include "schemas/http-server.active"`.

| File | Covers | Required (no default) |
|---|---|---|
| `http-server.active` | `host`, `port`, `tls`, `timeout`, `max_requests` | — (all defaulted) |
| `postgres.active` | `host`, `port`, `database`, `user`, `password`, `pool`, `contact` | `host`, `database`, `user`, `password`, `contact` |
| `observability.active` | `level`, `metrics`, `sample_rate`, `endpoint` | `level` |
| `feature-flags.active` | `flags` of `{name, enabled, rollout}` | flag entries |
| `ai-agent.active` | `model`, `temperature`, `max_tokens`, `retries` | `model` |
| `api-service.active` | `base_url`, `version`, `build`, `channel` | `base_url`, `version`, `build` |
| `tls-terminator.active` | `tls`, `cert` (requires `tls`), `key` (requires `tls`) | conditional (see `requires`) |

Every file follows the library rules (`spec/schema-library.md`):
closed tables, a descriptive comment on every field, defaults where
universal (`T004` naming the key otherwise), units for measurable
quantities, and no secrets inside — `Secret`-typed fields accept
your tainted values and redact like any other secret. Each schema
is validated by the test suite against good and bad sample
documents, and the field lists above are pinned by a docgen test
that renders every schema to Markdown: the table cannot drift
from the enforced contract.
