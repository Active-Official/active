# SOLID Language Grammar (v0.9 — alpha surface)

**Status:** normative for the alpha surface (C-013). This grammar describes the
language as implemented in `solid-core`. A second implementer following this
document should accept exactly the same inputs. Deviations are marked
**[impl note]**.

Spec-before-code rule: when the grammar changes, this file changes first.

---

## 1. Lexical grammar

### 1.1 Character classes

```
digit        = [0-9]
hex-digit    — none (no hex literals in the alpha)
letter       = [a-zA-Z]
ident-start  = letter | "_"
ident-char   = letter | digit | "_"
unit-char    = ident-char | "/"
```

### 1.2 Whitespace and comments

```
whitespace   = " " | "\t" | "\r"
newline      = "\n"                       — emitted as a Newline token
comment      = "#" { any-but-newline }     — skipped entirely, no token
```

Comments have no semantic effect. They are not retained in the IR and do not
survive formatting yet (C-040 adds comment preservation).

### 1.3 Identifiers and keywords

```
identifier   = ident-start { ident-char }
keyword      = "let" | "true" | "false" | "null" | "env" | "secret"
```

A keyword is lexed as its own token kind and never as an `identifier`.
`env` and `secret` are contextual keywords: they only have special meaning in
primary position (see 2.4).

### 1.4 Strings

```
string       = '"' { string-char } '"'
             | "'" { char-char } "'"
char-char    = any-but-'"' | escape
char-char    = any-but-''' | escape            (single-quoted form)
escape       = "\" ( "n" | "t" | "r" | "\" | '"' | "0" )
             | "\" ( "'" | "\" )               (single-quoted form)
```

Escape results: `\n` → LF, `\t` → TAB, `\r` → CR, `\\` → backslash, `\"` →
quote, `\0` → NUL. An unknown escape in a double-quoted string keeps the
backslash (`\x` → `\x`); in a single-quoted string the same. **[impl note]**
A string token terminates at the closing quote or end of input; an unclosed
string consumes to EOF without error (see 4.2).

Interpolation is lexically inert: the characters `${`, `}`, and everything
between them are part of the string token. Interpolation is resolved at parse
time (see 2.5). **[impl note]** Alpha interpolation accepts a bare identifier
between the braces only; full expression interpolation is C-023.

### 1.5 Numbers

```
integer      = digit { digit | "_" }
float        = integer "." digit { digit }
number       = integer | float
```

Underscores are digit separators and are stripped (`1_000` → 1000). A dot must
be followed by a digit to be part of the number; `1.` lexes as `integer(1)`
then a `.` operator token. There are no negative literals — `-5` is unary
minus applied to 5.

### 1.6 Unit suffixes

```
unit         = unit-char { unit-char }
```

A unit attaches to a number with **no intervening whitespace** and must be in
the known table:

| Suffix | Unit | | Suffix | Unit |
|---|---|---|---|---|
| `ns` | nanoseconds | | `KiB` `MiB` `GiB` `TiB` | binary bytes |
| `us` `µs` | microseconds | | `KB` `MB` `GB` `TB` | decimal bytes |
| `ms` | milliseconds | | `/s` `per_s` | per second |
| `s` | seconds | | `/m` `per_m` | per minute |
| `m` | minutes | | `/h` `per_h` | per hour |
| `h` | hours | | `req/s` `req_s` `/req/s` | requests per second |
| `d` | days | | `req/m` `req_m` `/req/m` | requests per minute |
| `B` | bytes | | `req/h` `req_h` `/req/h` | requests per hour |

Any other suffix rolls the lexer back (the characters are re-lexed as
separate tokens). **[impl note]** The lexer's rollback means `5x` lexes as
`integer(5)` followed by `identifier(x)`, which then fails at parse with E001.

### 1.7 Operators and delimiters

```
two-char   = "?:" | "==" | "!=" | "<=" | ">=" | "&&" | "||"
one-char   = "=" | "." | "," | "+" | "-" | "*" | "/" | "<" | ">" | "!"
           | "[" | "]" | "{" | "}" | "(" | ")"
```

Longest match wins (`==` beats `=` `=`; `?:` beats `?`).

### 1.8 The token stream

The token stream ends with exactly one `Eof` token. Newline tokens are emitted
verbatim; the parser treats them as soft separators (2.1). **[impl note]**
Unknown characters are silently skipped by the lexer — there is no
"unexpected character" diagnostic (see 4.2).

---

## 2. Syntactic grammar

### 2.1 Document

```
document     = { newline } { statement { newline } } { newline } EOF
statement    = let-decl | table-header | key-value | skip
skip         = any-token                     — silently consumed, no node
```

**[impl note]** The alpha parser is deliberately forgiving: an unexpected token
at statement position is skipped (no diagnostic) so that parsing can continue.
This is resilience, not conformance — a stricter second implementation may
reject. The test corpus's negative cases pin the diagnostics that ARE emitted
(E001/E002/E004).

### 2.2 Statements

```
let-decl      = "let" identifier "=" expression
table-header  = "[" identifier "]"             — single segment only
key-value     = identifier "=" expression
```

**[impl note]** Dotted table headers `[a.b]` are not yet supported (C-021) —
the parser emits E001 and returns. After a table header, key-values belong to
that table until EOF, the next `[`-header, or a `let` (a `let` inside a table
body ends the table).

### 2.3 Expressions (precedence, loosest to tightest)

