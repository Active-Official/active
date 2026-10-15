# SOLID

A new data serialization language for humans **and** AI — designed from the ground
up to be pleasant to read and write by hand, unambiguous to parse, and
expressive enough for real configuration and data needs. Think TOML/YAML/JSON,
but on steroids.

> **Status:** early scaffold. Repository structure is in place; the language
> design, grammar, and reference implementation are being specified.

## Repository layout

| Folder          | Purpose                                                        |
|-----------------|----------------------------------------------------------------|
| `src/`          | Reference implementation (parser, lexer, serializer, evaluator) |
| `tests/`        | Test suite — conformance, edge cases, regression               |
| `examples/`     | Example documents in the SOLID format                          |
| `spec/`         | Formal language specification (grammar, semantics)             |
| `benchmarks/`   | Performance benchmarks vs. other formats                       |
| `tools/`        | Dev tooling: CLI, formatter, linter, playground                 |
| `docs/`         | Public documentation                                           |

`scratch/` and two private working documents (`docs/Agent-Handbook.md`,
`docs/Memories-and-Observations.md`) exist locally but are **gitignored** and
never published.
