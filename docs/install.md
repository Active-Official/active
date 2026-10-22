# Install Active (CHECKLIST D.008, PROPOSAL §4.2)

One page that routes every user to the right package. Pick your
language; every path below ends with the same check: evaluate the
demo and get identical JSON.

## Rust

```sh
cargo install active-cli        # the `active` toolchain
cargo add active                # library use (facade over active_core)
```

Non-monorepo users add the published crate — no checkout needed
(see `active/README.md`; the doctest there is the proof).

## Python

```sh
pip install active-lang         # WASM-first binding, needs `wasmtime` (installed automatically)
```

The evaluator WASM is resolved at runtime: pass `wasm_path=...`,
set `ACTIVE_WASM_PATH`, or place `active_wasm.wasm` next to the
package. Maintainers build the distributable bundle with:

```sh
python packaging/fetch-wasm.py --version 1.4.0 --into bindings/python bindings/js
```

Verify:

```python
from active import Module
with Module() as m:
    print(m.eval("port = 8080"))
```

## JavaScript / TypeScript

```sh
npm install active-lang   # zero dependencies, Node 20+ / Deno / Bun / browsers
```

Same WASM resolution as Python (`ACTIVE_WASM_PATH`, bundled copy,
or explicit path). Load once per process and reuse the module (see
`bindings/js/README.md`).

```js
const { load } = require("active-lang");
const m = await load();
m.evaluate("port = 8080\n");
m.close();
```

## Go

```sh
go get github.com/Active-Official/active-go@latest   # wazero-based, zero cgo, go 1.25+
```

## Editors

- VS Code / Cursor / VSCodium: install the `active-lang` vsix from
  the GitHub release (bundles `active-lsp`).
- Neovim: use the `editors/neovim/` runtimepath plugin (see its
  README) with an `lspconfig` snippet pointing at `active-lsp`.

## Release archives (no toolchain)

Each GitHub release ships `active-*.tar.gz`/`.zip` per platform
with the CLI, formatter, LSP, README, licenses, and schemas (the
standard library, vendored for offline use), plus a separate WASM
bundle (`active_wasm.wasm`), `SHA256SUMS`, and build-provenance
attestations.
Verify with `gh attestation verify` (see `spec/packaging.md` §3).

## Version alignment

Every package tracks the workspace release line with the frozen
C ABI v1 underneath; the full matrix lives in
`docs/version-and-abi-policy.md`. If versions ever disagree, the
ABI gate fails closed at load — update, don't override.