```
expression    = or-expr
or-expr       = and-expr { "||" and-expr }
and-expr      = eq-expr { "&&" eq-expr }
eq-expr       = cmp-expr { ( "==" | "!=" ) cmp-expr }
cmp-expr      = term { ( "<" | "<=" | ">" | ">=" ) term }
term          = factor { ( "+" | "-" ) factor }
factor        = unary { ( "*" | "/" ) unary }
unary         = ( "-" | "!" ) unary | primary
```

All binary levels are left-associative. Comparison and arithmetic are
non-associative with respect to each other only through the rank above.

`unary` recurses through itself, so `--x` and `!!x` are accepted (and type
errors at evaluation).

### 2.4 Primaries

```
primary       = string | number-with-unit | boolean | null
              | env-ref | secret-ref | identifier
              | array | inline-table | grouped
boolean       = "true" | "false"
null          = "null"
grouped       = "(" expression ")"
number-with-unit = number [ unit ]
env-ref       = "env" "." identifier [ "?:" expression ]
secret-ref    = "secret" [ "." "env" ] "(" expression ")"
```

- `env.NAME` reads the environment variable NAME; missing → `null` (or the
  `?:` default when present). The default is optional.
- `secret(expr)` takes any expression as locator (usually a string literal).
  `secret.env(NAME)` requires a **string literal** argument naming the env var
  whose value becomes the secret (E003 otherwise). **[impl note]** implemented
  during C-010.
- `env` / `secret` as plain identifiers (not in primary position) are ordinary
  identifiers.

### 2.5 Interpolation (parse-time)

Within a double-quoted string token:

```
interpolation = "${" identifier "}"
```

The parser splits the raw string at each `${`. Text before/between/after
becomes literal parts; each `${name}` becomes a reference to `name`, resolved
by the same rules as a bare identifier (2.6). An unclosed `${` (no `}`) is
treated as literal text. **[impl note]** Alpha accepts a bare identifier
inside the braces only; nested/recursive interpolation is C-023.

### 2.6 Arrays and inline tables

```
array         = "[" [ newline ] [ expr-list [ "," ] [ newline ] ] "]"
expr-list     = expression { [ newline ] "," [ newline ] expression }
inline-table  = "{" [ newline ] [ kv-list [ "," ] [ newline ] ] "}"
kv-list       = key-value { [ newline ] "," [ newline ] key-value }
```

Arrays and inline tables may span lines freely. Leading, trailing, and
repeated commas are tolerated (skipped, not errors). An empty array/table is
valid. **[impl note]** Trailing-comma tolerance is deliberate alpha
resilience; comma *insertion* diagnostics are a C-016 healing concern.

### 2.7 Nesting limit

```
nesting-limit = 64
```

Expression nesting (parentheses, unary chains, array elements, inline-table
values) is capped at 64 levels. Exceeding it emits **E004** and substitutes a
`null` value. This is the parser-side half of the execution budget (C-037
adds the evaluator-side budgets).

---

## 3. Name resolution

An identifier reference resolves in this order:

1. **Sibling key** — a key with the same name in the innermost enclosing
   scope (the current table body, inline table, or document root), excluding
   the key currently being defined. Resolution happens after the whole scope
   is parsed, so forward references within a scope resolve.
2. **`let` binding** — a module-level `let` with the same name.
3. **Unresolved** — the identifier becomes `null` and a **W001** warning is
   emitted.

Edges recorded for the dependency graph: reference → referent. `env`, string
literals, and numeric literals are sources (no incoming edges). See
`spec/evaluation-model.md` (C-014) for the full lowering and evaluation rules.

---

## 4. Implementer notes (behavioral edge cases)

These are behaviors a conforming implementation must reproduce to match the
alpha exactly.

### 4.1 Quirks that matter

- `1.` lexes as `integer(1)` + `.` operator. A stray `.` at statement level is
  silently skipped, so `x = 1.` evaluates to `1` (verified behavior).
- `x = 5 5` evaluates to `5`: the second `5` is skipped at statement level.
  Stray tokens after a complete statement are silently absorbed by the
  statement-level skip (2.1).
- `"abc"s` **does** error (E001): `s` parses as a new key, then `=` is
  missing.
- `secret`/`env` in non-primary position are ordinary identifiers.
- Top-level keys after a table header belong to that table (TOML-consistent).
- A `let` inside a table body terminates the table.
- A key whose value references its own name falls through to `let`
  resolution (never a self-cycle) — see `solid-core/tests/regressions.rs`.

### 4.2 Resilience and healing (C-016)

Resilient inputs are diagnosed, not silently swallowed. Every repair carries a
confidence and is governed by the mode policy (see `docs/diagnostics.md`):

| Situation | Code | Confidence | Strict mode | Resilient mode |
|---|---|---|---|---|
| String reaches EOF unclosed | H001 | 0.98 | healed (Info) | healed (Info) |
| Trailing comma before a closer | H002 | 0.98 | healed (Info) | healed (Info) |
| Missing comma between elements/entries | H003 | 0.90 | **error E001** | healed (Warning) |
| Mismatched `]` / `}` closer | H004 | 0.82 | **error E001** | healed (Warning) |
| Unexpected character skipped | H006 | 0.60 | warning | warning |

Strict is the default: only safe repairs (≥ 0.95) apply, and any remaining
Warning fails the process (`--resilient` relaxes this). Unresolved identifiers
warn (W001) in both modes rather than error — that is a semantic warning, not
a syntax repair.

### 4.3 Diagnostic codes referenced by this grammar

| Code | Meaning | Emitted by |
|---|---|---|
| E001 | expected token | parser |
| E002 | unexpected token | parser |
| E003 | malformed `secret.env` argument | parser |
| E004 | expression nesting limit exceeded | parser |
| W001 | unresolved identifier | evaluator |

Full catalog: `docs/diagnostics.md` (C-015).
