# Julia Binding — `ccall` smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`smoke.jl` drives `libactive_ffi` through `ccall`: opaque handles as `Ptr{Cvoid}`, ABI major gate at startup, Allocator Model A (every owned return freed exactly once; strings copied with `unsafe_string`). Run from the repo root:

```bash
julia bindings/julia/smoke.jl path/to/libactive_ffi.so
```

Proven in a Linux container (Julia 1.13.2 via juliaup, Linux shared library):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` wraps the call for the shared harness and
skips cleanly without julia or a native shared library.
