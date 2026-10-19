# Error catalog: every code, worked

The living definitions live in `docs/diagnostics.md`; every code below
links its meaning there. Each worked example was executed against the
shipped binary; copy it to reproduce the exact diagnostic.

## Worked examples

### W001 unresolved identifier (the Norway case)

```text
[country]
name = NO
```

```text
[Warning] W001: unresolved identifier 'NO'
```

Fix: write `false` for false, `"NO"` for the country. Nothing unquoted
ever coerces; the warning is the language refusing to guess.

### T001 dimension mismatch

```text
timeout = 5s + 512MiB
```

```text
[Error] T001: type error: unsupported binary op Add on DurationMs(5000) and 536870912
```

Fix: convert explicitly (durations with durations, bytes with bytes).
`5s` and `512MiB` are different dimensions by design.

### T003 below minimum

```text
schema = { port = Int(min = 1) }
port = 0
```

```text
[Error] T003: schema violation at 'port': below minimum
```

Fix the value or loosen the schema; the span names the exact line.

### T005 unexpected key in a closed table

```text
schema = { srv = Table { host = String, closed = true } }
[srv]
host = "h"
extra = 1
```

```text
[Error] T005: schema violation at 'srv.extra': unexpected key 'extra' in closed table
```

Fix: remove the key or open the table. Closed means closed.

### S001 cycle before any value

```text
a = b
b = a
```

```text
[Error] S001: cycle detected in dependency graph
```

Fix: break the cycle; no value is produced until the graph is acyclic.

### S004 secret in interpolation

```text
let pw = secret("hunter2")
msg = "login ${pw}"
```

```text
[Error] S004: secret used in unsafe context
```

Fix: keep secrets out of strings. The refusal is the feature: logs stay clean.

### E004 nesting budget

65 nested arrays trip `E004: maximum expression nesting depth (64)
exceeded` at the exact column. Flatten the document or raise
`--max-depth` deliberately.

### E006 tabs

A tab-indented line fails `E006: tab character in leading whitespace; use
spaces` at its exact position. Indentation is spaces only, always.

### E023 integer range

```text
x = 9223372036854775808
```

```text
[Error] E023: integer literal out of i64 range: `9223372036854775808`
```

The value saturates at `i64::MAX`, never wraps to a silent wrong number.
A leading `-` is unary, so `-9223372036854775808` evaluates to `i64::MIN
+ 1` with the same code (spec §1.3).

### W011 duplicate key

```text
x = 1
x = 2
```

```text
[Warning] W011: duplicate key 'x'; last wins
```

Last wins, loudly. Warnings fail `active check`: say it once, mean it once.

### E019 immutable overlap

```text
# @ai_immutable: true
model = "acme-4"
@patch("model", "acme-5")
```

```text
[Error] E019: patch error: patch target `model` overlaps ai_immutable `model`
```

Immutable means no patch path may touch it, however well-intentioned.

## Complete code table

