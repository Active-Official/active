# Kotlin Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: Planned (no toolchain on this machine; design recorded, implementation deferred until a toolchain is available for proof).

Mechanism: Kotlin JVM (JNA/JNI).

JNA interface over the cdylib for the first cut (no JNI boilerplate); handles as Pointer with Cleaner actions calling the matching free; ABI gate in object init. No Kotlin toolchain on this machine: SPEC only.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Full manifest wiring follows the Go smoke pattern once implemented.
