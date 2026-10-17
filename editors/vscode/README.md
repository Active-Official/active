# ACTIVE for VS Code (also: Cursor, VSCodium, Positron)

One vsix serves every VS Code-compatible editor. Syntax highlighting
(TextMate), bracket/comment configuration, and the full `active-lsp`
language server (hover, go-to-definition, completions, diagnostics,
inlay hints, code actions) over stdio.

## Install

- **Release vsix** (no Rust needed): download
  `active-lang-<version>.vsix` from the
  [GitHub releases](https://github.com/Active-Official/active/releases),
  then `code --install-extension active-lang-<version>.vsix`
  (`cursor --install-extension …` / VSCodium equivalent).
- **From source**: `cargo install active-lsp`, then in `editors/vscode`
  run `npm install && npx vsce package`, and install the vsix above.
- **Marketplace**: `vsce publish` (maintainer step — needs a
  marketplace token; see spec/packaging.md §5).

## Settings

- `ACTIVE.languageServer.path` (default `"active-lsp"`): PATH lookup by
  default; point at an absolute binary elsewhere
  (e.g. next to a release archive). Changing it asks for
  a window reload (the server restarts).

## Requirements

`active-lsp` on PATH (or configured above). No Bundled server: the
extension never downloads binaries — supply-chain stays auditable.
