# V Binding — C-interop smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`smoke.v` drives `libactive_ffi` through V's C interop (`#include`, `C.` functions): opaque handles as `voidptr`, ABI major gate at startup, Allocator Model A (every owned return freed exactly once; borrowed code strings copied with `cstring_to_vstring` and never freed). Run from the repo root:

```bash
v -cflags -Iactive-ffi -ldflags path/to/libactive_ffi.a -ldflags -lm -ldflags -ldl -ldflags -lpthread -o smoke bindings/vlang/smoke.v && ./smoke
```

Proven in a Linux container (V 0.5.2 built from source, Linux staticlib):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` wraps the call for the shared harness and
skips cleanly without v or a native static library.
