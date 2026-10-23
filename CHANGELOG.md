# Changelog

All notable changes to the Active language, toolchain, and
specification. (Entries below the rename keep their original
SOLID-era names: that history is accurate as written.) Versions
follow SemVer with a unified workspace version (spec/packaging.md §1).

## [Unreleased]

### Master Program (CA, SC, R, PE, M, GQ, CAP)

Additive surface, no version bump: content-addressed Blob values with
BLAKE3 identity and B00x diagnostics (`blob()`, `Blob` schema type,
`--materialise-blobs`), schema evolution metadata and the
`active compatibility` CLI (C04x/E040), multi-step `active repair`
with H006/H008 terminal states, demanded-cone `eval --path` plus FFI
`active_value_project` and LSP incremental memo, Config/Data mode
flag with W013 progressive warnings and E024, decidable graph
queries (`active graph query`), in-document `active_capabilities`
with E025, the capability handshake (`--check-doc`) and sandbox
profiles (`--sandbox`, 4 profiles). Capability descriptor grows 10
→ 12 tools plus `profiles` and new `features` entries; INSPECT v2.
Consumers pinning exact tool/feature sets should expect additions —
any incompatible change ships under a new major version per the
stability policy (spec/packaging.md §7, docs/version-and-abi-policy).

### SOLID → Active rename

The language is renamed SOLID → Active across code, packages, docs,
and fixtures: crates `active`, `active-core`, `active-cli`,
`active-fmt`, `active-lsp` (+ `active-fuzz` harness); binaries
`active`, `active-fmt`, `active-lsp`; library `active_core`
(`ActiveType`, …) with the `active` facade crate; file extension
`.solid` → `.active`; editors, workflows, and release artifacts
follow. GitHub URLs and prior release notes keep their original
names (redirects/history).

## [1.4.0] — 2026-10-20

Embedding release, published to crates.io (`active`, `active-core`,
`active-cli`, `active-fmt`, `active-lsp`, plus first publishes of
`active-ffi` and `active-wasm`). New public crates are new
functionality, backward compatible: the frozen C ABI (opaque handles,
structured diagnostics, budgets, secret discipline, cbindgen header
with CI drift check) and the zero-import WebAssembly module (browsers,
Node, Deno, Bun via the JS/TS shim; Go over wazero with no cgo),
both running the same conformance manifest as the reference
implementation. Also aboard: the Miri-hardened wall-clock gate and
single-ownership FFI test host (full Miri 21 of 21 clean, zero
findings), the WASM-powered playground with headless-browser proof,
the enterprise README rewrite with Figma-composed banner and graphics,
and the secret taint plus dependency graph documentation. No language
changes: the 1.0 surface stays frozen.

## [1.3.0] — 2026-10-18

Units Phases B+C (AI-age trio + domain SI): token counts
(`tokens ktokens Mtokens`, glued spelling only), inference throughput
(`tok/s`), compute (`FLOPS TFLOPS PFLOPS`); pressure (`Pa bar psi`),
electrical (`V A W`, cross-dimension refuses), energy (`J kWh`),
force (`N`); angles (`deg rad`, radians canonical), rotation (`rpm`
retained); length (`meter km`, long-form only — `m` stays minutes);
mass (`kg g`); large bytes (`PiB EiB`, saturating); temperature
(`K Cel Fahr`, affine kelvin canonical, parser folds unary minus into
affine literals so the -40 crossover holds) and decibels (`dB`,
ordering-only — affine/log arithmetic refused). Same-suffix arithmetic
for all retained quantities. No changes to any previously-parseable
source (full suite green throughout).

## [1.2.0] — 2026-10-18

Units Phase A (Tier 0 gaps): percent `%` (dimensionless ratio, 100% = 1.0,
percent-form export, no implicit base, no clamping); frequency
`Hz kHz MHz GHz THz` (f64 hertz canonical, full arithmetic, case-sensitive
SI spellings); bits + data rates `b Kb Mb Gb bps Kbps Mbps Gbps MB/s`
(canonical bits / bits-sec, `MB/s` converts through megabytes, bits-bytes
mixing refused); `week` (exact 604800000ms) + `sec`/`min` long aliases
(canonical `s`/`m` respell; plurals refused); same-suffix arithmetic for
all retained quantities (rates included), cross-suffix stays dimensional
TypeError; explicit unary negation preserved on quantities. No changes to
any previously-parseable source (full suite green throughout).

## [1.1.0] — 2026-10-17

Hardening release, published to crates.io (`active`, `active-core`,
`active-cli`, `active-fmt`, `active-lsp`). No language changes:
the 1.0 surface is frozen. Tested brutally since 1.0.0 — lexer/parser
fuzz harness (no panics, no hangs), property-test invariants
(determinism, cycle detection, secret non-leakage), execution
budgets, and a 1500+ case research battery pinning YAML/TOML/JSON
failure lineages to diagnostics. Repository home moves to
`Active-Official/active` (mirror); personal repo renamed
`RatioArtificiosa/active-lang` (redirects preserved).

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
