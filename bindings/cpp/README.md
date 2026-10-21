# C++ Binding — RAII over the frozen C ABI (CHECKLIST 2.003, PROPOSAL section 2.1)

Status: smoke-proven on Linux (Ubuntu 24.04 container, g++ 13) against a
Linux-built staticlib. `smoke.cpp` drives the full agent-relevant surface
(parse, check with codes, eval with JSON, secret redaction, refusal paths).

## Build and run (Linux)

```bash
cargo build -p active-ffi --target x86_64-unknown-linux-gnu --release  # needs zig cc as linker, see below
g++ -std=c++17 bindings/cpp/smoke.cpp -I active-ffi \
  target/x86_64-unknown-linux-gnu/release/libactive_ffi.a \
  -o /tmp/active-cpp-smoke && /tmp/active-cpp-smoke
```

The cross build uses `zig cc` as the linker via a wrapper script
(`CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER`), since no Linux
toolchain is installed on the host. Expected output ends with
`SMOKE-OK`: ABI gate, clean check, eval value, T001 refusal, redaction.

## Ownership mapping

`unique_ptr` with custom deleters, exactly one free per owned pointer
(`active_module_free`, `active_diagnostics_free`, `active_value_free`,
`active_string_free`); diagnostic codes are borrowed views, never freed.
`active.h` carries `extern "C"` guards so C++ includes it directly.
