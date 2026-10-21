# Dart Binding — package:ffi smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`smoke.dart` drives `libactive_ffi` through `package:ffi` (pinned in `pubspec.yaml`; no other dependencies): opaque handles as `Pointer<Void>`, ABI major gate at startup, Allocator Model A (every owned return freed exactly once; borrowed code strings copied with `toDartString` and never freed). Run from `bindings/dart`:

```bash
dart pub get && dart smoke.dart path/to/libactive_ffi.so
```

Proven in a Linux container (Dart 3.13.6, Linux shared library):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` runs `dart pub get` in a temp copy first
(never in the repo) and wraps the call for the shared harness, skipping
cleanly without dart or a native shared library.
