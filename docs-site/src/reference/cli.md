# CLI reference

From `solid --help` (verified against the shipped binary):

- `solid eval <file>` — parse, evaluate, print JSON.
- `solid check <file>` — errors, cycles, unresolved names; `OK`
  and exit 0 when clean. `--resilient` collects warnings alongside
  the tree, but any Error diagnostic still exits 1.
- `solid graph <file>` — dependency graph (text, Mermaid, DOT, JSON).
- `solid impact <key>` / `solid why <key>` — transitive dependents
  (what breaks if a value changes) and dependencies, as single-file
  key queries.
- `solid convert <file> --to json|yaml|toml` — evaluated exports
  (secrets redact uniformly). `solid import --from … <file>` —
  best-effort ingestion with lossy-corner diagnostics.
- `solid generate --from <schema> --to <target>` — typed bindings,
  where target is `jsonschema`, `openapi`, `typescript` (or `ts`),
  `go`, `rust` (or `serde`).
- `solid doc` — schema + `@intent` + comments to Markdown, HTML, man.
- `solid lock` — reproducible lockfile (content hash + provenance).
- `solid keygen|sign|verify` — Ed25519 overlay signing workflow.
- `solid fmt [--check]` — canonical formatting (CI mode included).
- `solid lint` — independent rules (unused lets, missing
  `depends_on`, complexity, secret heuristics, deprecations).

`solid-fmt` and `solid-ls` (stdio Language Server Protocol 3.17)
ship as separate binaries installing from `solid-fmt` / `solid-ls`.
