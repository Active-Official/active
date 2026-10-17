# active-core

Core implementation of the [Active](https://crates.io/crates/active) configuration and data language: lexer, parser, IR, dependency graph, evaluator, types, diagnostics.

The command-line interface, formatter and language server are all built on this crate.

## Status

Stable 1.0 implementation: lexer, parser, arena IR with dependency DAG, topological evaluation with cycle rejection, units, schemas, budgets, healing, converters, and doc generation.

## License

Licensed under either of the Apache License, Version 2.0 or the MIT license, at your option.
