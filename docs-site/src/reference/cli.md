# CLI reference

From `active --help` (verified against the shipped 1.4.0 binary).

## Evaluate and check

- `active eval <file>` - parse, evaluate, print redacted JSON.
  - `--path a.b[0]` emits only the demanded cone (budgets still guard the
    whole graph).
  - `--materialise-blobs` includes blob payloads; tainted blobs are refused
    with B003.
  - `--audit-trail FILE` writes a re-validatable, secret-redacted audit
    document (counts, codes, content hash).
  - `--sandbox strict|data|agent|trusted` selects a budget preset plus the
    tool subset the profile offers.
- `active check <file>` - errors, cycles, unresolved names; `OK` and exit 0
  when clean. `--resilient` collects warnings alongside the tree; any Error
  still exits 1. `--budget-profile strict|interactive|trusted` selects a
  named preset.

Both accept `--trace` (value-free JSONL on stderr), `--profile NAME`, and the
full `--max-*` budget family.

## Inspect and query

- `active graph <file> [--format text|mermaid|dot|json]` - dependency graph.
- `active impact <key> <file>` / `active why <key> <file>` - transitive
  dependents and dependencies.
- `active graph query <file> <kind> ...` - `dependents`, `dependencies`,
  `shortest-path`, `secret-cone`, `ca-above-size`.
- `active inspect --from <file>` - versioned JSON schema introspection
  (INSPECT_VERSION 2): fields, types, constraints, defaults, descriptions,
  deprecations, `removed_in`, evolution metadata, and mode.
- `active compatibility <old> <new> [--format text|json]` - C040-C044
  findings with an E040 summary; exits nonzero on breaking changes.
- `active capability [--require-major N] [--check-doc FILE] [--profile NAME]`
  - the descriptor, a document-capability handshake, or one sandbox profile.

## Repair and explanation

- `active explain --from <file> [--format text|json]` - deterministic
  per-diagnostic guidance.
- `active patch --from <file> [--format text|json]` - minimal `@rename` /
  `@patch` directives for mechanical diagnostics.
- `active repair --from <file> [--goal validate|preserve-key|minimize-diff]
  [--preserve <path>] [--max-steps N] [--output FILE] [--format text|json]`
  - multi-step plan applied through the patch engine; H006 stuck and H008
  unrepairable are terminal states.

## Convert, generate, document

- `active convert <file> --to json|yaml|toml` - evaluated exports; secrets
  redact uniformly.
- `active import --from json|yaml|toml|jsonschema <file>` - best-effort
  ingestion with lossy-corner diagnostics.
- `active generate --from <schema> --to jsonschema|openapi|typescript|go|rust|python [--name Name]`
  - typed artifacts from an Active schema.
- `active doc --from <file> --to markdown|html|man [--title Title]` -
  schema + `@intent` + comments to documentation.
- `active lock` - reproducible lockfile (content hash + provenance).

## Format and lint

- `active fmt [--style brace|indent|preserve] [--write] [--check] <file>...`
  - canonical formatting; `--check` is the CI mode.
- `active lint [--disable-lint <rule>]... <file>...` - rules: `unused-lets`,
  `depends-on`, `complexity`, `hardcoded-secrets`, `deprecated`.

## Security workflow

- `active keygen`, `active sign <file> --key <key> --kid <id>`,
  `active verify <file> --trust-keys <toml>` - Ed25519 detached signatures
  for signed overlays.

## Editors

`active-fmt` and `active-lsp` (stdio Language Server Protocol 3.17) ship as
separate binaries installing from `active-fmt` / `active-lsp`.
