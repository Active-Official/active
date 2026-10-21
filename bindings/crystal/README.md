# Crystal Binding — `lib` smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`smoke.cr` drives `libactive_ffi` through Crystal `lib`/`fun` declarations: opaque handles as `Void*`, ABI major gate at startup, Allocator Model A (every owned return freed exactly once; borrowed code strings copied with `String.new` and never freed). Run from the repo root:

```bash
crystal build --release --link-flags "path/to/libactive_ffi.a -lm -lpthread -ldl" -o smoke bindings/crystal/smoke.cr && ./smoke
```

Proven in a Linux container (Crystal 1.22, Linux staticlib):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` wraps the call for the shared harness and
skips cleanly without crystal or a native static library.
