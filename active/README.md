# Active

**A modern configuration and data language: safer than YAML, more expressive than TOML, more human-friendly than JSON, and built for both humans and AI agents.**

Active is a configuration and data language with first-class units, string
interpolation, lexical bindings, redacted secrets, a dependency graph and
deterministic evaluation. This crate is the language core.

## Status

Stable 1.0 implementation: the lexer, parser, IR, dependency graph,
evaluator, units and interpolation live in `active-core` and are
re-exported here. This is the crate dependents use.

## Installation

```sh
cargo add active
```

## Usage

```rust
use active::VERSION;

println!("Active {VERSION}");
```

## Minimum supported Rust version

Rust 1.85 or newer.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
