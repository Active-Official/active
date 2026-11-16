# Conformance

`tests/spec/` is the official conformance suite: 13 case pairs
(`<name>.active` + `<name>.expected.json`) asserting accepted
documents' value trees byte for byte. The Rust implementation in
this repository is declared the **reference implementation for
the 1.0 conformance suite** — the compatibility target every other
implementation proves against. Declared in 1.0.0 (2026-10-16); the
current line is 1.4.0, moving to 2.0.0 (VERSIONING.md).

Passing means: parse with default options, evaluate with a default
engine, zero diagnostics, and byte-exact canonical JSON after CRLF
normalization (tables in source binding order, secrets as
`"***SECRET***"`). Full contract, canonical mapping, versioning
rules, and how to run it: `spec/conformance.md`.
