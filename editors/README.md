# Editor integrations

| Editor | Status | What ships |
|---|---|---|
| VS Code / Cursor / VSCodium | Supported | LSP client, grammar, language configuration, status bar (mode, node-budget %, taint), commands (`check`, `inspect`, `graph`, `repair`, `capability`), repair-plan webview |
| Neovim | Supported | filetype, syntax, ftplugin, docs, `lua/active.lua` status + five commands, `:help active` |
| Zed | Not implemented | Zed supports LSP via its `languages` extension config; a thin extension (`extension.toml` + `languages/active/config.toml`) can point at `active-lsp`. No grammar package is published yet, so this is a documented gap. |
| JetBrains | Not implemented | The LSP API (`com.intellij.platform.lsp`) can host `active-lsp`; a plugin needs a Gradle scaffold and TextMate grammar bundling. Documented gap. |

## Verification

- `editors/vscode/scripts/smoke.js` activates the real extension against a
  stubbed VS Code API: six commands registered, one status bar, and a
  redaction probe through the same CLI the extension calls.
- `editors/neovim/smoke.py` checks the Lua layer exposes the five commands
  and parses with `luac`/`nvim` when available (SKIPs openly otherwise).
- CI runs the VS Code smoke; the Neovim smoke runs where a Lua toolchain
  exists.

Stretch targets stay honest: no half-built plugin claims support it does
not have.
