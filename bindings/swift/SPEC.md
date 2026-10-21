# Swift Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (Swift 6.4.1 via swiftly, C shim plus generated modulemap against the Linux staticlib); run-smoke.py reproduces the proof, skipping without swiftc.

Mechanism: Swift C interop.

C shim (active_swift.c/h) plus a generated modulemap (Swift cannot name forward-declared C types); handles as UnsafeMutableRawPointer freed via defer; ABI gate at startup.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Harness path `swift` runs the smoke through run-smoke.py; SKIPs openly without the toolchain.
