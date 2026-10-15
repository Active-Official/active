# SOLID

**Symmetric · Overlay-ready · Loop-proof · Intent-driven · Deterministic**

SOLID is a modern configuration and data language designed to be safer than YAML, more expressive than TOML, more human-friendly than JSON, and excellent for both humans and AI agents.

This repository contains the **alpha production foundation** with a working parser, IR, dependency graph, evaluator, units, string interpolation, and CLI.

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
- CLI: `eval`, `check`, `graph`, `convert`
- Dual-syntax formatter stub
- Language server stub

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
