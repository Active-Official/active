# Lua Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: Planned (no toolchain on this machine; design recorded, implementation deferred until a toolchain is available for proof).

Mechanism: LuaJIT FFI.

ffi.cdef over the header text; handles as cdata with ffi.gc finalizers calling the matching free; ABI gate at require. No Lua toolchain on this machine: SPEC only.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Full manifest wiring follows the Go smoke pattern once implemented.
