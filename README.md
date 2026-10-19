# ACTIVE

**Symmetric · Overlay-ready · Loop-proof · Intent-driven · Deterministic**

[![crates.io](https://img.shields.io/crates/v/active-core)](https://crates.io/crates/active-core)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE-MIT)

ACTIVE is a modern configuration and data language designed to be safer than YAML, more expressive than TOML, more human-friendly than JSON, and excellent for both humans and AI agents.

Your config files are lying to you. YAML once turned an entire country into `false`. A stray tab silently changed what a file meant. An unquoted `yes` became `true` somewhere between your editor and production. Nobody got an error. Every week, engineers burn hours debugging config that *parsed fine* but *meant something else*.

Active ends that. It runs on one rule that changes everything: **nothing is ever implicit.** What you write is exactly what runs: for a human reading it at 2am, and for an AI agent generating it at machine speed.

**Active is the configuration language that refuses to lie.** What you write is what runs. Units have dimensions; secrets have taint; budgets always hold.

## What is Active?

Active is a configuration language (the files that tell your software what to do: ports, keys, limits, feature flags, secrets). It replaces YAML, TOML, and JSON with something safer than YAML, more explicit than TOML, and more human than JSON. Underneath it's a real language implementation (lexer, parser, IR, dependency graph, evaluator), not a pile of regexes. You never have to think about that to use it.

**What it accomplishes.** One file, one meaning, every time. Active evaluates deterministically: the same file always produces the same result, on every machine, in every pipeline. No locale quirks, no version-dependent parsing, no "works on my machine." And if something is wrong, you get an error with an exact location, never a silent wrong answer.

**What it solves for humans.** It kills the entire category of "the config parsed but did the wrong thing." Countries stay countries (`no` is never `false`). Numbers stay numbers (`01` is `1`, `"01"` is text: the quotes are your choice, and the language respects it). Units can't silently mix (`5s` and `512MiB` are different dimensions, and Active refuses to confuse them). Secrets come out redacted by construction, so they can't leak into your logs. And when you do make a mistake, the diagnostic points at the exact line and column instead of failing three services downstream.

**What it solves for AI agents.** Agents generate config faster than any human can review it. So the language itself has to be the reviewer. Active gives agents three things most formats can't. First, *explicitness*: there is nothing to hallucinate, because unquoted values never secretly become something else. Second, *budgets that always hold*: nesting depth, collection sizes, and runtime steps are capped, so a runaway generation can't hang or explode. Third, *machine-grade diagnostics*: every error carries a code and a span, so an agent can locate the problem, fix it, and retry without a human in the loop. Config that agents can't silently misuse is a prerequisite for autonomous infrastructure: Active is built to be that prerequisite.

**Why a senior engineer respects it.** Because the strictness is load-bearing, not cosmetic. Duplicate keys always warn instead of silently winning. Unknown names resolve to announced nulls instead of coercing into whatever is convenient. Interpolation is scoped to strings, which makes exponential expansion inexpressible rather than merely budgeted. And it's all verified: a 1500+ case battery, fuzz coverage over the lexer and parser, property-tested invariants, budgets enforced in code. Explicitness all the way down, including in the error taxonomy.

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

If Active saves you from one silent config failure, give it a star so the next reader, human or agent, finds it first.

## License

MIT OR Apache-2.0
