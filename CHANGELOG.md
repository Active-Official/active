# Changelog

All notable changes to the SOLID language, toolchain, and
specification. Versions follow SemVer with a unified workspace
version (spec/packaging.md §1).

## [1.0.0] — 2026-10-16

First stable release. The C-090 readiness review
(spec/readiness-1.0.md) passes all ten success criteria; the
checklist (C-010 through C-090 plus C-070) is complete; the
conformance suite is green and this repository is the declared
reference implementation.

Since 0.1.0-alpha.1: JSON keys serialize in source binding order
(`preserve_order`; conformance sidecars re-emitted), comment
attachment is file-aware across includes, schema entries take
per-field documentation comments, `solid doc` accepts
`--resilient`/`--allow-parent-escapes`, and the docs site gained
Terraform/CI cookbook entries.

## [0.1.0-alpha.1] — 2026-10-16

First public alpha of the complete roadmap state.

- Language: dual syntax (braces, indentation, mixed, dotted
  headers), interpolation with dependency edges, multi-line strings
  and semantic blocks, literal-path includes, profiles, structural
  patches, full schema language with strict/resilient validation,
  annotations (`@intent`, `@constraint`, `@ai_immutable`),
  transitive secret taint, execution budgets.
- Healing: three-phase engine (syntactic, schema-key H005,
  type-coercion H007) with the published confidence policy.
- Tooling: `solid` CLI (eval, check, graph, impact, why, convert,
  import, generate, doc, lock, keygen, sign, verify, fmt, lint),
  `solid-fmt`, `solid-ls` (LSP 3.17 over stdio), JSON/YAML/TOML
  converters, schema codegen (JSON Schema, OpenAPI, TypeScript, Go,
  Rust serde), Markdown/HTML/man docgen, Mermaid/DOT/JSON graphs.
- Reproducibility: hermetic evaluation, SHA-256 redacted-tree
  hashing, TOML lockfiles, Ed25519 signed overlays.
- Performance: 10k-node eval under 50 ms p99 with fingerprint
  incremental re-evaluation; CodSpeed-tracked criterion benches.
- Distribution: `solid-lang-core`, `solid-cli`, `solid-fmt`,
  `solid-ls` packaging metadata; 6-target release matrix; VS Code
  extension; Neovim plugin; standard schema library
  (`schemas/`); mdBook documentation site.
- Conformance: `tests/spec/` (13 pairs) is the official suite; this
  repository is the reference implementation for 1.0.
