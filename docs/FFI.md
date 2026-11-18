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
defaults; a non-null budget may only TIGHTEN them (LANG-G-008): a zero
field keeps the current limit, a non-zero field must be no larger than it,
and `u64::MAX` is rejected like any other loosening. The one exception is
the wall clock under trusted mode (the parse options flag below); count and
size limits stay tighten-only in both modes.

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

Budget zeros: `ActiveBudget` uses `0` for "keep the current limit" per
field, exactly like `Budget::resolve()` uses zero for the shipped default.
A non-zero field is accepted only when it is at or below the current limit
(tighten-only); anything larger fails closed with `BAD_BUDGET` (12) and no
evaluation runs. Parse bounds use the same zero-means-default rule.

Options: `active_module_parse` takes `options: *const ActiveOptions`, or
`NULL` for tighten-only mode. The struct is `{ uint32_t struct_size;
uint32_t trusted; }`; the size word must equal `sizeof(ActiveOptions)` and
`trusted` must be 0 or 1, otherwise the parse fails closed with
`BAD_ARGUMENT` (14). A non-null options pointer is never silently ignored.

Parse bounds: `active_parse_limits_set(max_input_bytes, max_wall_ms)` sets
process-wide parse ceilings used by every later parse (LANG-C-003); `0`
keeps the shipped default for that field (4 MiB input, a wall budget
that scales with input).
Defaults apply without any call: an oversize document fails parse with
`PARSE` (1) instead of being lexed. The C boundary intentionally offers no
"unlimited" request — a host that needs larger documents raises the ceilings
to explicit values.

## 3. Error codes (F-002; codes are API per G-025)

Every value a host can receive is reachable (LANG-G-007): an unresolved
reference during evaluation is `UNRESOLVED_REF` (2), an include family
failure (E015/E016/E017/E028/E041) is `INCLUDE_FAIL` (7), and a schema/unit/
cycle/budget refusal is its own code. `active_check` and `active_validate`
also report a class from the set they return: an error-severity diagnostic
wins, and a set that carries only `W001` (unresolved identifier) reports 2,
so a host polling `active_last_error` after a check sees the same verdict
the CLI applies downstream. `ABI_MISMATCH` (11) is reserved: the v1
surface has no host-declared ABI parameter, so no path sets it; it stays
documented and allocated so the number is never reused.

Stable `ACTIVE_ERR_*` integers: OK(0), PARSE(1), UNRESOLVED_REF(2),
CYCLE(3), UNIT_MISMATCH(4), SECRET_REFUSAL(5), BUDGET_EXCEEDED(6),
INCLUDE_FAIL(7), SCHEMA_FAIL(8), NULL_HANDLE(9), PANIC_CAUGHT(10),
ABI_MISMATCH(11), BAD_BUDGET(12), UTF8(13), BAD_ARGUMENT(14). Codes map from internal
diagnostic classes (E/W/H/T/S/X plus budgets); codes are never reused and
change only additively within 1.x. Diagnostics export stable code, span
(path or index, line, column, end line, end column), severity, and optional
UTF-8 message.

The receiver gate's refusals (`E033` envelope version, `E034` capability)
surface numerically as `SCHEMA_FAIL` (8) from `active_eval`; the precise
code and message arrive through `active_check`'s diagnostic set, which is
how a host tells a negotiation refusal from a schema failure.

## 4. Budget struct (F-014, G-016)

```c
typedef struct {
  uint32_t struct_size;   /* field zero: set to sizeof(ActiveBudget);
                             callee validates; mismatch fails closed */
  uint64_t max_nodes;     /* 0 keeps current; else must be <= current */
  uint64_t max_depth;     /* 0 keeps current; else must be <= current */
  uint64_t max_array_len; /* 0 keeps current; else must be <= current */
  uint64_t max_keys;      /* 0 keeps current; else must be <= current */
  uint64_t max_wall_ms;   /* 0 keeps current; trusted mode may raise it */
  uint64_t max_value_size;      /* 0 keeps current; else <= current */
  uint64_t max_total_value_size;/* 0 keeps current; else <= current */
} ActiveBudget;
```

Shipped defaults equal the CLI profile (nodes 100000, depth 256, array
100000, keys 100000, wall 100 ms, value 10 MB, total 100 MB). The current
limit for a check/eval is the engine's shipped profile, so a host can lower
a limit but never lift one. Trusted mode (the `trusted` flag on the parse
options struct) may raise the wall clock, and only the wall clock; setting
it to `u64::MAX` is the explicit way to disable that one limit. Count and
size limits cannot be disabled through this struct in either mode.

## 5. Allocator model A (F-003, chosen for all of 1.x)

The core allocates every buffer that crosses the boundary; the host frees
with the matching typed free (`active_string_free` for owned strings,
`active_module_free`, `active_value_free`, `active_diagnostics_free`). No
host-provided arenas, no custom allocator injection. The model never
changes within 1.x; a different model would be an ABI break gated by the
version constant.

## 6. Threading model (G-018)

Modules are thread-confined: one module lives on one thread at a time.
Callbacks run on the calling thread and must not re-enter evaluation on
the same module. Sharing a handle across threads without external
synchronization is a host bug; the threaded smoke test proves confined use
plus a contested Misuse case failing closed (not UB).

### 6.1 Callback buffers (LANG-I-014)

An include loader or env lookup returns a **malloc-allocated**,
NUL-terminated UTF-8 string, or `NULL` for a miss. The library copies the
bytes immediately and frees the pointer with `free`. Returning any other
pointer — a string literal, a stack buffer, or memory from a different
allocator — is **undefined behavior**, not merely unsupported: the
library calls `free` on it.

C:

```c
char *load(const char *name, void *userdata) {
    const char *text = lookup_in_your_store(name);
    return text ? strdup(text) : NULL;   /* malloc'd; the library frees it */
}
```

Rust (the host must hand ownership to `malloc`, because the library frees
with `free`; a `CString::into_raw` pointer is only safe if the host's
global allocator is the C allocator):

```rust
unsafe extern "C" fn load(name: *const c_char, _ud: *mut c_void) -> *mut c_char {
    let key = std::ffi::CStr::from_ptr(name).to_string_lossy();
    let Some(text) = your_store_get(&key) else { return std::ptr::null_mut() };
    let bytes = text.as_bytes();
    let ptr = libc::malloc(bytes.len() + 1).cast::<u8>();
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
    *ptr.add(bytes.len()) = 0;
    ptr.cast()
}
```

Null returns a miss; the callback must not re-enter evaluation on the
same module and must outlive every later check/eval on the handle.

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

`active_format` returns SOURCE TEXT: a `secret("...")` literal in the
input is returned verbatim, exactly as the file holds it. That is not a
leak (the output's exposure equals the input file's), but a host must not
log formatted output (LANG-I-011); every JSON exporter redacts.
