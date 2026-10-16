# SOLID Dual-Syntax Specification (C-020, revision 3 — reviewed, ready)

**Status:** normative design, C-020. Written before any parser change and
reviewed adversarially first (two review rounds, nine findings in round one
and six in round two, all resolved). This revision adds: the empty same-line
table block, E007 limited to unmatched dedentation, deeper-header recovery
with scope preservation, four ordered comment-attachment rules with explicit
EOF precedence, the same-line trailing comment in the trivia model, an
explicit statement of what is not preserved, and the E006 column rule.

---

## 1. The principle

A SOLID document may be written in two equivalent layouts:

- **Brace style** — explicit delimiters: `[name]` headers, `name { … }` blocks,
  and inline `{ … }` tables.
- **Indentation style** — implicit delimiters: a `key:` header followed by an
  indented block.

Both layouts describe the same tree. The two forms lower to the **identical
IR** (modulo source spans and trivia) — the dual-syntax guarantee C-021 tests.

```
# brace style                     # indentation style
[app]                             app:
name = "svc"                        name = "svc"
port = 8080                         port = 8080

app2 { }                          app2:
name = "svc2"                       name = "svc2"
```

## 2. Statement forms (normative)

```
statement      = let-decl | table-header | table-block | key-value | block-header
table-header   = "[" identifier "]"                 TOML-style (brace style)
table-block    = identifier "{" [ newline { statement } ] "}"
                                                    HCL-style (brace style)
block-header   = identifier ":" newline indent      indentation style (§4)
key-value      = identifier ( "=" | ":" ) expression
```

`identifier "{"` at statement position is always a **table block**; an inline
table can only appear as a *value* (`x = { … }`), so the two never conflict.
The body may be empty, either on one line (`app2 { }`) or with the closing
brace on its own line. `identifier ":"` followed by a value on the same line is
a key-value pair; followed by a newline (and an indent) it is a block header
(§4).

## 3. Where indentation is measured

The lexer maintains a **bracket depth** counter, incremented on `(`, `[`, and
`{` and decremented on their closers. Indentation is measured **only** when all
of the following hold at the start of a physical line:

1. bracket depth is 0;
2. the line start is not inside a string token (strings consume their own
   newlines atomically, so a line beginning inside a string is never measured);
3. the lexer is not inside a multi-line string.

When indentation is not measured, **no Indent/Dedent token is emitted and the
stack is not changed**, and the tab rule (E006) does not apply. This means:

- Multi-line arrays, parenthesised expressions, and brace blocks suppress
  indentation entirely: their interiors are governed by delimiters, and lines
  inside them may be indented or not (cosmetic).
- Consequently, an indented block can never be opened inside `(`, `[`, or
  `{`. Inside a brace block, nested tables use `[name]` or `name { }`.

## 4. The indentation stack

### 4.1 Measurement

Leading whitespace of a measured line consists of spaces. A tab in that
leading whitespace is **E006** (spaces only; tab width varies between editors
and guessing would make the tree ambiguous). Tabs remain legal elsewhere.

### 4.2 Stack updates

The stack holds indentation columns, initialised to `[0]`.

| Condition | Effect |
|---|---|
| greater than the top | push the column, emit **Indent** (any increase is a new level) |
| equal to the top | nothing |
| less than the top and matching a stack level | pop to that level, emitting one **Dedent** per pop |
| less than the top and matching no stack level | **E007** — only unmatched *dedentation* is an error; the stack pops to the nearest level *below* the column and does **not** push; the line joins that level |
| the line is blank, or contains only whitespace and/or a comment | no tokens, **no stack change** |

At end of input the lexer emits one **Dedent** per remaining stack level.

### 4.3 Level width

A level's width is simply the column its first line uses; later lines at the
same level must use exactly that column. Increases of any size open a new
level (2 spaces then 3 spaces is legal but confusing — it is not an error, it
is two levels). There is no fixed-width requirement.

## 5. Blocks and their termination

A **block header** is `identifier ":" newline` where the next measured line is
more deeply indented. Its body is every following statement until the matching
**Dedent**. If the next measured line is not deeper (or the file ends), the
header has no body and is **E008**.

