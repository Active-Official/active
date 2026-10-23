# Debugger: steps, breakpoints, replay, reduction (TOOLING §5)

The debugger is a deterministic observer over the shipped evaluator.
It never re-implements evaluation and never reveals secret material.

## 1. Step trace (DEBUG.001)

`Engine::evaluate_with_steps(&module)` returns the same value and
diagnostics as `evaluate`, plus one `StepEvent` per arena node in
topological order:

```
StepEvent { node: u32, path: Option<String>, kind: String,
            tainted: bool, value: JSON, codes: [String] }
```

- `path` is the dotted value path when the node maps to one.
- `value` renders through the canonical redacting JSON mapping.
- `codes` lists diagnostics emitted while that node was evaluated.
- **Locator redaction:** every node inside a `secret(...)` locator
  subtree renders as `"***SECRET***"` with `tainted: true`. A literal
  locator is not Secret-kinded, so this is enforced structurally, not
  by the value kind.

Determinism: same source, environment, profile, mode, and budget →
byte-identical step arrays.

## 2. Breakpoints (DEBUG.003)

Selectors match the first step:

| Selector | Matches |
|---|---|
| `taint` | first step whose value contains a secret |
| `path:<prefix>` | first step whose dotted path starts with the prefix |
| `code:<CODE>` | first step whose diagnostic codes include the code |
| `node:<n>` | the step at topological index `n` |

The CLI prints steps through the match (or nothing and exits 1 when a
selector never matches).

## 3. Replay (DEBUG.004)

`debug::replay` evaluates twice under the same configuration and
compares: the redacted content hash, the ordered diagnostic codes, and
the full step arrays. `--expect sha256:...` additionally pins the hash
(the audit trail's `result` value). Any mismatch exits non-zero.

## 4. Minimal failing case (DEBUG.005)

`debug::reduce(source, code, max_attempts)` deletes lines one at a time
while the target diagnostic code survives, through the real
parse/evaluate path. The result is always a subsequence of the original
lines; the attempt count is bounded and reported. `debug::minimal_case`
wraps it with sizes for a regression bundle.

## 5. CLI

```
active debug --from <file> [--break SELECTOR] [--replay [--expect HASH]]
             [--minimize CODE] [--max-attempts N] [--format json|text]
```

Default action prints the step trace. JSON output wraps
`{breakpoint, matched, total_steps, steps}`; text output is one line per
step, then the breakpoint verdict.

## 6. Secret guarantee

No step, breakpoint, replay report, or reduction bundle contains raw
secret material: the value mapping redacts Secret values, locator
subtrees are replaced structurally, and the reduction path never prints
values. Regression tests pin both the locator case and the CLI text
output.
