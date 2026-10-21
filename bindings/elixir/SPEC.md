# Elixir Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: Planned (no toolchain on this machine; design recorded, implementation deferred until a toolchain is available for proof).

Mechanism: Elixir NIFs (Rustler).

Rustler NIF crate owning handles as ResourceArc with on-drop free; ABI gate at load; errors map to {:error, {code, message}} tuples. No Elixir toolchain on this machine: SPEC only.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Full manifest wiring follows the Go smoke pattern once implemented.
