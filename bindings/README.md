# Active Language Bindings - Support Matrix and Ownership Rules

**Normative for all binding authors and AI agents.**
**Back-reference:** PROPOSAL section 2, section 2.2, section 2.4, section 5.5; CHECKLIST Steps 0.005, Appendix B.

## Ownership and Lifetime Rules (Mandatory)

1. All handles (`ActiveModule*`, `ActiveValue*`, `ActiveDiagnosticSet*`, `ActiveString`, etc.) are completely opaque. No field access, no size assumptions, no pointer arithmetic.
2. Memory allocated by the Active core that crosses the FFI boundary may be freed **only** by the corresponding `active_*_free` function. Host language garbage collectors, `free()`, `delete`, or equivalent must never be applied to Active-allocated pointers.
3. Check the ABI version via `active_abi_version()` at initialization; a major mismatch is a hard error that refuses to proceed.
4. `ACTIVE_ERR_PANIC_CAUGHT` and `ACTIVE_ERR_NULL_HANDLE` must surface as proper language-level errors, never silent nil or undefined behavior.
5. Every binding that claims Tier-1 status must expose the agent-loop surface (check, validate with budget, eval with secret-aware JSON, full diagnostic code, span, severity, and message) and pass the shared conformance harness (`conformance/harness/run.py`) with identical codes, spans, and redacted JSON.
6. Every binding ships with explicit ownership and lifetime documentation written for both humans and AI agents, linked from the matrix below.

## Tier Definitions

- **Tier-1**: Full CI, ownership guarantees, complete agent-loop surface, documented, supported for production agent use.
- **Tier-2**: Best-effort; may lag; still required to follow opacity and free rules.

## Support Matrix

| Language              | Tier | Status     | Notes / Tracker |
|-----------------------|------|------------|-----------------|
| Go                    | 1    | Complete | External wazero repo (conformance CI) plus in-tree smoke, reference loop, and harness `go-native` path running the external suite; CHECKLIST 1.001-1.003, 2.020 (Go) done |
| Python                | 1    | Complete (gate pending) | WASM-first via wasmtime in `bindings/python` (`active.py`, 5/5 tests, harness path `python` green); CHECKLIST 1.004-1.006 done, Phase 1 gate pending |
| JavaScript/TypeScript | 1    | Complete (gate pending) | WASM-first via hand-written shim plus `bindings/js` wrapper (agent-loop + codes, tests green, harness path `js-native`); N-API deferred; npm name reserved; CHECKLIST 1.007-1.008 done, Phase 1 gate pending |
| Rust (external crate) | 1    | Complete (gate pending) | Published `active` facade re-exports `active_core`; crate doctest proves external parse plus eval; raw ABI via published `active-ffi`; CHECKLIST 1.009 done, Phase 1 gate pending |
| Java                  | 2    | Smoke green | JNI binding over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path java green in Linux container, SKIP without JDK; SPEC.md maps ownership rules |
| C#                   | 2    | Smoke green | `bindings/csharp` P/Invoke smoke over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path `csharp` green; SPEC.md maps ownership rules |
| C++                   | 2    | Smoke green | RAII smoke over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path cpp green in Linux container, SKIP without g++ or Linux staticlib; SPEC.md maps ownership rules |
| Zig                   | 2    | Smoke green | `bindings/zig/smoke.zig` drives the frozen C ABI directly (ABI gate, T001 refusal, redaction) via `run-smoke.py`; harness path `zig` green; SPEC.md maps ownership rules |
| Ruby                    | 2    | Smoke green | Fiddle over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path ruby green in Linux container, SKIP without ruby; SPEC.md maps ownership rules |
| PHP                       | 2    | Smoke green | FFI over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path php green in Linux container (ext-ffi), SKIP without php; SPEC.md maps ownership rules |
| Swift                 | 2    | Smoke green | `bindings/swift` C-shim smoke over the frozen C ABI (ABI gate, T001 refusal, redaction; Swift cannot name forward-declared C types, so a 10-function void\*/char\* shim stands between); harness path `swift` green in Linux container, SKIP without swiftc; SPEC.md maps ownership rules |
| Kotlin                | 2    | Smoke green | `bindings/kotlin` JNI smoke over the frozen C ABI mirroring bindings/java (ABI gate, T001 refusal, redaction); harness path `kotlin` green in Linux container, SKIP without kotlinc/java; SPEC.md maps ownership rules |
| Dart                  | 2    | Smoke green | `bindings/dart` package:ffi smoke over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path `dart` green in Linux container, SKIP without dart; SPEC.md maps ownership rules |
| Elixir                | 2    | Smoke green | `bindings/elixir` NIF smoke over the frozen C ABI (resource type with destructor, ABI gate, T001 refusal, redaction); harness path `elixir` green in Linux container, SKIP without elixir/gcc/erlang-dev; SPEC.md maps ownership rules |
| Haskell               | 2    | Smoke green | `bindings/haskell` foreign-import-ccall smoke over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path `haskell` green in Linux container, SKIP without ghc; SPEC.md maps ownership rules |
| OCaml                 | 2    | Smoke green | `bindings/ocaml` external-declaration smoke over the frozen C ABI plus a small C stub for owned-string copies (ABI gate, T001 refusal, redaction); harness path `ocaml` green in Linux container, SKIP without ocamlopt; SPEC.md maps ownership rules |
| Nim                   | 2    | Smoke green | `bindings/nim` importc smoke over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path `nim` green in Linux container, SKIP without nim; SPEC.md maps ownership rules |
| Crystal               | 2    | Smoke green | `bindings/crystal` lib-declaration smoke over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path `crystal` green in Linux container, SKIP without crystal; SPEC.md maps ownership rules |
| V                     | 2    | Smoke green | `bindings/vlang` C-interop smoke over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path `vlang` green in Linux container, SKIP without v; SPEC.md maps ownership rules |
| Julia                 | 2    | Smoke green | `bindings/julia` ccall smoke over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path `julia` green in Linux container, SKIP without julia; SPEC.md maps ownership rules |
| R                     | 2    | Smoke green | `bindings/r` .Call-wrapper smoke over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path `r` green in Linux container, SKIP without Rscript; SPEC.md maps ownership rules |
| Perl                  | 2    | Smoke green | `bindings/perl` FFI::Platypus smoke over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path `perl` green in Linux container, SKIP without perl/FFI::Platypus; SPEC.md maps ownership rules |
| Lua / LuaJIT          | 2    | Smoke green | LuaJIT FFI over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path lua green in Linux container, SKIP without luajit; SPEC.md maps ownership rules |

Update this matrix on every binding status change. Statuses above were
verified against the live repo on 2026-10-20 by running the shared harness
(`python conformance/harness/run.py`: reference, C ABI, WASM, and Go smoke
all pass) and reading the in-tree shims; nothing here is claimed from
memory.
