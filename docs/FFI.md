# Active FFI Contract v1 (frozen for the 1.x line)

Status: normative. Reviewed before implementation (F-001). Any change here
requires a checklist update, a conformance run, and the H-004 review.

## 1. Handles (opaque, nullable-checked)

`ActiveModule*`, `ActiveValue*`, `ActiveDiagnosticSet*`, `ActiveString`.
Hosts never inspect interiors, never free them except through the matching
`active_*_free` function, and never pass a freed handle back. Every entry
point treats null handles as `ACTIVE_ERR_NULL_HANDLE`, never as UB.

## 2. Entry points

Lifecycle: `active_module_parse(source: *const u8, len: usize,
options: *const ActiveOptions) -> *mut ActiveModule` (null on error; error
detail via `active_last_error`), `active_module_free`,
`active_value_free`, `active_diagnostics_free`, `active_string_free`.
`active_abi_version() -> u32` returns `ACTIVE_ABI_VERSION` (1).

Check without evaluating (the agent-loop primitive, G-015):
`active_check(module) -> *mut ActiveDiagnosticSet`. Exit contract: empty
set means clean; presence of any diagnostic means reject, warnings included
(matching `active check` strictness).

Evaluate: `active_eval(module, budget: *const ActiveBudget) ->
*mut ActiveValue` (null on error). A null budget pointer selects shipped
defaults; a non-null budget tightens them (hosts can only tighten critical
limits; disabling requires the documented trusted-mode flag).

Validate against the in-file schema without returning values (G-015):
`active_validate(module, budget) -> *mut ActiveDiagnosticSet`. Runs the single shipped validator and keeps
T002-T006; a fatal evaluation failure surfaces with its true code instead
of a lying empty set.

Format and convert (G-015): `active_format(module, style: u32) ->
*mut ActiveString` with style 0 brace, 1 indent, 2 preserve (other values
fail closed with BAD_ARGUMENT; the playground brace/indent toggle calls
this, never a host reimplementation).

Inspection: `active_value_to_json(value) -> *mut ActiveString` (secrets
always redacted; provenance of this guarantee: core taint rules, H-001
log-leak trap); `active_value_is_secret(value) -> bool` (accurate for
nested values); `active_value_is_tainted(value) -> bool` (secrets plus
structurally tainted blobs); `active_value_project(value, path, len)`
demanded-cone JSON (PROPOSAL §2.2; null on unknown path); `active_value_type(value) -> ActiveType` enum (10 = blob identity).

Host callbacks: `active_set_include_loader(module, cb, userdata)` and
`active_set_env_lookup(module, cb, userdata)`. Callbacks run on the calling
thread, must not re-enter eval on the same module, and return owned buffers
under Model A below. Callback failures surface as diagnostics; they never
hang the call.

## 3. Error codes (F-002; codes are API per G-025)

Stable `ACTIVE_ERR_*` integers: OK(0), PARSE(1), UNRESOLVED_REF(2),
CYCLE(3), UNIT_MISMATCH(4), SECRET_REFUSAL(5), BUDGET_EXCEEDED(6),
INCLUDE_FAIL(7), SCHEMA_FAIL(8), NULL_HANDLE(9), PANIC_CAUGHT(10),
ABI_MISMATCH(11), BAD_BUDGET(12), UTF8(13), BAD_ARGUMENT(14). Codes map from internal
diagnostic classes (E/W/H/T/S/X plus budgets); codes are never reused and
change only additively within 1.x. Diagnostics export stable code, span
(path or index, line, column, end line, end column), severity, and optional
UTF-8 message.

## 4. Budget struct (F-014, G-016)

```c
typedef struct {
  uint32_t struct_size;   /* field zero: set to sizeof(ActiveBudget);
                             callee validates; mismatch fails closed */
  uint64_t max_nodes;     /* 0 selects shipped default (100000) */
  uint64_t max_depth;     /* 0 selects default (256) */
  uint64_t max_array_len; /* 0 selects default (100000) */
  uint64_t max_keys;      /* 0 selects default (100000) */
  uint64_t max_wall_ms;   /* 0 selects default (100); trusted mode may raise */
  uint64_t max_value_size;      /* 0 selects default (10000000) */
  uint64_t max_total_value_size;/* 0 selects default (100000000) */
} ActiveBudget;
```

Shipped defaults equal the CLI profile. Trusted mode (explicit flag on the
options struct, documented per call site) may raise wall-clock only; the
count and size limits cannot be disabled through this struct.

## 5. Allocator model A (F-003, chosen for all of 1.x)

The core allocates every buffer that crosses the boundary; the host frees
with `active_string_free` / `active_buf_free` (and the typed free
functions). No host-provided arenas, no custom allocator injection. The
model never changes within 1.x; a different model would be an ABI break
gated by the version constant.

## 6. Threading model (G-018)

Modules are thread-confined: one module lives on one thread at a time.
Callbacks run on the calling thread and must not re-enter evaluation on
the same module. Sharing a handle across threads without external
synchronization is a host bug; the threaded smoke test proves confined use
plus a contested Misuse case failing closed (not UB).

## 7. Security rules at the boundary

1. `catch_unwind` on every export; panics become PANIC_CAUGHT.
2. Hosts check `is_secret` before logging or displaying; JSON export
   always redacts; the H-001 log-leak trap fails the suite if raw secret
   bytes ever reach host-visible output.
3. No implicit I/O: includes and env flow only through host callbacks.
4. Opaque ownership per section 1; deterministic allocator policy per
   section 5.

## 8. Versioning and drift control

`active_abi_version()` returns 1. Bindings record the ABI they were built
against and refuse to load on major mismatch (H-002 test). The C header is
generated by cbindgen from the Rust exports and committed; any export change
regenerates it in the same commit, so the header and the exports are
reviewed together.
