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
| Swift                 | 2    | Planned    | SPEC drafted, no toolchain on this machine; design recorded, implementation deferred (CHECKLIST 2.007) |
| Kotlin                | 2    | Planned    | SPEC drafted, no toolchain on this machine; design recorded, implementation deferred (CHECKLIST 2.008) |
| Dart                  | 2    | Planned    | SPEC drafted, no toolchain on this machine; design recorded, implementation deferred (CHECKLIST 2.009) |
| Elixir                | 2    | Planned    | SPEC drafted, no toolchain on this machine; design recorded, implementation deferred (CHECKLIST 2.010) |
| Haskell               | 2    | Planned    | SPEC drafted, no toolchain on this machine; design recorded, implementation deferred (CHECKLIST 2.011) |
| OCaml                 | 2    | Planned    | SPEC drafted, no toolchain on this machine; design recorded, implementation deferred (CHECKLIST 2.012) |
| Nim                   | 2    | Planned    | SPEC drafted, no toolchain on this machine; design recorded, implementation deferred (CHECKLIST 2.013) |
| Crystal               | 2    | Planned    | SPEC drafted, no toolchain on this machine; design recorded, implementation deferred (CHECKLIST 2.014) |
| V                     | 2    | Planned    | SPEC drafted, no toolchain on this machine; design recorded, implementation deferred (CHECKLIST 2.015) |
| Lua / LuaJIT          | 2    | Smoke green | LuaJIT FFI over the frozen C ABI (ABI gate, T001 refusal, redaction); harness path lua green in Linux container, SKIP without luajit; SPEC.md maps ownership rules |

Update this matrix on every binding status change. Statuses above were
verified against the live repo on 2026-10-20 by running the shared harness
(`python conformance/harness/run.py`: reference, C ABI, WASM, and Go smoke
all pass) and reading the in-tree shims; nothing here is claimed from
memory.
