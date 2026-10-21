# Perl Binding — FFI::Platypus smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`smoke.pl` drives `libactive_ffi` through FFI::Platypus: opaque handles as `opaque`, ABI major gate at startup, Allocator Model A (owned JSON cast to `string` for the copy, then freed exactly once; borrowed codes are cast-copies and never freed). Run from the repo root:

```bash
perl bindings/perl/smoke.pl path/to/libactive_ffi.so
```

Proven in a Linux container (Perl 5.38, FFI::Platypus 2.08, Linux shared library):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` wraps the call for the shared harness and
skips cleanly without perl, FFI::Platypus, or a native shared library.
