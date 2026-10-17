" Active filetype detection (C-057): *.active files get filetype=active.
augroup active_ftdetect
  autocmd!
  autocmd BufNewFile,BufRead *.active setfiletype active
augroup END
