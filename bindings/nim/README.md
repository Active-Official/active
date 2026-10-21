# Nim Binding — `importc` smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`smoke.nim` drives `libactive_ffi` through Nim's C interop: opaque handles as `pointer`, ABI major gate at startup, Allocator Model A (every owned return freed exactly once; borrowed code strings copied with `$` and never freed). Run from the repo root:

```bash
nim c -d:release --passC:-Iactive-ffi --passL:path/to/libactive_ffi.a --passL:-lm --passL:-ldl --passL:-lpthread -o:smoke bindings/nim/smoke.nim && ./smoke
```

Proven in a Linux container (Nim 2.2.12 via choosenim, Linux staticlib):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` wraps the call for the shared harness and
skips cleanly without nim or a native static library.
