" SOLID filetype detection (C-057): *.solid files get filetype=solid.
augroup solid_ftdetect
  autocmd!
  autocmd BufNewFile,BufRead *.solid setfiletype solid
augroup END
