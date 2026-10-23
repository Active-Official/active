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

All formats exit `1` when any enabled rule fires, `0` otherwise.

## Disablement

```sh
active lint --disable-lint unused-lets --disable-lint data-mode file.active
```

Unknown names are an error listing the ten rules. Library callers pass
`LintOptions { disabled }` to `active_core::lint::lint_with`.
