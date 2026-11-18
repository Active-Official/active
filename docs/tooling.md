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

Analysis is hermetic (no ambient environment) and budgeted on both
halves; a trip publishes a coded diagnostic instead of hanging: E029 when
a parse bound trips (4 MiB input, 1,000,000 arena nodes, a parse wall
budget that scales with input), X0xx when evaluation exceeds the strict
budget.

## Inspector

```sh
active inspect --from svc.active                    # schema contract (INSPECT v2)
active inspect --from svc.active --path server.port # one value
active inspect --from svc.active --path token --format text
active inspect --from svc.active --path token --profile production
```

Path reports carry kind, redacted value, taint, size, content-addressed
identity (`blake3:…`, size, media type), schema constraints, and
dependency provenance. Taint is structural and value-aware: a plain
value used as a `secret(...)` locator (a `let`, table field, or `env`
lookup), any interpolation or container built from it, a copy of a
runtime secret, and a blob payload embedding one all report
`"***SECRET***"` with `tainted: true`, even though no `Value::Secret`
marker exists at runtime. The CLI evaluates through a taint probe that
retains payloads without charging per-alias copies, so a
payload-resident secret is noticed and the report still carries no
payload bytes. Inspection always uses the shipped default resource
limits (it has no `--max-*` flags); a document that needs a larger
budget must be evaluated by the embedding caller.
Containers keep their shape; only material leaves are replaced, and
known secret substrings are redacted anywhere they appear. Schema:
`docs/schemas/inspect-value.json`.

## Debugger

```sh
active debug --from svc.active                       # step trace (JSON)
active debug --from svc.active --break taint --format text
active debug --from svc.active --replay --expect sha256:…
active debug --from svc.active --minimize T003
```

Steps are secret-safe by construction (locator subtrees redacted).
Breakpoints: `taint`, `path:<prefix>`, `code:<CODE>`, `node:<n>`,
`budget:<percent>` (cumulative nodes against `max_nodes`), and
`budget:nodes:<n>` (the n-th evaluated node). Replay compares hashes,
codes, and traces. Reduction deletes lines while the target code
survives and exports the reproduction inputs: the reduced source, the
environment variable names it reads, and the effective budget.

The debugger also has a language-server bridge (spec/lsp.md):
`active/debug/steps` takes `{textDocument: {uri}, breakpoint?}` and
returns the same redacted step array, so an editor can show the trace
without spawning the CLI. Spec: `spec/debug.md`.

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

## Semantic diff

```sh
active diff old.active new.active
active diff old.active new.active --format json
```

Compares the evaluated documents, not their text: every added, removed,
or changed value path with both types and a redacted scalar echo, the
schema compatibility findings (C040-C044), and the dependents of each
changed path (resolved to the longest dotted prefix, so a nested change
names its own consumers). `behaviour: identical` means every *compared*
value is equal; material values are never compared, so a secret or blob
path always reports changed with a `***SECRET***` echo and a
guess can never be confirmed. A value that only one document classifies
as secret (for example `env.X` becoming `secret(env.X)`) is redacted on
both sides. Value echoes and paths come from the
redacted trees, so a material key renders as the marker. `--format json`
emits one machine-readable document under `diff` (schema:
`docs/schemas/diff.json`). Both documents parse and evaluate strictly:
an error or warning refuses with its stable codes and a nonzero exit
(the `check`/`eval` convention). Schema findings do not gate. Use
`active compatibility` for a fail-closed schema gate.

## Toolchain self-check

```sh
active doctor                      # text report for the current directory
active doctor --path ../active-lang --format json
```

One command checks the things that silently differ between machines and
checkouts (LANG-H-001/H-002 class): the binary evaluates a known document
to the known value, a schema violation still reports an error-severity
diagnostic, a one-node budget still refuses a three-node document, and —
when `--path` names a source checkout — the CLI and workspace versions
agree, no network-capable crate is in `active-core`'s dependency closure
(DEC-002), `active-ffi/active.h` is present, and a built WASM artefact
exists. Each check reports `pass`, `warn`, `fail`, or `skip`; the JSON
form is `{"ok": bool, "checks": [{"id", "status", "detail"}]}` and any
`fail` exits non-zero, so CI can gate on it.

## Editors

VS Code/Cursor/VSCodium: install the vsix; the status bar shows mode,
node-budget percentage, and taint, and six commands wrap the CLIs above.
Neovim: add `editors/neovim` to runtimepath, call
`require('active').setup()`, and add `M.status()` to the status line.
Details and stretch-target gaps: `editors/README.md`.

## Linter

```sh
active lint --format text|json|sarif --disable-lint <rule> file.active
active lint --config .active-lint.toml file.active
```

