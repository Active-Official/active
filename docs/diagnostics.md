# SOLID Diagnostics Catalog

**Status:** living catalog (C-015). Every diagnostic the parser, resolver, and
evaluator can emit is listed here with its stable code, severity, span policy,
and repair policy. When a code is added, it is added here first (spec before
code). The catalog is the contract tools and agents rely on.

**Ranges:** `E0xx` syntax · `S0xx` semantic · `H0xx` healing · `T0xx`
type/constraint · `X0xx` execution budget · `W0xx` warnings.

## Output format

```
<file>:<line>:<col>: [<Severity>] <code>: <message>
```

Spans are 1-based line/column, pointing at the offending token or node position.
Parse-time diagnostics carry spans by construction; evaluation-time diagnostics
carry spans via `Module::node_spans` (recorded by the parser for every node it
creates). A diagnostic without an attainable position omits the
`file:line:col:` prefix. In multi-file documents (includes, C-025), every
diagnostic carries the `file` it came from (`Diagnostic.file`); single-file
parses leave it unset and tools fall back to the input filename.

## Codes

### E0xx — Syntax (parser, severity Error)

| Code | Message | Span | Emitted when |
|---|---|---|---|
| E001 | `expected <TokenKind>, found <TokenKind>` | token | A required token is missing (unclosed `[table`, unclosed `{` block, or a `}` where an indentation block must close by Dedent) |
| E002 | `unexpected token <TokenKind>` | token | A token appears where no production accepts it |
| E003 | `expected \`env\` after \`secret.\` (use secret.env(name))` / `secret.env() requires a string literal env var name` | token | Malformed `secret.env` form |
| E004 | `maximum expression nesting depth (64) exceeded` | token | Expression nesting exceeds the parser-side budget (C-037) |
| E011 | `maximum block nesting depth (64) exceeded` | token | Structural (table/block) nesting exceeds the parser-side budget (C-037) |
| E012 | `unterminated interpolation: \`${\` has no matching \`}\`` | inside the string | A double-quoted string's `${` has no closing brace (C-023) |
| E013 | `empty interpolation: \`${}\` has no expression` | inside the string | A `${}` with no expression (C-023) |
| E005 | `unexpected character '<c>'` | token | A character no lexical rule accepts — never silently skipped |
| E006 | `tab character in leading whitespace; use spaces` | the tab | Indentation is spaces only (spec/dual-syntax.md §4.1); each tab counts as one column |
| E007 | `inconsistent dedentation` | line start | No enclosing indentation level matches; pops to the nearest level below |
| E008 | `block header has no indented body` | the colon | A `key:` header requires a deeper following line; no table is created |
| E010 | `unexpected indentation` | line start | An Indent where no block header opened one; the line joins the enclosing block, and the matching Dedent is discarded by level comparison (spec/dual-syntax.md §6) |
| E014 | `expected a semantic tag ...` / `unknown semantic tag ...` / `a semantic tag must be followed by a multi-line string` | token | Malformed `@tag` blocks (C-024) |
| E015 | `include paths must be string literals` / `interpolation in include paths is a hard error` / `include paths must not escape the including directory` / `include statements require file-based parsing` | token | Malformed include statements (C-025): non-literal paths, interpolated paths, absolute paths and `..` escapes (unless `allow_parent_escapes`), or includes under string-based parsing |
| E016 | `circular include detected: ...` / `maximum include depth (32) exceeded` | token | An include cycle through the file graph, or nesting deeper than 32 files (C-025) |
| E017 | `cannot read include file ...` | token | A missing or unreadable include target (C-025) |
| E018 | `profile blocks may only appear at document root` / `duplicate profile "name"` / `profile blocks need a string name` | token | Malformed profile blocks (C-027) |
| E019 | `patch statements may only appear at document root` / `expected ( after @patch/@remove/@rename` / `patch paths must be string literals` / `interpolation in patch paths is a hard error` / `rename target already exists` | token | Malformed patch statements (C-028) |
| E020 | `schema tables need valid type expressions` / `unknown schema type` / `unknown argument for <Type>` / `Enum needs at least one value` | token | Malformed schema blocks (C-030): non-type entries, unknown types or arguments |

### S0xx — Semantic (evaluation, severity Error)

| Code | Message | Span | Emitted when |
|---|---|---|---|
| S001 | `cycle detected in dependency graph` | none (graph-wide) | The dependency graph has a cycle — detected **before any value is produced** |
| S002 | `missing root` | none | The module has no root table (should not occur via the parser) |
| S003 | `node <id> not found` | none | Internal: an arena id is absent (defensive) |
| S004 | `secret used in unsafe context` | none (by design — never echo positions of secret data into messages) | A `Secret` value reaches string interpolation |

### T0xx — Type / constraint (evaluation, severity Error)

