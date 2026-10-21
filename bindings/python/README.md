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
refusal with stable code T001, secret redaction with no raw leak (including
refusal paths), closed-module fail-closed behavior on every entry point,
double-eval determinism, structured spans on diagnostics, `validate`/`check`
agreement, and `.pyi` stub fidelity to the runtime API (9 tests green).
Run from the repo root:

```bash
python bindings/python/test_active.py
```

Requires `wasmtime` (`pip install wasmtime`) and a built module
(`cargo build -p active-wasm --target wasm32-unknown-unknown --release`).

## Ownership and lifetime (normative for this binding)

- **Opaque handles:** a `Module` owns exactly one wasmtime instance. One `Module` per thread; never share a module across threads, with or without locking. The underlying engine state (including the module's live-buffer registry) is thread-confined by construction, so sharing would corrupt frees even under a lock.
- **Allocator Model A:** every allocation the module makes is freed inside the module through the official `active_free` export. The host never frees module memory with any other function, and module memory is never exposed as writable host buffers beyond the single input copy per call.
- **ABI gate:** `Module()` construction reads `active_abi_version` and raises `ActiveError` unless the major version is 1. A stale or foreign `.wasm` cannot be used silently.
- **Error mapping:** evaluation refusal surfaces as `ActiveError` (never a bare `None` or an empty dict); a missing module or envelope-shape mismatch also raises `ActiveError`, so misuse fails loudly instead of returning plausible-looking data.
- **Close discipline:** use the module as a context manager or call `close()` explicitly. Using a closed module raises `ActiveError`.
- **Budgets:** the module runs the hermetic engine with shipped hard budgets (fail-closed on exhaustion). `validate()` is the explicit validation step of the agent loop and shares `check()`'s verdict path; the reference loops additionally enforce an iteration budget (`max_iters`) with stuck-loop detection. Hosts that need custom per-call budgets drive the C ABI surface (`active_check` with `ActiveBudget`), which is the same core with a budget pointer.