Sixteen rules (catalog: `docs/lint-rules.md`); JSON and SARIF shapes are
pinned by `docs/schemas/lint.json` and SARIF 2.1.0. Project policy lives
in `.active-lint.toml` (discovered upward from each input file, or
pinned with `--config`): a `disable` array plus a `[severity]` table
(`off|error|warning|info|hint`). Unknown rules, unknown keys, and a
malformed discovered file are errors rather than silent no-ops.

## Include store

```sh
active eval  --store .cache/active doc.active
active lock  --store .cache/active doc.active -o active-lock.toml
active store add lib.active --store .cache/active
active store list --json --store .cache/active
active store gc --root active-lock.toml --store .cache/active
```

The store holds one immutable, read-only file per include SHA-256
(`<root>/<hex>`), deduplicating repeated includes of identical bytes.
Prefer a project-scoped root: `gc` roots come from one project's
lockfiles, so running it against the shared `~/.cache` store deletes
entries other projects still need.
`gc` keeps everything a given lockfile records and deletes the rest;
`--dry-run` reports without deleting. The store is a cache: evaluation
never reads it as an input, and a write failure is the informational
W018 while the tree and exit status stay unchanged. It contains raw include bytes (including secrets), so
point it at a directory with the source's confidentiality.

## Evaluation cache

```sh
active eval --cache .cache/active-eval doc.active
active eval --cache .cache/active-eval --format yaml doc.active
```

A repeat run whose inputs are unchanged prints the stored result
without evaluating. The key is a keyed BLAKE3 MAC over the evaluator
version, the source and include content hashes, mode, profile, resolved
budget, the values of every environment variable the document names
(`env.NAME` or `secret.env(NAME)`), the input path, and the output
selectors (`--path`, `--materialise-blobs`, `--reader-version`,
`--resilient`, sandbox). One entry serves all three formats.
Diagnostics replay with the result, so stdout, stderr, and exit status
match the run that filled the entry.

The MAC key is a per-user file outside the cache directory:
`$ACTIVE_CACHE_KEY_FILE` when set, else `active/cache-key` under the
platform config directory (`%APPDATA%` on Windows, `$XDG_CONFIG_HOME` or
`~/.config` on Unix). It is generated on first use, created owner-only,
and refused if it is not a regular file owned by the running user and
readable only by them (Windows ACLs are not checked). Another user who
can list or write a private cache directory cannot compute an entry name
or confirm a guessed environment value; entries are not portable across
users or machines. Without a usable key file the run proceeds uncached.

Entries are JSON with a v2 receipt binding the value to its schema,
profile, budget, mode, and capability. A run whose profile or schema
selector is material writes no entry, so the selector never reaches
disk in cleartext. Entries hold the printed output,
including materialised blob payloads when `--materialise-blobs` was
used, so point `--cache` at a directory with the source's
confidentiality. On Unix the cache root must belong to the same user as
the key file, must not be a symlink or group/other-writable, and an
entry readable by or owned by another user is not served; Windows ACLs
are not checked. The cache is best-effort: a failed write is a warning,
and a corrupt, stale, or non-regular entry is a miss with a warning.
`--audit-trail`, `--envelope`, and `--trace` record or observe the run
itself, and `--store`, `--trust-keys`, `--lockfile`, and
`--envelope-trust` add side effects, signature policy, or parse-time
diagnostics outside the key, so those flags bypass the cache (with a
warning). The cache never supplies inputs: includes are re-read and
re-verified every run. Delete the directory to clear entries, including
ones left by a rebuild at the same version.

## One-pass test command

```sh
active test tests/fmt/indent --strict
active test --conformance conformance/manifest.toml
```

`active test` runs schema validation (a hermetic evaluation), lint,
fmt-check, and the conformance manifest in one pass with one exit code.
Directories are walked for `.active` files (hidden, `target`,
`node_modules`, and the `conformance` subtree owned by a discovered
manifest are skipped); a directory containing
`conformance/manifest.toml` runs it automatically unless `--conformance`
names one. The formatter check matches `active fmt --check` (indent
style, no include merge) and refuses files that need repair, pointing at
`active repair`. Lint uses the file's discovered `.active-lint.toml`;
findings are notes by default and failures under `--strict`. Schema
errors and non-canonical formatting always fail. Evaluation is hermetic
by default: pass `--env KEY=VALUE` (repeatable) for documents that read
the environment. An empty walk, an empty manifest, or a malformed one
fails rather than passing vacuously. No external test framework or new
tool is required.

## Playground loop

`playground/loop/` runs the full agent loop against the shipped CLI:
generate a JSON Schema contract, check the failing document, plan the
repair, apply one step (and see the truthful partial verdict), then
apply the plan and evaluate the clean result. `run.ps1` and `run.sh`
execute it; `transcript.txt` is a captured run of the shipped binary;
`cargo test -p active-cli --test loop_demo` asserts the same pipeline so
the transcript stays honest.

