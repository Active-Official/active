# Perl Binding SPEC (CHECKLIST Phase 2 stretch, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (Perl 5.38 with FFI::Platypus 2.08 against the Linux shared library); run-smoke.py reproduces the proof, skipping without perl or FFI::Platypus.

Mechanism: FFI::Platypus over the frozen C ABI. Opaque handles stay `opaque` and are never dereferenced; owned JSON crosses as `opaque`, is copied with a cast to `string`, then freed exactly once; borrowed diagnostic codes are cast-copies and never freed. ABI gate at startup dying on mismatch. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one: check returns a code count plus codes, eval returns redacted JSON or undef on refusal, units mismatches surface T001, secrets never print.

Build/run: `perl bindings/perl/smoke.pl path/to/libactive_ffi.so` (the shared library, since Platypus dlopens). The shared harness drives it through `bindings/perl/run-smoke.py`, which skips openly without perl, FFI::Platypus, or a native shared library.
