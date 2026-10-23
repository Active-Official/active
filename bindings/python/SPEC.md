# Active Python Binding — Ownership & Contract Map

Tier-1 binding (matrix row: Complete). The full contract lives in
[`README.md`](README.md) (agent-loop surface, WASM-first via
wasmtime decision, type stubs, close discipline). This file maps
the matrix's ownership-rule checklist onto it:

1. **Opaque handles** — `Module()` is opaque; the wasmtime instance
   never escapes the object.
2. **Allocator Model A** — the WASM module allocates; the binding
   frees only through the module's `active_free` export (host GC
   never touches module memory).
3. **ABI gate at load** — major mismatch raises `ActiveError`
   instead of running.
4. **Error mapping** — refusals raise `ActiveError` carrying the
   diagnostic codes; `validate`/`check` agree on every code, span,
   and message (tested in `test_active.py`, including the
   `PANIC_CAUGHT` and closed-module fail-closed paths).
5. **Harness parity** — `conformance/harness/run.py` path `python`
   green; differential parity green.

Status pointer for the matrix: evidence `bindings/python/test_active.py`
(9/9 incl. stub-fidelity gate) + `conformance/harness/run.py`
differential green (flipped 2026-10-21).
