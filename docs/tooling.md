# Tooling guide (human + agent)

One page per surface. Every command below is covered by tests; the
machine-output shapes with a committed schema are pinned in
`docs/schemas/` and validated in CI.

## Language server

`active-lsp` speaks LSP 3.17 over stdio. Advertised capabilities:
diagnostics, hover, definition, references, rename, completion (schema
fields, lets, literals, units, mode/capability keys), code actions
(healing, extract-let, style conversion, depends_on, mode marker,
`@ai_immutable`, multi-step repair plan), inlay hints, semantic tokens
(keyword/string/number/comment/property/variable/operator plus custom
`secret`, `mode`, `contentAddressed`, `schema`), full and range
formatting, document and workspace symbols, and the custom
`active/graph`, `active/impact`, `active/why` methods.

Analysis is hermetic (no ambient environment) and evaluates under the
strict budget; a trip publishes an X0xx diagnostic instead of hanging.

## Inspector

```sh
active inspect --from svc.active                    # schema contract (INSPECT v2)
active inspect --from svc.active --path server.port # one value
active inspect --from svc.active --path token --format text
active inspect --from svc.active --path token --profile production
```

Path reports carry kind, redacted value, taint, size, content-addressed
identity (`blake3:…`, size, media type), schema constraints, and
dependency provenance. Taint is structural: a plain value used as a
`secret(...)` locator (a `let`, table field, or `env` lookup) and any
interpolation or container built from it report `"***SECRET***"` with
`tainted: true`, even though no `Value::Secret` marker exists at
runtime. Containers keep their shape; only material leaves are
replaced, and known secret substrings are redacted anywhere they
appear. Schema: `docs/schemas/inspect-value.json`.

## Debugger

```sh
active debug --from svc.active                       # step trace (JSON)
active debug --from svc.active --break taint --format text
active debug --from svc.active --replay --expect sha256:…
active debug --from svc.active --minimize T003
```

Steps are secret-safe by construction (locator subtrees redacted).
Breakpoints: `taint`, `path:<prefix>`, `code:<CODE>`, `node:<n>`.
Replay compares hashes, codes, and traces. Reduction deletes lines while
the target code survives. Spec: `spec/debug.md`.

## Visualizers

```sh
active viz graph  --from svc.active --format mermaid
active viz taint  --from svc.active
active viz taint  --from svc.active --profile production
active viz budget --from svc.active
active viz budget --from svc.active --profile production
active viz schema --from svc.active
active viz repair --from svc.active
```

Load any JSON export into `viz/index.html` for a dependency-free view.
Shapes: `docs/schemas/{graph,taint,budget}.json`.

## Repair browser

```sh
active repair --from svc.active --browser
active repair --from svc.active --apply-step 0 --format text
active repair --from svc.active --cherry-pick server --output fixed.active
```

`--browser` pairs diagnostics with the multi-step plan (no source text,
so authored secrets are never echoed). Partial application applies only
safe steps inside the index bound or path prefix. Spec:
`spec/repair-ui.md`.

## Query CLIs

```sh
active graph svc.active query --query dependents-of --key server.port
active schema inspect --from svc.active --path port
active compatibility old.active new.active --format json
active capability --require-major 1
active mode --from svc.active --format json
active budget explain --from svc.active
```

## Editors

VS Code/Cursor/VSCodium: install the vsix; the status bar shows mode,
node-budget percentage, and taint, and six commands wrap the CLIs above.
Neovim: add `editors/neovim` to runtimepath, call
`require('active').setup()`, and add `M.status()` to the status line.
Details and stretch-target gaps: `editors/README.md`.

## Linter

```sh
active lint --format text|json|sarif --disable-lint <rule> file.active
```

Ten rules (catalog: `docs/lint-rules.md`); JSON and SARIF shapes are
pinned by `docs/schemas/lint.json` and SARIF 2.1.0.
