# Java Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (JDK 21, JNI shim, Linux staticlib); run-smoke.py reproduces the proof, skipping without a JDK.

Mechanism: JNI.

JNIEnv native methods over the frozen C ABI; handles as long-lived long fields holding native pointers, freed in close() plus Cleaner fallback; ABI gate in static initializer; errors map to typed ActiveException.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Full manifest wiring follows the Go smoke pattern once implemented.
