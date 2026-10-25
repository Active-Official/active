# ACTIVE Linter (C-042)

Static checks beyond validation: style, hygiene, and probable
mistakes. Every rule is independently disableable, emits stable
`L0xx` diagnostics (Warning severity) that the language server
surfaces as-is (C-043), and never fails evaluation — `ACTIVE lint`
is a separate check tool that exits 1 when any enabled rule fires.

## 1. Rules

| ID | Rule | Fires when | Severity |
|---|---|---|---|
| `unused-lets` (L001) | Unused `let` binding | A binding is never referenced by any `Ref` (values, patches, constraints, schema args, interpolations all count as uses) | Warning |
| `depends-on` (L002) | Incomplete or dangling `depends_on` | A table declares `depends_on` but references something unlisted, lists an unresolvable path, lists its own scope, or is empty while references exist | Warning |
| `complexity` (L003) | Excessive expression complexity | One key/let value holds more than 100 operator nodes (`Binary`, `Unary`, interpolation expressions, references, env/secret lookups). Data nodes (literals, arrays, tables) are free: size is not complexity | Warning |
| `hardcoded-secrets` (L004) | Probable hard-coded secret | A pure string literal sits under a credential-named key (`password`, `passwd`, `secret`, `token`, `api_key`, `apikey`, `private_key`, `privatekey`, `auth`, `credentials`, `access_key`, `secret_key` — case-insensitive, `_`/`-` agnostic, prefix/suffix match), unless the value is a known placeholder (`changeme`, `password`, `xxx`, `test`, `example`, `placeholder`, `redacted`, `***`, `***SECRET***`, empty) or a `scheme://` URL locator; **or** any string literal with Shannon entropy ≥ 4.5 bits/char at length ≥ 20 (precision-tuned: no per-file suppression exists yet, so the bar stays high; URLs are not exempt here — token-laden query strings stay flaggable). Messages name the key path only — never the value (C-036) | Warning |
| `deprecated` (L005) | Use of a schema-deprecated field | A value exists for a field whose schema declares `deprecated` (message rendered from the schema; authored material is redacted; lint is hermetic and does not read host values) | Warning |
| `schema-evolution` (L006) | Partial evolution metadata | Schema carries `min_reader`, `min_writer`, or a field `removed_in` but no `version`; a fully unversioned schema is allowed | Warning |
| `unknown-capability` (L007) | Unadvertised capability | `active_capabilities` names a capability the shipped descriptor does not advertise | Warning |
| `key-style` (L008) | Mixed key style in one table | One table mixes cased (camel/Pascal) keys with snake_case keys; per table, not per document | Warning |
| `large-literal` (L009) | Oversized string literal | A string literal exceeds 65,536 bytes (content addressing candidate) | Warning |
| `data-mode` (L010) | Data construct without the marker | Content-addressed values, evolution metadata, or capability declarations while the document is in Config Mode | Warning |

## 2. `depends_on` syntax and semantics

```active
[server]
depends_on = ["database", "cache"]
port = 8080
```

- `depends_on` is a **contextual keyword**: a key with an array of
  string literals parses as dependency metadata; any other value
  stays ordinary data (mirroring `schema`). It is allowed as a
  direct member of any table scope including the root — never inside
  profiles, schemas, array elements, or inline tables (inline
  occurrences are E022 with a pointer to the right place).
  A malformed entry (an array containing a non-string) is E022.
- Listed paths resolve dotted from the document root at graph-build
  time; each resolvable path adds a dependency edge into the scope's
  table node (so declared cycles fail as S001 like all cycles).
  Evaluation is otherwise already total — edges document order for
  agents and deployment tooling; they change no values.
- L002 fires per scope when `depends_on` is present and: (a) a
  reference from inside the scope (sibling key, `let`, or nested
  path, resolved through `Ref`s) names something not in the declared
  set — "missing depends_on entry"; (b) a declared path resolves to
  nothing — "dangling"; (c) a declared path falls inside the scope
  itself — "unnecessary" (self-edges prove nothing); (d) the list is
  empty while references exist (same message as (a)). References
  that cannot be mapped to a name or path are skipped silently
  (conservative: no false L002).
- `depends_on` blocks re-emit in the formatter (canonical position:
  after `schema`, before keys) so `fmt` never strips declarations.

## 3. `deprecated` schema argument

- Every schema leaf type accepts a `deprecated` argument (alongside
  its existing args): `deprecated = true` (generic) or
  `deprecated = "use new_port instead"` (migration message, string
  literal only — anything else, including `false`, is E020 like any
  bad argument).
- Stored as `SchemaField.deprecated: Option<String>` (`None` = live,
  `Some("")` = bare true, `Some(msg)` = custom). Table-level
  deprecation is future work; leaf coverage satisfies the checklist.
- L005 fires per present value under a deprecated field, quoting the
  migration message when present. Absent deprecated keys stay
  silent. Closed-table and default machinery are unaffected.

## 4. Disablement

- CLI: `ACTIVE lint [--format text|json|sarif] [--disable-lint <rule>]... <file>...`
  (repeatable; unknown rule names are an error listing the ten).
- Library: `LintOptions { disabled: HashSet<String> }` (rule IDs as
  above); `active_core::lint::lint(&module, &options)`. Reports:
  `lint::json_report` (stable array schema) and `lint::sarif_report`
  (SARIF 2.1.0).
- No file-level suppression comments in this phase (noted future).

## 5. `ACTIVE lint` contract

```
ACTIVE lint [--disable-lint <rule>]... <file>...
```

- Each file lints independently (default parse options, includes
  resolved — analysis sees the composed module); read-only, so all
  files always run (no abort-on-error like `fmt --write`).
- Output: one line per finding (`file:line:col: [Warning] L00x:
  message`, reusing the CLI reporter). Exit `1` when any enabled
  rule fires in any file (including parse errors, which abort that
  file with its diagnostics), else `0`.
- Linter findings never enter `evaluate()` diagnostics: strict and
  resilient modes are unaffected. Future default fills or healer
  wiring never depends on lint output.

## 6. LSP readiness (C-043 consumes this)

`lint()` returns `Vec<Diagnostic>` with stable codes, spans, and
severities — exactly what incremental diagnostics publish. No
linter changes are expected for LSP work.
