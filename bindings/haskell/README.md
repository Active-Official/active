# Haskell Binding — `foreign import ccall` smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`smoke.hs` drives `libactive_ffi` through GHC's FFI: opaque handles as abstract `Ptr ()`, ABI major gate at startup, Allocator Model A (every owned pointer freed exactly once; borrowed code strings copied with `peekCString` and never freed). Run from the repo root:

```bash
ghc -o smoke bindings/haskell/smoke.hs path/to/libactive_ffi.a && ./smoke
```

Proven in a Linux container (GHC 9.4.7, Linux staticlib):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` wraps the call for the shared harness and
skips cleanly without GHC or a native static library.
