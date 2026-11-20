# ACTIVE Language Grammar (v0.9 — alpha surface)

**Status:** normative for the alpha surface (C-013). This grammar describes the
language as implemented in `ACTIVE-core`. A second implementer following this
document should accept exactly the same inputs. Deviations are marked
**[impl note]**.

Spec-before-code rule: when the grammar changes, this file changes first.

---

## 1. Lexical grammar

### 1.0 Input text

Source text is UTF-8. A single leading byte-order mark (U+FEFF) is skipped
before lexing and never becomes a token (research HIST-YAML-021). Inside
string literals, `\r\n` contributes one newline and a lone `\r` becomes LF
(research HIST-YAML-014); explicit `\r` escapes still decode to CR.

### 1.1 Character classes

```
digit        = [0-9]
hex-digit    = [0-9a-fA-F]
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
keyword      = "let" | "true" | "false" | "null" | "env" | "secret" | "blob"
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
escape       = "\" ( "n" | "t" | "r" | "\" | '"' | "0" | "$" )
             | "\" ( "'" | "\" )               (single-quoted form)
```

Escape results: `\n` → LF, `\t` → TAB, `\r` → CR, `\\` → backslash, `\"` →
quote, `\0` → NUL, `\$` → a literal `$` (the next characters never open
interpolation). An unknown escape in a double-quoted string keeps the
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

### 1.4.2 Include content pins (L-P06)

```
hash-pin     = "sha256" ":" 64 * hex-digit
```

- The keyword, colon, and all 64 hex digits must be adjacent: no
  whitespace is allowed inside the pin. A `sha256:` followed by fewer or
  more than 64 hex digits is not a pin token; in include-option position
  it is a malformed statement (E015).
- Hex digits are case-insensitive for comparison; the token text keeps
  the source case.
- The token is reserved in every position at the lexer level, not only
  after `include`: a document that spells `sha256:<64 hex>` as a value
  gets one token. It has meaning only as an include option; elsewhere it
  is a malformed value.
- The digest is SHA-256 over the included file's bytes (the bytes the
  loader returned), never over a re-read file.

### 1.5 Numbers

```
integer      = digit { digit | "_" }
float        = integer "." digit { digit }
number       = integer | float
```

Underscores are digit separators and are stripped (`1_000` → 1000). The
grammar accepts an underscore anywhere in an integer, so a leading, trailing,
or doubled underscore is stripped as well (`1_` → 1, `1__000` → 1000). Leading
zeros are accepted and stripped: `010` is ten, never octal. These acceptances
are deliberate and pinned by the research corpus (LANG-A-008): Active has one
numeric base — decimal — and no digits-as-octal reading. A dot must
be followed by a digit to be part of the number; `1.` lexes as `integer(1)`
then a `.` operator token. There are no negative literals — `-5` is unary
minus applied to 5.

### 1.6 Unit suffixes

```
unit         = unit-char { unit-char }
```

A unit attaches to a number with **no intervening whitespace** and must be in
the known table. Unit characters are identifier characters plus `/`, `_`,
and `%` (percent is the only symbolic suffix; case-SENSITIVE always):

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
| `%` | percent (dimensionless ratio, 100% = 1.0) | | `Hz kHz MHz GHz THz` | frequency (hertz, SI) |
| `b Kb Mb Gb` | bits (binary magnitude, distinct from bytes) | | `bps Kbps Mbps Gbps MB/s` | data rates (bits/sec canonical) |
| `week sec min` | week (exact 604800000ms); long aliases for `s` `m` | | | |
| `tokens ktokens Mtokens` | tokens (count, longest-match prefix composition) | | `tok/s` | tokens per second (inference throughput) |
| `FLOPS TFLOPS PFLOPS` | floating-point ops per second (compute accounting) | | | |
| `Pa bar psi` | pressure (pascals canonical) | | `V A W` | electrical (volts/amps/watts canonical) |
| `J kWh` | energy (joules canonical) | | `N` | force (newtons canonical) |
| `deg rad` | angle (radians canonical) | | `rpm` | rotation rate (retained) |
| `meter km` | length (meters canonical, long-form only) | | `kg g` | mass (kilograms canonical) |
| `PiB EiB` | binary bytes (saturating u64) | | | |
| `K Cel Fahr` | temperature (kelvin canonical, affine) | | `dB` | decibels (log special, ordering only) |

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
- Keys are identifiers only: a string literal at statement position
  (e.g. `" key " = 1`) is E002, never silently dropped (research
  HIST-YAML-020).