### 5.1 Termination table

| Construct | Body ends at |
|---|---|
| block header (`key:`) | the matching Dedent |
| `name { … }` table block | its closing `}` |
| `[name]` header, at bracket depth 0 outside any block | the next `[` header, `let`, or EOF (existing rule) |
| `[name]` header inside an enclosing block | the next header/`let`, or a Dedent that goes **below the column at which the header itself appeared** |

### 5.2 Bracket headers inside an indented block

`[nested]` inside an enclosing table introduces the table `nested` **as a child
of that enclosing table**, at the point the header appears. Its body is
governed by the last row of §5.1: it continues while statements stay at the
header's own column or deeper, and ends at a Dedent past that column, the next
header, or `let`. This is the only reading: a header never escapes its
enclosing block.

**A header must appear at its enclosing block's own indentation column.** If a
header is written more deeply indented than the statements around it, the
Indent before it is E010 (§6) and the header is parsed at the enclosing
block's level; the following lines are then measured normally.

### 5.3 Delimiter discipline

- A `key:` header can only be closed by its Dedent; a `}` at that point is
  **E001** (delimiter mismatch) — the brace cannot close an indented block.
- A `[name]` header is never closed by `}`.
- Indentation never closes a brace block; only `}` does.
- Unbalanced braces are **E001**; unbalanced indentation is **E008/E007**.

## 6. Unexpected indentation and recovery

An Indent token is meaningful **only** immediately after a `key:` header. Any
other Indent (for example `x = 1` followed by an indented `y = 2`, or an
indented first line) is **E010 unexpected indentation**: the parser emits the
diagnostic with the line's span, consumes the Indent token, and continues
parsing the line as a statement of the **enclosing** block.

To keep block scope intact, the parser records the indentation depth opened by
that Indent and **consumes its matching Dedent** when it arrives. Without this,
a recovered indent would leave a stray Dedent that closes the enclosing block
early. Recovery is therefore scope-preserving: the tree is exactly what a
reader would expect, and no statement is lost.

E007 recovery: as specified in §4.2, the offending column is discarded (the
stack does not push), so subsequent lines are classified against the surviving
levels. One diagnostic per offending line.

## 7. Comment and annotation attachment

Comments and annotations are **trivia**: they never affect the value tree, and
parse equivalence is defined modulo trivia. But they must be re-emittable, so
attachment is normative and total — **every comment receives exactly one
attachment**.

### 7.1 Determining a comment's block

- **Indentation style:** a comment's block is the innermost block whose
  statements are at the comment's own indentation column.
- If the comment's column matches no statement level, its block is the
  innermost block whose level is **less than** the comment's column (the block
  it visually indents into).
- If no such block exists, its block is the **document**.
- **Brace style:** a comment's block is the innermost brace block it appears
  in (indentation is cosmetic inside braces).

### 7.2 Attachment precedence

Apply in order; the first matching rule wins. Every comment receives exactly
one attachment:

1. A comment **after code on the same line** is a **trailing comment of that
   statement** (`Trailing(statement)`).
2. Otherwise, if a statement **follows the comment within the same block**, the
   comment is a **leading comment of that next statement** (consecutive
   comments attach in order).
3. Otherwise, if the comment's block is **not** the document, it is a
   **trailing comment of that block**.
4. Otherwise the comment is a **trailing comment of the document**.

This resolves the overlap: the comment in `a:` / `x = 1` / `# note` / `b = 2`
belongs to the enclosing block (its column is at the outer level), where a
statement does follow — so it is the leading comment of `b = 2`, and it is
*not* a trailing comment of `a`. A comment after the last statement of `a:`'s
body, before a dedent to the outer level, belongs to `a` and is its trailing
comment (rule 3). A comment after the last statement of the file is the
document's trailing comment (rule 4).

Annotations (`# @intent: …`, `# @constraint: …`, `# @ai_immutable`,
`# @ai_context: …`) follow the same attachment rules and additionally attach
semantically to their statement (C-033).

## 8. The trivia model (for lossless round-tripping)

