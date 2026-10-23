# Active JS/TS Binding — Ownership & Contract Map

Tier-1 binding (matrix row: Complete). The full contract lives in
[`README.md`](README.md) (agent-loop surface, WASM-first decision,
npm identity, load-once guidance, ownership and close discipline).
This file maps the matrix's ownership-rule checklist onto it:

1. **Opaque handles** — `load()` returns an opaque module object;
   internals (`active-wasm/js/active-shim.js`) never surface.
2. **Allocator Model A** — the WASM module allocates; hosts free
   only through the module's `close()` path (`active_free`
   under the hood). No host GC touches module memory.
3. **ABI gate at load** — `load()` checks the ABI major and throws
   on mismatch instead of running.
4. **Error mapping** — refusals throw (`refused:` with the
   diagnostic codes); `validate`/`check` agree on every code, span,
   and message (tested in `test_active.js`).
5. **Harness parity** — `conformance/harness/run.py` path
   `js-native` green; differential parity green.

Status pointer for the matrix: evidence `test_active.js` +
`conformance/harness/run.py` differential green (flipped
2026-10-21).
