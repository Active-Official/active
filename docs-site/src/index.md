# SOLID

**Symmetric · Overlay-ready · Loop-proof · Intent-driven · Deterministic**

SOLID is a configuration and data language in the TOML/YAML/JSON class,
built for humans and AI agents alike: safer than YAML, more expressive
than TOML, more human-friendly than JSON.

- **Symmetric** — write tables with braces *or* indentation (or both);
  both lower to identical values.
- **Overlay-ready** — profiles, includes, and structural patches compose
  base configs into environments without copy-paste.
- **Loop-proof** — every reference is a dependency edge; cycles are
  rejected before any value is produced. Evaluation is hermetic and
  budgeted — no I/O, no shell, no surprises.
- **Intent-driven** — `@intent`, `@constraint`, schemas, and semantic
  blocks carry *why*, not just *what*; the toolchain enforces it.
- **Deterministic** — same input, same value tree, same diagnostics,
  everywhere.

This book has four parts. The [tutorial](tutorial/getting-started.md)
takes you from zero to validated configs. The
[cookbook](cookbook/kubernetes.md) solves real deployment shapes.
[Migration](migration/toml.md) brings your TOML/YAML/JSON over.
[Reference](reference/syntax.md) is the precise surface.

Install in one line (`cargo install solid-cli solid-fmt solid-ls`,
Rust 1.85+), or grab a release archive from
[GitHub releases](https://github.com/RatioArtificiosa/solid-lang/releases).
The language server ships as a VS Code extension (`editors/vscode`)
and a Neovim plugin (`editors/neovim`).
