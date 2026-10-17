# ACTIVE

**Symmetric · Overlay-ready · Loop-proof · Intent-driven · Deterministic**

[![crates.io](https://img.shields.io/crates/v/active-core)](https://crates.io/crates/active-core)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE-MIT)

ACTIVE is a modern configuration and data language designed to be safer than YAML, more expressive than TOML, more human-friendly than JSON, and excellent for both humans and AI agents.

This repository contains the **production implementation**: parser, IR,
dependency graph, evaluator, units, string interpolation, schema
validation, CLI, formatter, converters, documentation generator, and
language server.

## Features in 1.1

- `let` bindings with lexical references
- `env.VAR ?: default`
- `secret(...)` (redacted on output)
- Full string interpolation `"…${name}…"`
- First-class units: `5s`, `512MiB`, `15m`, `100/req/s` …
- Tables (`[name]`), arrays, inline tables
- Arithmetic and boolean operators with correct precedence
- Dependency graph + topological evaluation + cycle detection
- JSON and simple TOML conversion
- Structured diagnostics
- CLI: `eval`, `check`, `graph`, `convert`, `fmt`, `lint`
- Dual-syntax formatter (`active fmt`: indent, bracketed, preserving)
- Language server (`active-lsp`: hover, go-to-definition, completions,
  diagnostics, inlay hints, code actions)

## Install

```bash
cargo install active-cli active-fmt active-lsp   # needs Rust 1.85+
```

Release archives (Linux/macOS/Windows × x86_64/ARM64, with SHA256SUMS
and build attestations) are attached to
[GitHub releases](https://github.com/Active-Official/active/releases).
Editors: `editors/vscode` (VS Code / Cursor / VSCodium vsix) and
`editors/neovim` (runtimepath plugin + `active-lsp`). Details and the
crates.io procedure: `spec/packaging.md`.

## Quick start

```bash
cargo build -p active-cli --release   # needs recent stable Rust (1.85+)

# simple demo
./target/release/active eval examples/demo.active

# comprehensive example (units + interpolation + secrets + arrays)
./target/release/active eval examples/complete.active

./target/release/active check examples/complete.active
./target/release/active graph examples/complete.active
./target/release/active convert examples/complete.active --to json
```

## Project layout

```
ACTIVE/
├── Cargo.toml
├── README.md
├── docs/DESIGN.md
├── examples/
│   ├── demo.active
│   └── complete.active          ← richer example
├── active-core/                 # parser, IR, evaluator, types, diagnostics
│   └── src/
│       ├── parser/
│       │   ├── lexer.rs        # full lexer with units & operators
│       │   └── mod.rs          # full recursive-descent parser
│       ├── ir.rs
│       ├── eval/mod.rs
│       ├── types.rs            # units + normalization
│       ├── diag.rs
│       ├── heal.rs
│       └── convert/mod.rs
├── active-cli/
├── active-fmt/
└── active-lsp/
```

## Parser surface (current)

```
let name = expr
[table]
key = expr

expr  :=  or
or    :=  and (|| and)*
and   :=  equality (&& equality)*
…     :=  + - * /   == != < <= > >=   unary - !
primary := literal | ident | env.NAME ?: default
         | secret(…) | array | inline-table | ( expr )
         | number unit?

unit  :=  ns us ms s m h d | B KiB MiB GiB | /s /m /h | req/s …
```

String interpolation: any `"…${ident}…"` becomes an `Interpolate` node.

## Status

1.1 is the current release ([crates.io](https://crates.io/crates/active-core):
`active`, `active-core`, `active-cli`, `active-fmt`, `active-lsp`).
Beyond the 1.0 conformance suite, 1.1 was tested brutally: a fuzz
harness over lexer and parser (`fuzz/`, `active-fuzz`: no panics, no
hangs), property-test invariants (determinism, cycle detection,
secret non-leakage), execution budgets that always hold, and a
1500+ case research battery turning YAML/TOML/JSON failure lineages
into pinned diagnostics. The Rust implementation in this repository
remains the **reference implementation for the conformance suite**
(`tests/spec/`, contract: `spec/conformance.md`).

## License

MIT OR Apache-2.0
