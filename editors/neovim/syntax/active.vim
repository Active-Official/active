" Active syntax (C-057): comments, strings (incl. triple-quoted blocks
" and ${...} interpolation), tables, keywords, annotations,
" numbers with units, booleans. Mirrors editors/vscode TextMate scopes.
if exists("b:current_syntax")
  finish
endif

syntax case match

" Comments first so everything else nests outside them.
syntax match activeComment "#.*$" contains=activeAnnotation
syntax match activeAnnotation "@\(intent\|constraint\|ai_immutable\|patch\|remove\|rename\)\>" contained

" Triple-quoted blocks (can span lines) before single-line strings.
" Separate openers so a ''' block never closes on """.
syntax region activeBlockString start=+"""+ end=+"""+ contains=activeEscape,activeInterp keepend
syntax region activeBlockString start=+'''+ end=+'''+ keepend
syntax sync minlines=50
syntax region activeString start=+"+ skip=+\\\\\|\\"+ end=+"+ contains=activeEscape,activeInterp oneline
syntax region activeString start=+'+ skip=+\\\\\|\\'+ end=+'+ contains=activeEscape oneline
syntax match activeEscape "\\\\\(n\|t\|r\|\\\\\|\"\|'\|0\|\$\|u{[^}]*}\)" contained
syntax region activeInterp matchgroup=activeInterpDelim start="\${" end="}" contains=activeString,activeNumber,activeBool,activeKeyword contained

syntax match activeTableHeader "^\s*\[[^]]*\]" contains=activeTableName
syntax match activeTableName "[A-Za-z_][A-Za-z0-9_]*" contained
syntax keyword activeKeyword let profile include schema active_profile active_mode active_capabilities depends_on as private closed
syntax match activeFunction "\<\(env\|secret\|blob\)\>\ze\s*[.(]"

syntax match activeNumber "\<-\=\(\d\+\.\d\+\|\d\+\)\(ns\|us\|µs\|ms\|s\|m\|h\|d\|week\|sec\|min\|B\|KiB\|MiB\|GiB\|TiB\|PiB\|EiB\|KB\|MB\|GB\|TB\|/s\|/m\|/h\|req/s\|req/m\|req/h\|Hz\|kHz\|MHz\|GHz\|THz\|b\|Kb\|Mb\|Gb\|bps\|Kbps\|Mbps\|Gbps\|MB/s\|tokens\|ktokens\|Mtokens\|tok/s\|FLOPS\|TFLOPS\|PFLOPS\|Pa\|bar\|psi\|V\|A\|W\|J\|kWh\|N\|deg\|rad\|rpm\|meter\|km\|kg\|g\|K\|Cel\|Fahr\|dB\)\=\>"
syntax keyword activeBool true false null

highlight default link activeComment Comment
highlight default link activeAnnotation Special
highlight default link activeBlockString String
highlight default link activeString String
highlight default link activeEscape SpecialChar
highlight default link activeInterpDelim Delimiter
highlight default link activeTableHeader Title
highlight default link activeTableName Identifier
highlight default link activeKeyword Keyword
highlight default link activeFunction Function
highlight default link activeNumber Number
highlight default link activeBool Boolean

let b:current_syntax = "active"
