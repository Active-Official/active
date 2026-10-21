# Lua Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (LuaJIT, Linux shared library); run-smoke.py reproduces the proof, skipping without luajit.

Mechanism: LuaJIT FFI.

ffi.cdef over the header text; handles as cdata with ffi.gc finalizers calling the matching free; ABI gate at require.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Full manifest wiring follows the Go smoke pattern once implemented.
