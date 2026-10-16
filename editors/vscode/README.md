# SOLID for VS Code (also: Cursor, VSCodium, Positron)

One vsix serves every VS Code-compatible editor. Syntax highlighting
(TextMate), bracket/comment configuration, and the full `solid-ls`
language server (hover, go-to-definition, completions, diagnostics,
inlay hints, code actions) over stdio.

## Install

- **Release vsix** (no Rust needed): download
  `solid-lang-<version>.vsix` from the
  [GitHub releases](https://github.com/RatioArtificiosa/solid-lang/releases),
  then `code --install-extension solid-lang-<version>.vsix`
  (`cursor --install-extension …` / VSCodium equivalent).
- **From source**: `cargo install solid-ls`, then in `editors/vscode`
  run `npm install && npx vsce package`, and install the vsix above.
- **Marketplace**: `vsce publish` (maintainer step — needs a
  marketplace token; see spec/packaging.md §5).

## Settings

- `solid.languageServer.path` (default `"solid-ls"`): PATH lookup by
  default; point at an absolute binary elsewhere
  (e.g. next to a release archive). Changing it asks for
  a window reload (the server restarts).

## Requirements

`solid-ls` on PATH (or configured above). No Bundled server: the
extension never downloads binaries — supply-chain stays auditable.
