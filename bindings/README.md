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
| Go                    | 1    | In progress | External wazero binding repo plus in-tree `active-wasm/smoke-go` (green); conformance wiring pending CHECKLIST 1.001-1.003 |
| Python                | 1    | Planned    | WASM-first recipe exists; native binding mechanism decision pending CHECKLIST 1.004 |
| JavaScript/TypeScript | 1    | In progress | Hand-written `active-wasm/js` shim plus browser playground live; N-API path decision pending CHECKLIST 1.007 |
| Rust (external crate) | 1    | Planned    | In-workspace `active` facade exists; external re-export story pending CHECKLIST 1.009 |
| Java                  | 2    | Planned    | CHECKLIST 2.001 |
| C#                    | 2    | Planned    | CHECKLIST 2.002 |
| C++                   | 2    | Planned    | CHECKLIST 2.003 |
| Zig                   | 2    | Planned    | CHECKLIST 2.004 |
| Ruby                  | 2    | Planned    | CHECKLIST 2.005 |
| PHP                   | 2    | Planned    | CHECKLIST 2.006 |
| Swift                 | 2    | Planned    | CHECKLIST 2.007 |
| Kotlin                | 2    | Planned    | CHECKLIST 2.008 |
| Dart                  | 2    | Planned    | CHECKLIST 2.009 |
| Elixir                | 2    | Planned    | CHECKLIST 2.010 |
| Haskell               | 2    | Planned    | CHECKLIST 2.011 |
| OCaml                 | 2    | Planned    | CHECKLIST 2.012 |
| Nim                   | 2    | Planned    | CHECKLIST 2.013 |
| Crystal               | 2    | Planned    | CHECKLIST 2.014 |
| V                     | 2    | Planned    | CHECKLIST 2.015 |
| Lua / LuaJIT          | 2    | Planned    | CHECKLIST 2.016 |

Update this matrix on every binding status change.
