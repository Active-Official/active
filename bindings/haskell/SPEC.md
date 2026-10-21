# Haskell Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (GHC 9.4.7, foreign import ccall against the Linux staticlib); run-smoke.py reproduces the proof, skipping without ghc.

Mechanism: GHC FFI.

foreign import ccall with explicit frees inside a bracket envelope; handles as abstract Ptr (); ABI gate at startup failing fast.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Harness path \haskell\ runs the smoke through run-smoke.py; SKIPs openly without the toolchain.
