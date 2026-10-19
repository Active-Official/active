# How to write an Active binding in one day (B-004)

You need: the released `active_wasm.wasm` (or `libactive` for native),
`conformance/manifest.toml`, and this page. No Rust, no core sources.

## Hour 1: load and evaluate

WASM path (default): instantiate the module (zero imports), call
`active_alloc`, write UTF-8 source, call `active_eval_json`, read the
JSON envelope `{"ok","json","diagnostics"}`, free both sides. The
reference implementation is 60 lines: see `active-wasm/js/active-shim.js`
and the Go smoke at `active-wasm/smoke-go/main.go`.

Native path: link `libactive`, include `active-ffi/active.h`, same calls
through C (`active_module_parse`, `active_check`, `active_eval`,
`active_value_to_json`, matching frees). Register include/env callbacks
only if your host owns files or environment; otherwise skip them and let
missing inputs fail with diagnostics.

## Hour 2: run conformance

Drive every `[[cases]]` entry in `conformance/manifest.toml`: feed
`cases/<id>.active`, compare the value against `expected/<id>.json` and
the code set against `errors/<id>.json`. Codes compare strictly; spans
stay advisory. Record `[[pins]]` entries as pins with reason, never
failures. Green means no drift, forever.

## Hour 3: harden your surface

Null every handle path, fail closed on ABI major mismatch (check
`active_abi_version()` at load), never log source or JSON without
checking `is_secret` semantics (redaction happens inside, verify with a
secret fixture), enforce your own timeout (Worker terminate on WASM;
budgets bound everything else).

## Rules (from 04, non-negotiable)

Ship idiomatic wrappers, callback adapters, error mapping, a conformance
runner, and packaging metadata. Never copy the parser, IR, or evaluator.
Never reimplement secret or budget logic. Never invent a second oracle.
Pin released artifacts by version plus checksum.

## Python via wasmtime (B-003, WASM-first)

No PyO3 surface until measured demand says otherwise. Python hosts drive
the same zero-import module through wasmtime (`pip install wasmtime`):

```python
from wasmtime import Store, Module, Linker
store = Store()
module = Module.from_file(store.engine, "active_wasm.wasm")
instance = Linker(store.engine).instantiate(store, module)
ex = instance.exports(store)
mem = ex["memory"]
assert ex["active_abi_version"](store) == 1

def evaluate(source: str) -> dict:
    data = source.encode()
    sp = ex["active_alloc"](store, len(data))
    mem.write(store, data, sp)
    slot = ex["active_alloc"](store, 4)
    out = ex["active_eval_json"](store, sp, len(data), slot)
    n = int.from_bytes(mem.read(store, slot, slot + 4), "little")
    env = __import__("json").loads(bytes(mem.read(store, out, out + n)).decode())
    ex["active_free"](store, sp, len(data))
    ex["active_free"](store, out, n)
    ex["active_free"](store, slot, 4)
    return env
```

Memory slices are `[start:stop]` in this API generation; results are bare
values, not lists. Proven against the shipped module (clean value, T001
refusal, secret redaction) before this recipe was written.
