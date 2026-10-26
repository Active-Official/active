# Lint rule catalog (stable IDs)

The linter is static: it reads the parsed module, never evaluates, and
never changes a diagnostic verdict. Every rule is independently
disableable and emits a stable `L0xx` code at Warning severity
(spec/linter.md is normative).

## Rules

| ID | Name | Category | Fires when | Mode dependence |
|---|---|---|---|---|
| L001 | `unused-lets` | Hygiene | A `let` binding is never referenced (values, patches, constraints, schema args, interpolations all count) | None |
| L002 | `depends-on` | Correctness | A table declares `depends_on` but omits a reference, lists an unresolvable path, lists its own scope, or is empty while references exist | None |
| L003 | `complexity` | Performance | One key or binding holds more than 100 operator nodes (data nodes are free) | None |
| L004 | `hardcoded-secrets` | Safety | A credential-named key holds a plain string that is neither a placeholder nor a URL, or a string has Shannon entropy >= 4.5 bits/char at length >= 20 | None |
| L005 | `deprecated` | Schema | A present value sits under a schema field marked `deprecated` | None |
| L006 | `schema-evolution` | Schema | The schema uses `min_reader`, `min_writer`, or `removed_in` without declaring `version` | None (a fully unversioned schema is allowed) |
| L007 | `unknown-capability` | Agent ergonomics | `active_capabilities` names a capability the descriptor does not advertise | None |
| L008 | `key-style` | Style | One table mixes cased (camel/Pascal) keys with snake_case keys | None |
| L009 | `large-literal` | Performance / size | A string literal exceeds 65,536 bytes; consider `blob()` content addressing | None |
| L010 | `data-mode` | Mode discipline | The document uses content-addressed values, evolution metadata, or capability declarations without `active_mode = "data"` | Config Mode only; silent in Data Mode |
| L011 | `requires-discipline` | Schema & evolution | A schema field's `requires` target is empty, names itself, or does not resolve to another schema path | None |
| L012 | `agent-annotations` | Agent ergonomics | A node carries `# @constraint` with no `# @intent` description, so an agent cannot read the reason behind the predicate | None |
| L013 | `ambiguous-repair` | Agent ergonomics | Sibling keys normalize to the same case/punctuation-insensitive identity, so a rename repair cannot choose between them | None |
| L014 | `canonical-order` | Style | `depends_on` appears after ordinary keys in its scope (canonical position: after `schema`, before keys) | None |
| L015 | `mixed-syntax` | Style | One document declares both brace-style and indentation-style tables | None |
| L016 | `include-hygiene` | Style | An include is absolute, escapes its directory with `..`, does not end in `.active`, or repeats a target | None |

## Examples

```active
# L001: idle is never used
let idle = 1
port = 8080
```

```active
# L006: partial evolution metadata
schema = { min_reader = "1.0.0", port = Int }
```

```active
# L008: one table, two key styles
[server]
readTimeout = 5s
max_retries = 3
```

```active
# L010: Data construct without the marker
id = blob("text/plain", "hello")
```

## Output formats

- `text` (default): `file:line:col: [Warning] L00x: message`.
- `json`: a stable array of `{file, severity, code, message, path, span}`.
- `sarif`: SARIF 2.1.0 (`runs[0].tool.driver.name = "active"`), suitable
  for code-scanning upload.

All formats exit `1` when any enabled rule fires, `0` otherwise. A file
that does not parse still fails the run: its parse diagnostics go to
stderr, and the JSON/SARIF item list stays lint-only (the pinned schema
admits `^L[0-9]{3}$` codes).

## Disablement

```sh
active lint --disable-lint unused-lets --disable-lint data-mode file.active
```

Unknown names are an error listing the sixteen rules. Library callers pass
`LintOptions { disabled, severity }` to `active_core::lint::lint_with`.

## Project configuration (`TOOLING-PROPOSAL` §2.4)

`active lint` reads `.active-lint.toml` from the input file's directory
or the nearest ancestor. `--config FILE` pins a specific file and
skips discovery. The format is deliberately small:

```toml
disable = ["unused-lets"]

[severity]
key-style = "error"     # off | error | warning | info | hint
```

- `disable` is an array of rule IDs; it unions with `--disable-lint`.
- `[severity]` overrides one rule's severity. `off` drops the rule's
  findings entirely; the other levels map to the diagnostic severity
  carried into text, JSON, and SARIF (the pinned schema admits all
  four levels).
- Unknown rule names and unknown keys are errors. A malformed
  discovered file fails the run instead of being ignored, so a typo
  cannot silently disable a rule.
- Configuration applies per file, which lets a workspace scope rules
  to a subtree without a global switch. Core rule evaluation stays
  deterministic: the same module and the same options always produce
  the same findings.
