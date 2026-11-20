# Propose / gate conformance vectors (L-P62)

Each `*.active` file is a candidate for the verifier. `manifest.toml`
records the verdict the shipped binary must produce; the
`active-cli/tests/lp62_vectors.rs` test drives `active gate` and
`active propose` against every case, so a regression in the verdict (or
in the receipt a case asks for) fails the suite.

These vectors pin command behavior, not language semantics: the language
cases stay in the top-level `conformance/manifest.toml`.
