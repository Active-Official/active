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
`file:line:col:` prefix.

## Codes

### E0xx — Syntax (parser, severity Error)

| Code | Message | Span | Emitted when |
|---|---|---|---|
| E001 | `expected <TokenKind>, found <TokenKind>` | token | A required token is missing (e.g. unclosed `[table`) |
| E002 | `unexpected token <TokenKind>` | token | A token appears where no production accepts it |
| E003 | `expected \`env\` after \`secret.\` (use secret.env(name))` / `secret.env() requires a string literal env var name` | token | Malformed `secret.env` form |
| E004 | `maximum expression nesting depth (64) exceeded` | token | Expression nesting exceeds the parser-side budget (C-037) |

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

### W0xx — Warnings

| Code | Message | Span | Emitted when |
|---|---|---|---|
| W001 | `unresolved identifier '<name>'` | node | An identifier matches no sibling key and no `let` binding; the value becomes `null` |

### H0xx — Healing (implemented in C-016)

| Code | Message | Confidence | Strict | Resilient |
|---|---|---|---|---|
| H001 | `string literal reached end of input without a closing <q>; inserted` | 0.98 | applied (Info) | applied (Info) |
| H002 | `removed trailing comma before closing <delim>` / `removed stray comma` | 0.98 | applied (Info) | applied (Info) |
| H003 | `inserted missing comma between <elements\|table entries>` | 0.90 | E001 error | applied (Warning) |
| H004 | `mismatched closing delimiter; replaced <a> with <b>` | 0.82 | E001 error | applied (Warning) |
| H005 | `unknown key '<typo>', did you mean '<suggestion>'?` | caller-set | — | — |
| H006 | `unexpected character '<c>' skipped` | 0.60 | warning | warning |

Phase 1 codes (H001–H004, H006) are emitted by the lexer and parser. H005 is
the phase-2 semantic key repair: the similarity engine
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

No current code emits repairs yet — E004/W001 are exact-position diagnostics
where a repair is either trivial (nesting limit) or impossible (unknown name).
C-016 turns on the H0xx repair range.

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