| Code | Message | Span | Emitted when |
|---|---|---|---|
| T001 | `type error: unsupported binary op <op> on <lhs> and <rhs>` / `unsupported unary op <op> on <v>` / `quantity requires numeric value, got <v>` | node | An operator is applied to unsupported operands; dimension mismatches land here (e.g. `5s + 512MiB`) |
| T002 | `schema violation at '<path>': expected <type>, got <kind>` | value node when resolvable through the arena, else the schema field span, else none | A value's kind does not match its schema type (spec/validation.md §3). `Int` is accepted where `Float` is expected (widening); every other mismatch fails, including `Float` where `Int` is expected |
| T003 | `schema violation at '<path>': <detail>` | same as T002 | A constraint fails: `<detail>` is one of `below minimum`, `above maximum`, `too short`, `too long`, `does not match pattern`, `not a member of Enum(<members>)`, `invalid pattern in schema`, `incomparable <bound> bound for <type> fields`. Enum members are schema literals, safe to quote; evaluated values and bounds are never echoed (secret invariant) |
| T004 | `schema violation at '<path>': missing required key (no default)` | schema field span | A schema field has no value and declares no `default` (C-032 fills defaults; C-031 only reports). Absent `Null`-typed keys are allowed and never reported |

### W0xx — Warnings

| Code | Message | Span | Emitted when |
|---|---|---|---|
| W001 | `unresolved identifier '<name>'` | node | An identifier matches no sibling key and no `let` binding; the value becomes `null` |
| W002 | `unknown profile '<name>'` / `active_profile must be a string` | node | `active_profile` names no defined profile, or is not a string; no profile merges (C-027) |
| W003 | `profile '<name>' overrides '<path>' with a different type` | node | A profile value wins over a base value of a different type (C-027) |
| W004 | `remove target '<path>' does not exist` / `rename source '<path>' does not exist` / `array index <i> out of bounds at '<path>'` | node | A patch target is missing; the operation succeeds idempotently (C-028) |

### H0xx — Healing (implemented in C-016)

| Code | Message | Confidence | Strict | Resilient |
|---|---|---|---|---|
| H001 | `string literal reached end of input without a closing <q>; inserted` | 0.98 | applied (Info) | applied (Info) |
| H002 | `removed trailing comma before closing <delim>` / `removed stray comma` | 0.98 | applied (Info) | applied (Info) |
| H003 | `inserted missing comma between <elements\|table entries>` | 0.90 | applied (Warning) | applied (Warning) |
| H004 | `mismatched closing delimiter; replaced <a> with <b>` | 0.82 | E001 error | applied (Warning) |
| H005 | `unknown key '<typo>', did you mean '<suggestion>'?` | caller-set | — | — |

Strict mode exits non-zero whenever any Warning is present (so an applied H003
still fails the process); `--resilient` exits zero in that case. Repairs below
the 0.70 floor are never applied — the input is rejected instead (E005 for
characters no rule accepts).

Phase 1 codes (H001–H004) are emitted by the lexer and parser; H005 is the
phase-2 semantic key repair: the similarity engine
(`normalized_levenshtein` + `keyboard_similarity` + `phonetic_similarity`,
combined with a noisy-OR) and `best_key_match` are implemented and tested;
the schema-driven wiring lands with C-030. Phase-3 coercions
(`try_coerce_int`, `try_coerce_duration`, `sentinel_meaning`,
`coerce_string_to`) are implemented as pure functions, likewise wired to
schemas in C-030.

### X0xx — Execution budget (reserved for C-037)

Reserved for evaluator-side budget violations: node count, wall-clock, array
length, total keys. The parser-side nesting limit already uses E004.

## Repair policy and confidence

Every diagnostic may carry a `Repair { original, suggested, kind, safe }` and a
`confidence` in `[0, 1]`. The confidence thresholds are fixed by the design:

| Confidence | Action |
|---|---|
| ≥ 0.95 | auto-apply |
| 0.85 – 0.94 | auto-apply and warn |
| 0.70 – 0.84 | apply only in resilient mode |
| < 0.70 | never auto-apply |

C-016 implements the H0xx repair range (see the table above); E004 is an
exact-position hard limit and W001 an unknown-name warning, neither of which
carries a repair suggestion.

## Severity levels

`Error` (evaluation cannot continue / the CLI exits non-zero), `Warning`
(the value is produced; `check` still reports), `Info`, `Hint` (linter-class
advisory, C-042).

## What is deliberately absent

- **No message ever contains secret material** (invariant; the proptest suite
  asserts it).
- **No `TODO` markers here** — this file is a deliverable, not code.
- The alpha does not emit machine-readable JSON diagnostics per run yet; the
  stable codes and spans above are the contract that the C-045 LSP and the
  `--format json` output path build on.
