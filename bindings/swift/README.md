# Swift Binding — C-shim smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`smoke.swift` plus a small C shim (`active_swift.c`/`active_swift.h`, since Swift cannot name the header's forward-declared handle types) drives `libactive_ffi` through a generated modulemap: opaque handles as `UnsafeMutableRawPointer`, ABI major gate at startup, Allocator Model A (owned JSON copied to a Swift string then freed exactly once; borrowed codes copied with `String(cString:)` and never freed). Build and run (run-smoke.py generates the modulemap with the real include path):

```bash
swiftc -I <modulemap dir> bindings/swift/smoke.swift bindings/swift/active_swift.c path/to/libactive_ffi.a -o smoke && ./smoke
```

Proven in a Linux container (Swift 6.4.1 via swiftly, Linux staticlib):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` compiles the shim, generates the modulemap
in a temp dir, and wraps the call for the shared harness, skipping
cleanly without swiftc or a native static library.
