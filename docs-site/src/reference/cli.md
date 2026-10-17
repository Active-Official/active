# CLI reference

From `ACTIVE --help` (verified against the shipped binary):

- `ACTIVE eval <file>` — parse, evaluate, print JSON.
- `ACTIVE check <file>` — errors, cycles, unresolved names; `OK`
  and exit 0 when clean. `--resilient` collects warnings alongside
  the tree, but any Error diagnostic still exits 1.
- `ACTIVE graph <file>` — dependency graph (text, Mermaid, DOT, JSON).
- `ACTIVE impact <key>` / `ACTIVE why <key>` — transitive dependents
  (what breaks if a value changes) and dependencies, as single-file
  key queries.
- `ACTIVE convert <file> --to json|yaml|toml` — evaluated exports
  (secrets redact uniformly). `ACTIVE import --from … <file>` —
  best-effort ingestion with lossy-corner diagnostics.
- `ACTIVE generate --from <schema> --to <target>` — typed bindings,
  where target is `jsonschema`, `openapi`, `typescript` (or `ts`),
  `go`, `rust` (or `serde`).
- `ACTIVE doc` — schema + `@intent` + comments to Markdown, HTML, man.
- `ACTIVE lock` — reproducible lockfile (content hash + provenance).
- `ACTIVE keygen|sign|verify` — Ed25519 overlay signing workflow.
- `ACTIVE fmt [--check]` — canonical formatting (CI mode included).
- `ACTIVE lint` — independent rules (unused lets, missing
  `depends_on`, complexity, secret heuristics, deprecations).

`active-fmt` and `active-lsp` (stdio Language Server Protocol 3.17)
ship as separate binaries installing from `active-fmt` / `active-lsp`.