The parser records trivia alongside the IR so the formatter can re-emit it:

```
Trivia = Comment { span, text, attachment }
       | BlankLines { count, before_span }
Attachment = Leading(statement)        // own-line comment(s) before a statement
           | Trailing(statement)       // after code on the same line
           | TrailingBlock(block)      // no following statement in that block
           | TrailingDocument          // no following statement in the document
```

- A **statement's span, its leading comments, its same-line trailing comment,
  and the blank-line counts around it** are together enough to re-emit the
  statement in canonical form.
- **Blank lines** between statements are recorded as counts with their
  position; canonical modes collapse runs to at most one blank line between
  top-level blocks and none inside a block; preserve mode reproduces the
  recorded counts exactly.
- **Canonical layouts** are defined as: indentation style uses two spaces per
  level and `key:` headers; brace style uses `[name]` headers, `key = value`
  pairs, and one entry per line.
- **What is not preserved:** intra-line spacing that is not canonical (for
  example extra spaces around `=` in a non-canonical file, or a blank line
  inside a block in canonical mode). Preservation is a guarantee about
  *canonical* documents and about comments/blank-line structure, not about
  arbitrary whitespace.
- **CLI flag names:** `solid-fmt --bracketed` emits brace style and
  `solid-fmt --indented` emits indentation style (`--write` edits in place).
  "Bracketed" and "brace style" are the same layout; "indented" and
  "indentation style" are the same layout.

### 8.1 The round-trip guarantee (tested in C-022)

1. **Parse equivalence.** Two documents that differ only in style produce
   identical IR modulo spans and trivia.
2. **Idempotence.** `fmt(fmt(D)) == fmt(D)` for each canonical style.
3. **Preservation.** `fmt_preserving(D) == D` byte-for-byte for any document
   that is already canonical in its layout (per the definition above) — which
   includes every document the canonical formatter itself produces.
4. **Trivia preservation.** No mode drops, duplicates, or reorders comments
   relative to the attachment rules of §7, and blank-line counts are
   reproduced in preserve mode.

## 9. Diagnostics introduced by this design

| Code | Meaning | Span | Recovery |
|---|---|---|---|
| E006 | tab in leading whitespace | the tab | each tab counts as one column for measurement, so the leading width is spaces + tabs; the first tab on the line emits E006 once; measurement continues |
| E007 | inconsistent dedentation (unmatched column **below** the top) | line start | pop to the nearest level below, do not push (§4.2) |
| E008 | `key:` header with no indented body (including inside a brace block) | the colon | consume the header; no table is created |
| E010 | unexpected indentation | line start | consume the Indent, parse the line in the enclosing block, and discard its matching Dedent by level comparison (§6) |
| E011 | structural nesting limit exceeded (64 levels) | line start | the body is skipped to its terminator; no further recursion (§11) |

E001 continues to cover delimiter mismatches. All codes are added to
`docs/diagnostics.md` with the severity/span policy stated here.

## 10. Structural nesting limit

Block nesting (`key:` blocks, brace blocks, and `[header]` bodies) is bounded
at **64 levels** by `MAX_BLOCK_DEPTH`, mirroring `MAX_EXPR_DEPTH` for
expressions. Exceeding it emits **E011** at the offending line, skips the body
to its terminator, and continues — so pathological input can never exhaust the
parser's stack. The evaluator-side budgets arrive with C-037.

## 11. Dedent levels

To make recovery precise, an **Indent token carries the column it opens** and
a **Dedent token carries the column it dedents to**, both as token text. A
block body ends only at a Dedent whose target is **below** the block's own
level; a Dedent at or above it closes a nested (possibly recovered) block
instead. This is what makes E010 recovery scope-preserving: a stray Indent
adds exactly one level, and its Dedent is recognised by level rather than by
position, so it can never swallow an enclosing block's Dedent.

## 12. Non-goals

- No significant indentation inside expressions or bracketed constructs (§3).
- No tab/space mixing arithmetic; spaces or E006.
- No third reading of `key:` — it is a block header or a key-value, never both.
- Indentation never changes a value's meaning, only which table it belongs to.