- An indented block ends at its Dedent; a brace block at its `}`.

### 2.1 Document


A statement ends at the newline. A token that follows a complete value on the same line is announced as **W015** (the documented value is kept): two statements must not share a line, and a trailing token such as `1 2 3` or `1.2.3` is not silently dropped. The two intentional quirks in §4.1 (`x = 5 5` and `d = 1.`) are the only exception. A comma between entries inside a block or inline table is a separator, not a stray. A missing operand after a binary or unary operator is **E001** at parse time.
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
the body belongs to the leaf. Repeated table bodies merge at shared
prefixes (`[a.b]` then `[a.c]` keep both). A repeated leaf unifies
(L-P09, 2.0.0) for every body form: `[header]`, brace `name { ... }`,
indentation `name: ...`, and an inline `name = { ... }` followed by a
body. Tables merge recursively at Local priority, equal values agree,
and a scalar, array, or shape disagreement at a path is **E046 merge
conflict**; the first body's value stays in resilient mode. An include
inside a later body never overrides an earlier local leaf (Local beats
Include at every level). The formatter re-emits every repeated body as
its own statement. Descending through a present non-table value is
E001. After a table header, key-values belong to that table until EOF,
the next `[`-header, or a `let` (a `let` inside a table body ends the
table).

### 2.7 Include statements (C-025)

```
include-stmt  = "include" string-literal [ include-option ]*
include-option = hash-pin | "as" identifier | "private"
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
- An inline content pin `sha256:<64 hex>` binds the include to the exact
  bytes read (L-P06): the digest is SHA-256 over the file's contents, hex
  is case-insensitive, and a mismatch is **E041** before the included
  file is parsed. The pin makes the importing document self-describing -
  a copied or substituted include cannot pass. The canonical formatter
  emits the pin after the path, before `as`/`private`.
- Options may appear in any order (`include "lib.active" private
  sha256:<hex> as lib`); repeating any option, or a malformed pin, is
  E015 and never falls through to an ordinary key-value statement.
- Lockfile pinning (L-P03): when evaluation is given a lockfile
  (`--lockfile`, or the sibling `active-lock.toml` under `--trust-keys`),
  every include is resolved against its recorded digest as well, matching
  the canonical resolved path first and the document spelling second. A
  mismatch is E041. A lockfile that pins spellings is an allow-list of
  edges: an include absent from it is E041, even when its bytes match
  another pinned file. A legacy lockfile without identity pins resolves
  by content and warns W017 on a miss unless the include already carries
  a matching inline pin. Both pin sources apply when both exist;
  spec/reproducibility.md §4 owns the lockfile side.
- Paths resolve against the including file's directory. Absolute paths and
  paths escaping the directory via `..` are E015 by default (opt out with
  `allow_parent_escapes`, e.g. the CLI `--allow-parent-escapes` flag).
- A missing or unreadable file is E017. Including a file already on the
  include stack (a cycle), or nesting deeper than 32 files, is E016.
- Path portability is enforced on every host (LANG-H-004): include paths
  use `/` separators and are relative to the including file. A backslash
  separator or a Windows drive-letter absolute (`C:\x`, `C:/x`) is
  **E026** on all platforms — the form cannot mean the same file
  everywhere, so it is refused instead of resolving differently per host.
- Merge semantics (normative, 2.0.0; L-P09): **included files provide
  defaults, and includes are equal peers to one another.** Local keys beat
  included values at every level (Local over Include), regardless of
  position. Tables merge recursively. Two included files are the same
  priority: equal values agree, and a scalar, array, or table/array shape
  disagreement at a path is **E046 merge conflict**, naming the path and
  both sources. E046 is an error in strict and resilient modes; resilient
  evaluation keeps the first source in source order, so the exported tree
  is deterministic. Included includes compose by the same rule (each
  included file's local keys win inside that file before it participates
  as one included value). Secrets never compare equal, so a path both
  includes fill with material is E046 rather than an equality oracle. Deep
  structural composition beyond defaults is the domain of profiles (C-027)
  and patches (C-028/029). An included file's `let` bindings are visible
  to the including module but never exported.
- Included root metadata: `active_mode` and `active_capabilities` stay
  local to the main document (mode is a host decision, spec/mode.md
  §1). A root `active_profile` is an ordinary value that the evaluator
  reads after the merge, so an include may supply it as a default and
  the included profile block then applies (spec/specification.md §3.5).
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
schema-table  = "{" [ schema-entry { "," schema-entry } ]
                { "," check-block } "}"
schema-entry  = identifier "=" schema-type
check-block   = "check" ( "{" [ check-entry { "," check-entry } ] "}"
                        | ":" indented-check-body )
check-entry   = identifier "=" expression
schema-type   = bare-type [ "(" [ schema-arg { "," schema-arg } ] ")" ]
              | "Array" "<" schema-type ">"
              | "Table" [ "{" [ schema-entry { "," schema-entry } ] "}" ]
              | "Enum" "(" string { "," string } [ "," named-args ] ")"
bare-type     = "Int" | "Float" | "String" | "Bool" | "Null"
              | "Duration" | "Bytes" | "Secret" | "Blob" | "Any" | "Table" | "Array"
schema-arg    = identifier "=" expression
```

