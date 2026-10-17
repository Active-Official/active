# ACTIVE Language Grammar (v0.9 — alpha surface)

**Status:** normative for the alpha surface (C-013). This grammar describes the
language as implemented in `ACTIVE-core`. A second implementer following this
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
comment      = "#" { any-but-newline }     — trivia: no token, no semantics
```

Comments have no semantic effect. They are retained as trivia on
the module (normative attachment, spec/dual-syntax.md §7) and
survive formatting round-trips (C-022/C-040).

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
time (see 2.5).

### 1.4.1 Multi-line strings (C-024)

```
ml-string     = '"""' ml-body '"""'
              | "'''" ml-body-literal "'''"
ml-body       = { ml-char }                    (double-quoted form)
ml-body-literal = { ml-char-literal }          (single-quoted form)
ml-char       = any-char | escape | newline    (may span lines)
ml-char-literal = any-char | newline           (escapes inert, no interpolation)
```

- The token opens at `"""` / `'''` and closes at the next matching triple
  quote. An unterminated multi-line string consumes to EOF and records an
  H001 repair diagnostic (same channel as single-line unterminated strings)
  rather than rejecting the input.
- **Common-indent stripping:** if the first line after the opening delimiter
  is blank (only whitespace up to the first newline), that leading newline is
  dropped. Then the longest common leading-whitespace prefix of all non-blank
  lines is removed from every line. Tabs and spaces are distinct characters —
  a line is only part of the common prefix measurement when its indentation
  characters match the prefix exactly; mixed-indent lines keep their excess.
  A final line containing only whitespace before the closing delimiter is
  dropped. Whitespace-only lines in the interior are normalized to empty
  (matching Python's `textwrap.dedent`); they contribute no prefix and keep
  no trailing whitespace.
- **Escapes:** the `"""` form processes escapes exactly like `"…"` (see 1.4).
  The `'''` form is literal: backslashes are kept verbatim and `${…}` is not
  interpolation.
- **Interpolation:** the `"""` form supports full `${…}` expression
  interpolation (same machinery as 2.5). The `'''` form never interpolates.
- **Semantic blocks:** an optional annotation marks the string's role for
  tools (formatter, LSP, doc generator):

```
semantic-tag  = "prompt" | "policy" | "template"
ml-annotated  = "@" semantic-tag ws* ml-string
```

  The tag is trivia: it does not change the value, only tool treatment.
  Unknown tags are an E014 hard error (a mistyped tag silently ignored
  would be worse than a rejection); Phase 2 annotations are C-033/034.
- **Statement form:** a bare `@tag ml-string` at statement level binds
  under the tag name itself (`@prompt """…"""` → key `prompt`), so AI
  prompt files read naturally. In value position (`key = @tag """…"""`,
  array elements) the tag wraps that value.
- **Spans:** the token span covers the opening delimiter through the closing
  delimiter (or EOF when unterminated); interpolation sub-expression spans
  are rebased onto the string position exactly as in 2.5.

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
verbatim; the parser treats them as soft separators (2.1). **[impl note]** A
character that no lexical rule accepts is a hard error (**E005**), reported
with a precise span — never silently skipped (see 4.2). Stray *tokens* at
statement position are still skipped by the parser's error recovery.

---

## 2. Syntactic grammar

### 2.0 Statement forms (dual syntax)

A document may be written in brace style, indentation style, or a mix. The
normative design is `spec/dual-syntax.md` (C-020); this section summarizes it.

```
statement     = let-decl | table-header | table-block | block-header | key-value
table-header  = "[" identifier "]"                  TOML-style
table-block   = identifier "{" [ newline { statement } ] "}"   HCL-style
block-header  = identifier ":" newline indent      indentation style
key-value     = identifier ( "=" | ":" ) expression
```

- `identifier "{"` at statement position is always a table block; an inline
  table can only appear as a value, so the forms never conflict.
- `identifier ":"` followed by a value on the same line is a key-value pair;
  followed by a newline and an indent it is a block header.
- Indentation is measured only at bracket depth 0: inside `(`, `[`, `{` the
  layout is cosmetic (`spec/dual-syntax.md` §3).
- A table body (`[name]`) ends at the next header, `let`, EOF, or a Dedent
  belonging to an enclosing block.
- An indented block ends at its Dedent; a brace block at its `}`.

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
table-header  = "[" dotted-path "]"            — one or more segments
dotted-path   = identifier ("." identifier)*
key-value     = identifier "=" expression
```

Dotted headers (`[a.b.c]`) descend through (or create) nested tables;
the body belongs to the leaf. Repeated headers merge at shared
prefixes (`[a.b]` then `[a.c]` keep both); a repeated leaf is
last-wins, exactly like repeated single headers. Descending through
a present non-table value is E001. After a table header, key-values
belong to that table until EOF, the next `[`-header, or a `let`
(a `let` inside a table body ends the table).

### 2.7 Include statements (C-025)

```
include-stmt  = "include" string-literal [ "as" identifier ] [ "private" ]
string-literal = String | SingleString | MlString | MlSingleString
```

- `include` is a **contextual keyword**: `include` followed by `=`, `:`,
  or `{` stays an ordinary key, table header, or table block (existing
  documents using `include` as a name keep working in all three forms);
  `include` followed by anything else starts an include statement. A bare
  `include` with no path is E015.
- The path must be a **string literal**. A non-string path (`include foo`,
  `include 5`) is E015. **Interpolation in the path is a hard error
  (E015)**, even when the referenced name exists — include paths are
  always static so the dependency graph stays computable without
  evaluation.
- `as alias` nests the included file's top-level keys under `alias`.
  `private` keeps the included keys out of the exported value tree while
  leaving them referenceable (C-026).
- Paths resolve against the including file's directory. Absolute paths and
  paths escaping the directory via `..` are E015 by default (opt out with
  `allow_parent_escapes`, e.g. the CLI `--allow-parent-escapes` flag).
- A missing or unreadable file is E017. Including a file already on the
  include stack (a cycle), or nesting deeper than 32 files, is E016.
- Merge semantics (normative): **included files provide defaults**.
  Explicit local keys always win, regardless of position; when several
  included files define the same key, the LAST include wins (cascade).
  Tables merge recursively — local scalars win at every level, and a
  later include only overwrites paths a previous include provided, never
  explicit local content (so a later `key = scalar` can never discard a
  local table holding local keys). Deep structural composition beyond
  defaults is the domain of profiles (C-027) and patches (C-028/029). An
  included file's `let` bindings are visible to the including module but
  never exported.
- `include` statements are allowed anywhere a statement is allowed
  (document root, table bodies, brace blocks, indented blocks) and merge
  into the enclosing scope.

### 2.8 Profile blocks (C-027)

```
profile-block = "profile" string-literal "{" [ newline { statement } ] "}"
```

- `profile` is a **contextual keyword** like `include`: `profile` followed
  by `=`, `:`, or `{` stays an ordinary key, header, or table block;
  `profile` followed by a string literal opens a profile block. A
  `profile` block with no string name is E018.
- Profile blocks may appear **only at document root**; anywhere else is
  E018. A duplicate profile name is E018.
- Selection is by the root key `active_profile` (any expression, usually
  `env.ACTIVE_PROFILE ?: "development"`): if it evaluates to a string
  naming a defined profile, that profile merges; if it is missing, no
  profile merges (base configuration only); if it names no defined
  profile, or is not a string, a W002 warning is emitted and no profile
  merges.
- Merge (normative): the selected profile's fields merge into the
  document root — scalars last-wins (profile wins), tables deep-merge
  with the same local-wins semantics as includes (the profile counts as
  the "later" layer over base). A profile value whose type differs from
  the base value still wins, with a W003 warning. Profile blocks never
  appear in the exported tree; `active_profile` itself stays visible.

### 2.9 Structural patches (C-028)

```
patch-stmt    = "@patch" "(" string-literal "," expression ")"
              | "@remove" "(" string-literal ")"
              | "@rename" "(" string-literal "," string-literal ")"
path          = segment { "." segment }
segment       = identifier { "[" integer "]" }
identifier    = ASCII letter or "_" followed by ASCII alphanumerics or "_"
```

- `@patch`, `@remove`, `@rename` are **reserved at statement level**:
  `@` followed by one of those names never forms an ordinary key (keys
  cannot contain `@`), so any continuation other than `(` is E019. Other
  `@tag` forms are semantic blocks (§1.4.1).
- Patch statements may appear **only at document root** (E019 elsewhere,
  including inside profiles). Paths are absolute from the document root.
- The first argument (and both rename arguments) must be string
  literals; interpolation in a path is E019 (paths are static so the
  affected tree is computable without evaluation). The `@patch` value
  is a full expression — patches are dependency-aware and may reference
  computed base values (even the patched key itself: values evaluate
  before application, so no cycle is possible).
- `@patch(path, value)` sets the value at path (set semantics, not
  merge). Missing intermediate tables are created silently; the path
  itself is the declaration.
- `@remove(path)` deletes the key or array element. A missing target
  warns (W004) and succeeds (idempotent). Array indices must be
  non-negative; out-of-bounds warns (W004).
- `@rename(old, new)`: `old` is a path, `new` is the new key name in the
  same parent. A missing source warns (W004); an existing target is a
  hard error (E019).
- Patches apply in source order, after evaluation (and after profile
  merge), before schema validation. Patch statements never appear in
  the exported tree.

### 2.10 Schema blocks (C-030)

```
schema-block  = "schema" ( "=" inline-table | "{" { statement } "}"
                | ":" newline indent )
schema-table  = "{" [ schema-entry { "," schema-entry } ] "}"
schema-entry  = identifier "=" schema-type
schema-type   = bare-type [ "(" [ schema-arg { "," schema-arg } ] ")" ]
              | "Array" "<" schema-type ">"
              | "Table" [ "{" [ schema-entry { "," schema-entry } ] "}" ]
              | "Enum" "(" string { "," string } ")"
bare-type     = "Int" | "Float" | "String" | "Bool" | "Null"
              | "Duration" | "Bytes" | "Secret" | "Any" | "Table" | "Array"
schema-arg    = identifier "=" expression
```

- `schema` is contextual: a `schema` key with a **table value** parses
  as a schema block; a `schema` key with any other value stays ordinary
  data. A schema table whose entries are not valid type expressions is
  E020 (a mistyped schema must never silently become data).
- Named arguments per type (anything else is E020): `Int`/`Float` take
  `min`, `max`, `default`; `Duration`/`Bytes` take `min`, `max`,
  `default`; `String` takes `min_len`, `max_len`, `pattern`, `default`;
  `Bool`/`Null`/`Secret`/`Any` take `default` only; `Enum` takes
  positional strings only (empty is E020); `Array` takes only `<T>`;
  `Table` takes only `{...}` with an optional reserved `closed = true`
  entry. Every leaf type additionally accepts `deprecated` (C-042):
  `true` or a string-literal migration message (anything else is
  E020). Argument values are full expressions (literals, env refs,
  arithmetic), evaluated with the document — except `deprecated`,
  which is static text by design.
- Schemas may appear at root (validating the document) or nested (validating
  that table). Validation itself is C-031; C-030 delivers parsing and IR only.
- External schemas ride the include mechanism: an included file's schema
  merges like any other key. No new syntax is required. Two schemas merged
  under one key union their fields (local-wins on conflicts, cascade
  among includes); the `closed` flag follows the same rule.- Schemas are banned inside profiles (E020), including schemas that would
  arrive through an include spliced into a profile scope: an included
  schema there is E020 at its own span, never profile data.
- Schema blocks are table-scope metadata: they may not appear inside
  array elements (E020) — not even inside inline tables nested in
  arrays. Element types are described with `Array<T>` (validated in
  C-031), so no expressiveness is lost and extraction stays total:
  every surviving SchemaTable is reachable by table descent.
- Schema type nesting (`Array<Array<…>>`, `Table { … Table { … } … }`) is
  bounded at 64 levels (E004 beyond); a brace-form schema without its
  closing `}` is E001. A call-form `Table(…)` is E020 — nested schemas
  use `Table { … }` only. The canonical empty schema is `schema = {}`.

### 2.11 Dependency declarations (C-042)

```
depends-on    = "depends_on" "=" string-array
string-array  = "[" [ string { "," string } ] "]"
```

- `depends_on` is contextual: a `depends_on` key with an array of
  string literals parses as dependency metadata for its table scope
  (spec/linter.md §2); any other value stays ordinary data. An
  array containing a non-string is E022, as is an inline-table
  occurrence (dependency blocks belong to named scopes only).
- Listed paths resolve dotted from the document root; each
  resolvable path adds a dependency edge (undeclared cycles surface
  as S001). The linter checks completeness (L002).

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

### 2.5 Interpolation (parse-time, double-quoted strings only)

Within a double-quoted string token:

```
interpolation = "${" expression "}"
```

The contents of `${ … }` are a **complete expression** (C-023): arithmetic,
comparisons, boolean logic, env defaults, unit literals, function-free
expressions of any depth, and further double-quoted strings (which may
themselves interpolate — nested interpolation). The matching `}` is found by
scanning with awareness of nested `${` and of string literals inside the
braces.

Escaping and parity: `\$` is a literal dollar sign, so `"\${5}"` is the text
`${5}`; `\\` is a literal backslash, so `"a\\${x}b"` is a backslash followed by
an interpolation of `x`. The two cases stay distinct because the lexer marks an
escaped dollar internally and the parser resolves that mark (an implementation
detail: a private-use sentinel character that never reaches string values).

**Single-quoted strings do not interpolate** (spec §1.4): `'${x}'` is the
literal text `${x}`. They process `\\` and `\'` only; other backslash sequences
are preserved verbatim, so a single-quoted string can carry `\n` as two
characters and a real newline literally.

Diagnostics: **E012** unterminated interpolation (no matching `}`), **E013**
empty interpolation (`${}`).

A `Secret` value reaching an interpolation is a hard error (S004) — secrets
may never be rendered into strings.

Every sub-expression inside `${ … }` participates in the dependency graph
exactly as if it appeared in expression position (C-023).

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
  resolution (never a self-cycle) — see `active-core/tests/regressions.rs`.

### 4.2 Resilience and healing (C-016)

Resilient inputs are diagnosed, not silently swallowed. Every repair carries a
confidence and is governed by the mode policy (see `docs/diagnostics.md`):

| Situation | Code | Confidence | Strict mode | Resilient mode |
|---|---|---|---|---|
| String reaches EOF unclosed | H001 | 0.98 | healed (Info) | healed (Info) |
| Trailing comma before a closer | H002 | 0.98 | healed (Info) | healed (Info) |
| Missing comma between elements/entries | H003 | 0.90 | healed (Warning) | healed (Warning) |
| Mismatched `]` / `}` closer | H004 | 0.82 | **error E001** | healed (Warning) |
| Character no rule accepts | E005 | — | **error** | **error** |

`--resilient` relaxes strict mode in two ways: it allows the resilient-only
0.70–0.84 band to apply, and it exits zero despite Warning-severity repairs.
Strict mode (the default) additionally exits non-zero whenever any Warning is
present, even when the repair itself was applied. Characters that no rule
accepts are rejected in both modes — skipping them would silently change the
configuration. Unresolved identifiers warn (W001) in both modes.

### 4.3 Diagnostic codes referenced by this grammar

| Code | Meaning | Emitted by |
|---|---|---|
| E001 | expected token | parser |
| E002 | unexpected token | parser |
| E003 | malformed `secret.env` argument | parser |
| E004 | expression nesting limit exceeded | parser |
| E005 | unexpected character | lexer |
| E006 | tab in leading whitespace | lexer |
| E007 | inconsistent dedentation | lexer |
| E008 | `key:` header with no indented body | parser |
| E010 | unexpected indentation | parser |
| H001–H004 | healing repairs | lexer/parser |
| W001 | unresolved identifier | evaluator |

Full catalog: `docs/diagnostics.md` (C-015), including the H0xx healing range
and the confidence policy (C-016).
