# Vlang Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (V 0.5.2, C interop against the Linux staticlib); run-smoke.py reproduces the proof, skipping without v.

Mechanism: V C interop.

C interop over active.h with defer-based frees at every call site; ABI gate at startup.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Harness path \vlang\ runs the smoke through run-smoke.py; SKIPs openly without the toolchain.
