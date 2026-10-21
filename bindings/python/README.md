# Active Python Binding — WASM-first (CHECKLIST 1.004-1.006, PROPOSAL §2.1, §2.3)

Zero native compilation. The binding drives the zero-import
`active_wasm.wasm` module through `wasmtime` (pip-installable, no compiler
needed) and exposes the agent-loop surface with opaque handles, an ABI
major gate, and Allocator Model A (the module's `active_free` frees
everything the module allocated).

## Decision (DERIVED, owner authority)

WASM-first instead of cffi/PyO3: no per-platform wheels, no compiler on the
user machine, identical semantics to every other embedding by construction.
A native extension remains an option if profiling ever justifies it; nothing
in this layout precludes adding one later behind the same API.

## API

```python
from active import Module

with Module() as m:                      # or Module(path_to_wasm)
    diags = m.check(source)              # [Diagnostic(code, message)]
    value = m.eval(source)               # redacted dict, raises on refusal
```

`Module` instances are confined to the creating thread. `check` returns
`[]` when clean. `eval` raises `ActiveError` on refusal with the
diagnostics text. Secrets always render `***SECRET***`.

## Tests

`test_active.py` drives the shipped module: ABI gate, eval value, units
refusal with stable code T001, secret redaction with no raw leak, and
double-eval determinism. Run from the repo root:

```bash
python bindings/python/test_active.py
```

Requires `wasmtime` (`pip install wasmtime`) and a built module
(`cargo build -p active-wasm --target wasm32-unknown-unknown --release`).
