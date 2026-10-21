# PHP Binding SPEC (CHECKLIST 2.006, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (PHP 8.3 FFI over the Linux
shared library, ext-ffi enabled); run-smoke.py reproduces the proof,
skipping without PHP or a native shared library.

Mechanism: ext-ffi `FFI::cdef` over the frozen C ABI (opaque struct
types only). Input strings cross as explicit `uint8_t[]` buffers via
`FFI::memcpy` (PHP strings do not coerce to C pointers); output strings
are read with explicit lengths (no `FFI::string` before PHP 8.4).
Allocator Model A with try/finally on every path.

Agent-loop surface mirrors bindings/python/active.py one to one.
