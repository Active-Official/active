# R Binding — `.Call` wrapper smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`r_wrap.c` exposes the frozen C ABI through `.Call` entry points (`r_abi`, `r_check_codes`, `r_eval_json`), and `smoke.R` asserts the contract: ABI major gate at load, Allocator Model A inside the wrapper (owned JSON copied to an R string then freed exactly once; borrowed codes copied and never freed; every handle freed on all paths). Build and run from the repo root:

```bash
PKG_LIBS="path/to/libactive_ffi.a -lpthread -ldl -lm" PKG_CPPFLAGS="-Iactive-ffi" R CMD SHLIB bindings/r/r_wrap.c
Rscript bindings/r/smoke.R ./r_wrap.so
```

Proven in a Linux container (R 4.3.3, Linux staticlib):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` builds the wrapper in a temp dir and wraps
the call for the shared harness, skipping cleanly without Rscript or a
native static library.
