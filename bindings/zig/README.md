# Zig Binding — Smoke over the frozen C ABI (CHECKLIST 2.003)

`smoke.zig` drives `active-ffi` directly through `@cImport("active.h")`:
opaque handles, ABI major gate at startup, Allocator Model A (every
owned pointer freed exactly once, verified by the harness run).

## Build and run

From the repo root (needs the `active-ffi` staticlib/cdylib built and
Zig 0.16+):

```bash
cargo build -p active-ffi
zig build-exe bindings/zig/smoke.zig -I active-ffi -L target/debug -l active_ffi -lc
./smoke            # needs active_ffi.dll beside the exe on Windows
```

Expected output ends with `SMOKE-OK` and covers: ABI gate, clean check,
eval value, units refusal with code `T001` (eval null plus check code),
secret redaction with zero raw leakage.