`check:` is valid in the indentation-form schema body (`schema:`); a
brace-body schema uses `check { ... }`. Checks are accepted only in the
document's root schema (spec/validation.md section 11). Check blocks may
appear before or after entries and more than once. A nested data
table, a `let` table, a profile, or an included file that declares one
is E020.


`Blob` fields additionally accept `media_type = "<mime>"` and
`max_size = <int>` (spec/blob.md); evolution metadata accepts
`removed_in = "<major.minor.patch>"` (spec/schema-evolution.md).

- `schema` is contextual: a `schema` key with a **table value** parses
  as a schema block; a `schema` key with any other value stays ordinary
  data. A schema table whose entries are not valid type expressions is
  E020 (a mistyped schema must never silently become data).
  The table value includes the header form: `[schema]` (and `[t.schema]`
  for a nested scope) declares the same schema block as `schema { ... }`
  and `schema:`, with its body ending at the next header or document-level
  statement. The header form is what brace-style formatting emits for a
  root schema whose body is not empty; treating it as data let a formatter
  change a document's meaning when it re-emitted the indented form
  (LANG-C-004 follow-up, fuzz seed crash-504a8155).
- Named arguments per type (anything else is E020): `Int`/`Float` take
  `min`, `max`, `default`; `Duration`/`Bytes` take `min`, `max`,
  `default`; `String` takes `min_len`, `max_len`, `pattern`, `format`,
  `default`; `Bool`/`Null`/`Secret`/`Any` take `default` only; `Enum` takes
  positional strings plus optional `descriptions = [...]` and
  `deprecated`; `Array` takes only `<T>`; `Table` takes only `{...}`
  with an optional reserved `closed = true`. Every type additionally
  accepts `description = "..."` (metadata, never a verdict) and
  `requires = "<dotted-path>"` (conditional presence,
  spec/schema-extensions.md section 4).
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
  `?:` default when present). The default is optional. NAME is
  case-insensitive on every platform: names fold to uppercase at snapshot
  time, so `env.Path` and `env.PATH` are the same variable (LANG-H-003).
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
`null` value. The limit counts nesting *inside* a value: a value wrapped in 64
parentheses is legal, the 65th trips; the same rule applies to brace blocks
(64 nested tables parse, the 65th is E011) and to schema wrappers (64
`Array<…>`/`Table {…}` levels parse, the 65th is E004). One over-deep
construct produces one root-cause diagnostic: recovery consumes the offending
balanced construct rather than emitting one follow-on error per enclosing
level. This is the parser-side half of the execution budget (C-037
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
`spec/specification.md` Chapter 3 (C-014) for the full lowering and
evaluation rules.

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
| E023 | integer literal out of i64 range (saturates at `i64::MAX`) | parser |
| H001–H004 | healing repairs | lexer/parser |
| W001 | unresolved identifier | evaluator |

Full catalog: `docs/diagnostics.md` (C-015), including the H0xx healing range
and the confidence policy (C-016).
