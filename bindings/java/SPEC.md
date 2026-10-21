# Java Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: Planned (no toolchain on this machine; design recorded, implementation deferred until a toolchain is available for proof).

Mechanism: JNI.

JNIEnv native methods over the frozen C ABI; handles as long-lived long fields holding native pointers, freed in close(); ABI gate in static initializer; errors map ACTIVE_ERR_* to typed exceptions. No toolchain on this machine: SPEC only, implementation when a JDK is available.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Full manifest wiring follows the Go smoke pattern once implemented.
