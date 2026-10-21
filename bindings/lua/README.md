# Lua Binding — LuaJIT FFI smoke (CHECKLIST 2.016)

`smoke.lua` drives the frozen C ABI through LuaJIT FFI: opaque handles as
cdata, ABI major gate at load, Allocator Model A (every owned pointer
freed exactly once, with `ffi.gc` finalizers as backstop). Run:

```bash
luajit bindings/lua/smoke.lua path/to/libactive_ffi.so
```

Proven in a Linux container against the in-tree Linux shared library:
ABI gate, clean check, eval value, T001 refusal with code check, secret
redaction with zero raw leakage. `run-smoke.py` wraps this for the
shared harness and skips cleanly without LuaJIT or the Linux library.
