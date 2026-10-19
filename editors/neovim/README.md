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
vim.lsp.config('active_ls, {
  cmd = { 'active-lsp' },
  filetypes = { 'active' },
  root_markers = { '.git' },
})
vim.lsp.enable('active_ls')
```
