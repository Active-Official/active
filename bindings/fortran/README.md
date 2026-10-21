# Fortran Binding — ISO_C_BINDING smoke (CHECKLIST 4.001 stretch).

`smoke.f90` is pure standard Fortran with `ISO_C_BINDING` interfaces
over the frozen C ABI: opaque handles as `TYPE(C_PTR)`, ABI major gate
at load, Allocator Model A (every owned pointer freed on every path,
including assertion exits). No C shim, no preprocessor, no compiler
extensions beyond standard-conforming `ERROR STOP`.

Build and run (Linux, gfortran):

```bash
gfortran bindings/fortran/smoke.f90 -I active-ffi \
  target/x86_64-unknown-linux-gnu/release/libactive_ffi.a \
  -o /tmp/active-fortran-smoke && /tmp/active-fortran-smoke
```

Expected output ends with `SMOKE-OK`: ABI gate, clean check, eval
value, T001 refusal, secret redaction with zero raw leakage.
`run-smoke.py` wraps the build plus run for the shared harness and
skips cleanly without gfortran or a native static library.
