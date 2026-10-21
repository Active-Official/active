# Dart Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (Dart 3.13.6, package:ffi against the Linux shared library); run-smoke.py reproduces the proof (runs dart pub get first), skipping without dart.

Mechanism: dart:ffi.

package:ffi declarations (pinned in pubspec.yaml); handles as Pointer<Void> freed in try/finally; ABI gate at startup.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Harness path `dart` runs the smoke through run-smoke.py; SKIPs openly without the toolchain.
