# SOLID

**Symmetric · Overlay-ready · Loop-proof · Intent-driven · Deterministic**

SOLID is a modern configuration and data language designed to be safer than YAML, more expressive than TOML, more human-friendly than JSON, and excellent for both humans and AI agents.

This repository contains the **production implementation**: parser, IR,
dependency graph, evaluator, units, string interpolation, schema
validation, CLI, formatter, converters, documentation generator, and
language server.

## Features in this alpha

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
- Dual-syntax formatter (`solid fmt`: indent, bracketed, preserving)
- Language server (`solid-ls`: hover, go-to-definition, completions,
  diagnostics, inlay hints, code actions)

## Install

```bash
cargo install solid-cli solid-fmt solid-ls   # needs Rust 1.85+
```

Release archives (Linux/macOS/Windows × x86_64/ARM64, with SHA256SUMS
and build attestations) are attached to
[GitHub releases](https://github.com/RatioArtificiosa/solid-lang/releases).
Editors: `editors/vscode` (VS Code / Cursor / VSCodium vsix) and
`editors/neovim` (runtimepath plugin + `solid-ls`). Details and the
crates.io procedure: `spec/packaging.md`.

## Quick start

```bash
cargo build -p solid-cli --release   # needs recent stable Rust (1.85+)

# simple demo
./target/release/solid eval examples/demo.solid

# comprehensive example (units + interpolation + secrets + arrays)
./target/release/solid eval examples/complete.solid

./target/release/solid check examples/complete.solid
./target/release/solid graph examples/complete.solid
./target/release/solid convert examples/complete.solid --to json
```

## Project layout

```
solid/
├── Cargo.toml
├── README.md
├── docs/DESIGN.md
├── examples/
│   ├── demo.solid
│   └── complete.solid          ← richer example
├── solid-core/                 # parser, IR, evaluator, types, diagnostics
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
├── solid-cli/
├── solid-fmt/
└── solid-ls/
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

Alpha. The lexer, parser, IR, graph, evaluator, units and interpolation are implemented and usable. Dual-syntax indentation mode, profiles, `@patch`, full schema engine and the production healer are the next milestones.

## License

MIT OR Apache-2.0
