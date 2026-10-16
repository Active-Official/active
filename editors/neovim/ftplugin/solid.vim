" SOLID buffer defaults (C-057): two-space indent (both syntax styles),
" # comments, solid-ls as the language server hook point.
if exists("b:did_ftplugin")
  finish
endif
let b:did_ftplugin = 1

setlocal shiftwidth=2
setlocal softtabstop=2
setlocal expandtab
setlocal comments=:# commentstring=#\ %s
" Dotted paths (a.b.c) move as one word; `-` and `/` stay motion
" boundaries (they appear inside values, never identifiers).
setlocal iskeyword+=.
