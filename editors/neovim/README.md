# SOLID for Neovim (C-057)

Runtimepath plugin: filetype detection (`*.solid`), buffer defaults,
syntax highlighting, and `:help solid` docs. The language server is
`solid-ls` over stdio (hover, go-to-definition, completions,
diagnostics, inlay hints, code actions).

## Install

vim-plug (subdirectory plugin): >

```vim
Plug 'RatioArtificiosa/solid-lang', {'rtp': 'editors/neovim'}
```

Manual: add `editors/neovim` to `'runtimepath'`, then `:helptags`
for the local help page.

## Language server

Install once: `cargo install solid-ls` (or a release archive).
nvim-lspconfig: >

```lua
require('lspconfig').solid_ls.setup({
  cmd = { 'solid-ls' },
  filetypes = { 'solid' },
  root_markers = { '.git' },
})
```

Neovim 0.11+ built-in client (`root_markers`, plural, needs 0.11;
on older Neovim use nvim-lspconfig with `root_dir` instead): >

```lua
vim.lsp.config('solid_ls', {
  cmd = { 'solid-ls' },
  filetypes = { 'solid' },
  root_markers = { '.git' },
})
vim.lsp.enable('solid_ls')
```
