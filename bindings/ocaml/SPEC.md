# Ocaml Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (OCaml 4.14.1, external declarations plus a small C stub for owned-string copies, against the Linux staticlib); run-smoke.py reproduces the proof, skipping without ocamlopt.

Mechanism: OCaml C stubs.

external declarations with handles crossing the C stubs as nativeint (the GC never chases raw C pointers); a small C stub copies owned strings then frees; explicit frees on every path; ABI gate at startup.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Harness path \ocaml\ runs the smoke through run-smoke.py; SKIPs openly without the toolchain.
