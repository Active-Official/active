# Active — repository instructions for Copilot

This repository contains **Active**, a configuration and data language (TOML/YAML/
JSON class) implemented in Rust as a Cargo workspace. The language is designed
to be safe for both humans and AI agents: deterministic, hermetic, bounded,
observable, dual-syntax, secure-by-default.

## Workspace layout

- `active-core/` - parser (lexer + recursive descent), IR, dependency graph,
  evaluator, types/units, diagnostics, healer, converters, blob identity.
- `active-cli/` - the `active` binary: eval, check, fmt, lint, graph/impact/
  why/query, convert, import, inspect, generate, doc, lock, explain, patch,
  repair, compatibility, capability, keygen/sign/verify.
- `active-fmt/` - dual-syntax formatter (production, `--check` included).
- `active-lsp/` - Language Server Protocol 3.17 implementation (hover,
  definition, references, rename, completion, code actions, inlay hints,
  semantic tokens, diagnostics, graph methods).
- `active-ffi/` - frozen C ABI v1 (opaque handles, Allocator Model A).
- `active-wasm/` - zero-import WebAssembly module plus the hand-written shim.
- `bindings/` - Tier-1 (Go external, Python, JS/TS, Rust facade) and the
  Tier-2 matrix; `bindings/README.md` is the support matrix.
- `conformance/` - manifest, cases, harness, normalization contract, evidence.
- `agent-kit/` - capability descriptor, eval harness, vectors, reference loops.
- `schemas/` - standard schema library; `editors/` - VS Code and Neovim.
- `examples/` - `.active` example files; both MUST keep evaluating cleanly.
- `spec/` - the public language specification and contracts (normative).
- `tests/`, `fuzz/`, `benches/`, `tools/` - corpora, fuzzing, benchmarks,
  helper tools.

## Engineering rules (non-negotiable)

1. **Determinism and hermeticity.** Evaluation performs no arbitrary I/O, no
   shell, no network. The only external inputs are declared env vars and the
   controlled include graph. Never introduce side effects in `active-core`.
2. **Bounded evaluation.** Hard limits on node count, depth, array length,
   wall-clock time. Never add constructs that can loop or recurse unboundedly.
3. **Secrets.** `secret(...)` values are redacted on every export path
   (`***SECRET***`), and secret material must never appear in diagnostics,
   logs, or error messages.
4. **Zero warnings.** `cargo build --workspace`, `cargo clippy --workspace
   --all-targets`, and `cargo fmt --check` must all stay clean. CI enforces
   this (`.github/workflows/ci.yml`).
5. **Rust style.** Edition 2021, rustfmt-clean, clippy-clean. Prefer
   `IndexMap` for order-preserving tables; `petgraph` for the dependency DAG.
6. **MSRV 1.85+** — the resolved dependency graph includes edition-2024
   crates (indexmap 2.14, clap 4.6). Do not claim a lower MSRV.

## Stub and pending-work markers

Any deliberately unfinished code is marked with a searchable tag naming the
checklist item that will complete it:

```rust
// TODO(Active C-040): production dual-style formatter
```

Find all pending items with:

```
rg "TODO\(ACTIVE"
```

Never leave an unmarked stub or a vague `// TODO:` without a checklist id.

## Language design authority

The normative public contract is `spec/` (grammar, specification,
validation, budgets, secrets, blob, mode, partial evaluation, schema
evolution, graph, linter, formatter, LSP, packaging, threat model,
conformance). The Master Program working documents are local-only and never
part of the repository. When code and `spec/` disagree, that is a bug: fix
the code or the spec in the same change, and never let them drift.

**Core principles (from `spec/specification.md` and `spec/threat-model.md`):**
deterministic (same input → same value tree + diagnostics), hermetic (no
arbitrary I/O, shell, or network during evaluation), bounded (hard limits on
nodes, depth, size, wall time), observable (graph, diagnostics, audit trail,
opt-in tracing), dual-syntax, secure by default (secrets tainted and
redacted; env read-only).

**Healing confidence policy:** ≥0.95 auto-apply; 0.85–0.94 auto-apply with
warning; 0.70–0.84 resilient mode only; <0.70 never auto-apply.

**Permanent non-goals:** no Turing-complete computation, no arbitrary side
effects, no dynamic code loading or eval of strings, no unrestricted implicit
coercion, not a general data-transformation language.
