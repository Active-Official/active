# Active for Neovim (C-057)

Runtimepath plugin: filetype detection (`*.active`), buffer defaults,
syntax highlighting, and `:help active` docs. The language server is
`active-lsp` over stdio (hover, go-to-definition, completions,
diagnostics, inlay hints, code actions).

**Active is the configuration language that refuses to lie.** What you
write is what runs. Units have dimensions; secrets have taint; budgets
always hold.

## Install

vim-plug (subdirectory plugin): >

```vim
Plug 'RatioArtificiosa/active-lang', {'rtp': 'editors/neovim'}
```

Manual: add `editors/neovim` to `'runtimepath'`, then `:helptags`
for the local help page.

## Language server

Install once: `cargo install active-lsp` (or a release archive).
nvim-lspconfig: >

```lua
require('lspconfig').active_ls.setup({
  cmd = { 'active-lsp' },
  filetypes = { 'active' },
  root_markers = { '.git' },
})
```

Neovim 0.11+ built-in client (`root_markers`, plural, needs 0.11;
on older Neovim use nvim-lspconfig with `root_dir` instead): >

```lua
vim.lsp.config('active_ls', {
  cmd = { 'active-lsp' },
  filetypes = { 'active' },
  root_markers = { '.git' },
})
vim.lsp.enable('active_ls')
```

## Status line and commands (EDITOR.004)

The bundled Lua layer adds a status string and five commands. It needs
the `active` CLI on PATH (override with `vim.g.active_cli = '/path/to/active'`):

```lua
require('active').setup()
vim.opt.statusline:append('%{v:lua.require"active".status()}')
```

| Command | Runs |
|---|---|
| `:ActiveCheck` | `active check` |
| `:ActiveGraph` | `active graph --format mermaid` |
| `:ActiveRepair` | `active repair --browser` |
| `:ActiveCapability` | `active capability` |
| `:ActiveMode` | `active mode` |

`M.status()` renders `Active <mode> <node-budget %> [taint]`; it runs on
`BufEnter`/`BufWritePost` and is safe to call from a status line (it
returns an empty string for non-file buffers). Secrets never enter the
status string: the CLI exports are redacted and the layer prints only
mode, percentages, and a taint marker.
