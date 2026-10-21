# Kotlin Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (kotlinc 1.3 on JDK 21, JNI shim mirroring bindings/java against the Linux staticlib); run-smoke.py reproduces the proof, skipping without kotlinc/java/gcc.

Mechanism: Kotlin JVM (JNA/JNI).

JNI shim (active_kni.c) mirroring bindings/java; handles as Long freed via Closeable.use; ABI gate at startup.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Harness path `kotlin` runs the smoke through run-smoke.py; SKIPs openly without the toolchain.
