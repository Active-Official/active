" SOLID syntax (C-057): comments, strings (incl. triple-quoted blocks
" and ${...} interpolation), tables, keywords, annotations,
" numbers with units, booleans. Mirrors editors/vscode TextMate scopes.
if exists("b:current_syntax")
  finish
endif

syntax case match

" Comments first so everything else nests outside them.
syntax match solidComment "#.*$" contains=solidAnnotation
syntax match solidAnnotation "@\(intent\|constraint\|ai_immutable\|patch\|remove\|rename\)\>" contained

" Triple-quoted blocks (can span lines) before single-line strings.
" Separate openers so a ''' block never closes on """.
syntax region solidBlockString start=+"""+ end=+"""+ contains=solidEscape,solidInterp keepend
syntax region solidBlockString start=+'''+ end=+'''+ keepend
syntax sync minlines=50
syntax region solidString start=+"+ skip=+\\\\\|\\"+ end=+"+ contains=solidEscape,solidInterp oneline
syntax region solidString start=+'+ skip=+\\\\\|\\'+ end=+'+ contains=solidEscape oneline
syntax match solidEscape "\\\\\(n\|t\|r\|\\\\\|\"\|'\|0\|\$\|u{[^}]*}\)" contained
syntax region solidInterp matchgroup=solidInterpDelim start="\${" end="}" contains=solidString,solidNumber,solidBool,solidKeyword contained

syntax match solidTableHeader "^\s*\[[^]]*\]" contains=solidTableName
syntax match solidTableName "[A-Za-z_][A-Za-z0-9_]*" contained
syntax keyword solidKeyword let profile include schema active_profile depends_on as private closed
syntax match solidFunction "\<\(env\|secret\)\>\ze\s*[.(]"

syntax match solidNumber "\<-\=\(\d\+\.\d\+\|\d\+\)\(ns\|us\|µs\|ms\|s\|m\|h\|d\|B\|KiB\|MiB\|GiB\|TiB\|KB\|MB\|GB\|TB\|/s\|/m\|/h\|req/s\|req/m\|req/h\)\=\>"
syntax keyword solidBool true false null

highlight default link solidComment Comment
highlight default link solidAnnotation Special
highlight default link solidBlockString String
highlight default link solidString String
highlight default link solidEscape SpecialChar
highlight default link solidInterpDelim Delimiter
highlight default link solidTableHeader Title
highlight default link solidTableName Identifier
highlight default link solidKeyword Keyword
highlight default link solidFunction Function
highlight default link solidNumber Number
highlight default link solidBool Boolean

let b:current_syntax = "solid"
