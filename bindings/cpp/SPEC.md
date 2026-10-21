# Cpp Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: Planned (no toolchain on this machine; design recorded, implementation deferred until a toolchain is available for proof).

Mechanism: C++ RAII.

unique_handle wrappers with custom deleters calling the matching active_*_free; ABI gate in constructor throwing on mismatch; diagnostics copied to std::string before the set is freed. Header-only wrapper first, no build system churn. No C++ CI image on this machine: SPEC only.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Full manifest wiring follows the Go smoke pattern once implemented.
