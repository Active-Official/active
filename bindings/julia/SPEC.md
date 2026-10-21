# Julia Binding SPEC (CHECKLIST Phase 2 stretch, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (Julia 1.13.2, ccall against the Linux shared library); run-smoke.py reproduces the proof, skipping without julia.

Mechanism: `ccall` over the frozen C ABI. Opaque handles stay `Ptr{Cvoid}` and are never dereferenced; every owned return is freed exactly once (`unsafe_string` copies borrowed and owned strings, then the owned pointer is freed); borrowed diagnostic codes are copied and never freed. ABI gate at startup raising on mismatch. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one: check returns a code vector (empty means clean), eval returns redacted JSON or null on refusal, units mismatches surface T001, secrets never print.

Build/run: `julia bindings/julia/smoke.jl path/to/libactive_ffi.so` (the shared library, since `ccall` dlopens). The shared harness drives it through `bindings/julia/run-smoke.py`, which skips openly without julia or a native shared library.
