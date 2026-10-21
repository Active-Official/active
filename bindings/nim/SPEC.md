# Nim Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (Nim 2.2.12, importc against the Linux staticlib); run-smoke.py reproduces the proof, skipping without nim.

Mechanism: Nim importc.

importc over the header with try/finally frees; handles as pointer; ABI gate at startup raising on mismatch.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Harness path \nim\ runs the smoke through run-smoke.py; SKIPs openly without the toolchain.