| Code | What it means | Fix |
|---|---|---|
| E001 | A required token is missing (unclosed bracket, block, or brace) | Close the construct; the span points at the gap |
| E002 | A token appears where no production accepts it | Remove or rewrite the token; dotted keys do not exist |
| E003 | Malformed `secret.env` form | Use `secret.env("NAME")` or `secret.env("NAME", "default")` exactly |
| E004 | Expression nesting exceeds the parser budget | Flatten, or raise `--max-depth` deliberately |
| E005 | A character no lexical rule accepts; never silently skipped | Remove the character; the lexer guesses nothing |
| E006 | Indentation is spaces only | Replace tabs with spaces |
| E007 | No enclosing indentation level matches | Align to a real level; check for mixed widths |
| E008 | A `key:` header needs a deeper following line | Indent the body or drop the colon |
| E010 | An indent with no block header | Check the header above; the line joins the enclosing block |
| E011 | Structural nesting exceeds the parser budget | Flatten tables/blocks, or raise the budget deliberately |
| E012 | `${` has no closing brace | Close the interpolation |
| E013 | `${}` is empty | Put an expression inside, or escape the dollar |
| E014 | Malformed `@tag` block | Use documented tag shapes only |
| E015 | Malformed include (non-literal/interpolated/absolute/`..` path, or includes under string parsing) | Use a literal relative path from a file parse |
| E016 | Include cycle or depth past 32 files | Break the cycle; flatten the tree |
| E017 | Missing or unreadable include target | Create the file or fix the path |
| E018 | Malformed profile block | Use `profile "name" { ... }` with table bodies |
| E019 | Malformed patch, or patch over `@ai_immutable` | Fix the patch syntax or respect the immutability |
| E020 | Malformed schema block (non-types, bad args) | Use documented types and arguments; Enum members are bare strings |
| E021 | Malformed annotation (bad intent string, non-boolean immutable, empty constraint) | Fix the annotation payload |
| E022 | Malformed dependency block | String array elements; no inline tables there |
| E023 | Integer literal past i64 range; saturates at MAX, never wraps | Narrow the literal or accept the saturation loudly |
| E032 | Included file failed signature enforcement | Provide the sibling sidecar from the trust set |
| S001 | Dependency cycle, detected before any value | Break the cycle |
| S002 | Module has no root table (should not occur via the parser) | Report as a bug with the reproducer |
| S003 | Internal arena id absent (defensive) | Report as a bug with the reproducer |
| S004 | Secret reached string interpolation | Keep secrets out of strings |
| T001 | Operator on unsupported operands, including dimension mixes | Convert explicitly; check unit dimensions |
| T002 | Value kind mismatches schema type (`Int` widens to `Float`) | Fix the value or the declared type |
| T003 | Constraint fails (range, length, pattern, enum) | Fix the value or loosen the constraint |
| T004 | Schema field has no value and no default | Supply the value or declare a default |
| T005 | Unexpected key in a `closed = true` table | Remove the key or open the table |
| T006 | `@constraint` predicate fails | Satisfy the predicate or revise it |
| W001 | Unresolved identifier; value becomes `null` | Spell a real sibling/`let` name, or quote the word |
| W002 | Unknown profile name; no profile merges | Spell a defined profile |
| W003 | Profile value wins over a different-typed base | Align the types or accept the documented win |
| W004 | Patch target missing; succeeds idempotently | Create the target or accept the no-op |
| W005 | Resilient mode filled a schema default | Informational: the fill is validated like written values |
| W006 | TOML export skips null keys | Informational per key; restructure if nulls must travel |
| W007 | Imported key sanitized; duplicates last-win | Check the renamed form (`-` to `_`, digits prefixed) |
| W008 | TOML datetime arrived as string | Use the RFC 3339 text; there is no datetime type by design |
| W009 | Scalar/array root wrapped as `value` | Informational; the document is valid |
| W010 | Unrepresentable number mapped (u64 to float, non-finite to null) | Check the mapped value; precision notes in converters |
| W011 | Duplicate key; last wins | Say it once: remove the duplicate |
| W012 | JSON Schema construct skipped or tightened on import | Read the path; the mapping table is in converters §4 |
| H001 | Syntactic repair applied as Info | Informational; strict still exits non-zero on warnings |
| H002 | Syntactic repair applied as Info | Informational; see H001 |
| H003 | Repair proposed at 0.90 confidence | Accept explicitly or rewrite; strict only suggests |
| H004 | Repair context for an E001 error | Fix the underlying syntax first |
| H005 | Key-typo suggestion, never an auto-rename | Fix the key spelling yourself |
| H007 | Coercion noted under resilient mode | Pass `--resilient` to apply, or fix the type |
| X001 | Node count past `max-nodes` | Shrink the document or raise the budget deliberately |
| X002 | Wall-clock past `max-wall-ms` (excluded from determinism) | Raise the budget or split the evaluation; no-op on wasm32 (see budgets §6) |
| X003 | Value nesting past `max-depth` | Flatten, or raise the budget deliberately |
| X004 | Array past `max-array-len` | Chunk the data or raise the budget deliberately |
| X005 | Key total past `max-keys` | Reduce keys or raise the budget deliberately |
| X006 | Materialized size past `max-value-size` | Shrink values or raise the budget deliberately |