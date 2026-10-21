# Php Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: Planned (no toolchain on this machine; design recorded, implementation deferred until a toolchain is available for proof).

Mechanism: PHP FFI extension.

PHP 7.4+ FFI cdef over docs/FFI.md header text; owned handles wrapped in objects with __destruct calling the matching free; ABI gate at module load. No PHP toolchain on this machine: SPEC only.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Full manifest wiring follows the Go smoke pattern once implemented.
