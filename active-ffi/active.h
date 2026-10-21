#ifdef __cplusplus
extern "C" {
#endif

#ifndef ACTIVE_H
#define ACTIVE_H

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * ABI version line (docs/FFI.md section 8). Bindings fail closed on major
 * mismatch (H-002).
 */
#define ACTIVE_ABI_VERSION 1

/**
 * Error codes (docs/FFI.md section 3; codes are API, never reused).
 */
#define ACTIVE_ERR_OK 0

#define ACTIVE_ERR_PARSE 1

#define ACTIVE_ERR_UNRESOLVED_REF 2

#define ACTIVE_ERR_CYCLE 3

#define ACTIVE_ERR_UNIT_MISMATCH 4

#define ACTIVE_ERR_SECRET_REFUSAL 5

#define ACTIVE_ERR_BUDGET_EXCEEDED 6

#define ACTIVE_ERR_INCLUDE_FAIL 7

#define ACTIVE_ERR_SCHEMA_FAIL 8

#define ACTIVE_ERR_NULL_HANDLE 9

#define ACTIVE_ERR_PANIC_CAUGHT 10

#define ACTIVE_ERR_ABI_MISMATCH 11

#define ACTIVE_ERR_BAD_BUDGET 12

#define ACTIVE_ERR_UTF8 13

#define ACTIVE_ERR_BAD_ARGUMENT 14

/**
 * Owned diagnostic set from check/eval (F-013). Empty means clean.
 */
typedef struct ActiveDiagnosticSet ActiveDiagnosticSet;

/**
 * Opaque parsed module (F-012/F-016). Stores the SOURCE plus registered
 * host callbacks, not a parsed tree: every check/eval re-parses fresh so
 * callbacks observe the current registrations and no stale state survives
 * across calls. Deterministic for fixed inputs and callbacks.
 */
typedef struct ActiveModule ActiveModule;

/**
 * Owned evaluated value handle (F-014/F-015).
 */
typedef struct ActiveValue ActiveValue;

typedef struct Option_ActiveLookupFn Option_ActiveLookupFn;

/**
 * C layout of the budget struct (docs/FFI.md section 4). Field zero is
 * the size word (G-016); every other zero selects its shipped default.
 */
typedef struct ActiveBudget {
  uint32_t struct_size;
  uint64_t max_nodes;
  uint64_t max_depth;
  uint64_t max_array_len;
  uint64_t max_keys;
  uint64_t max_wall_ms;
  uint64_t max_value_size;
  uint64_t max_total_value_size;
} ActiveBudget;

/**
 * ABI version constant for bindings to check at load time.
 * # Safety
 *
 * No preconditions; always safe to call.
 */
uint32_t active_abi_version(void);

/**
 * Code from the last failed call on this thread (OK when clean).
 * # Safety
 *
 * No preconditions; always safe to call.
 */
int32_t active_last_error(void);

/**
 * Register the host include loader on a live handle (F-016). Null `cb`
 * unregisters (legacy hermetic behavior returns). Null module is a
 * no-op miss recorded as NULL_HANDLE for the next `active_last_error`.
 * # Safety
 *
 * The module must be a live handle from `active_module_parse` on the
 * calling thread. The callback and userdata must outlive every later
 * check/eval on this handle, and the callback must not re-enter
 * evaluation on the same module. Buffers the callback returns must
 * be malloc-allocated (the core copies then frees with `free`).
 */
void active_set_include_loader(struct ActiveModule *module,
                               struct Option_ActiveLookupFn cb,
                               void *userdata);

/**
 * Register the host env lookup on a live handle (F-016). Semantics mirror
 * the snapshot map: consulted on miss before defaults, charged equally.
 * Null `cb` unregisters.
 * # Safety
 *
 * Same obligations as `active_set_include_loader`: live handle,
 * calling thread, outliving callback and userdata, no re-entrancy,
 * malloc-allocated return buffers.
 */
void active_set_env_lookup(struct ActiveModule *module,
                           struct Option_ActiveLookupFn cb,
                           void *userdata);

/**
 * Parse UTF-8 source bytes into an owned module handle (null on error;
 * detail via `active_last_error`). Null-safe on the source pointer.
 * # Safety
 *
 * When `source` is non-null, `len` bytes starting at `source` must be
 * readable for this call. `options` is reserved and must be null.
 */
struct ActiveModule *active_module_parse(const uint8_t *source,
                                         uintptr_t len,
                                         const void *_options);

/**
 * Free a module handle. Null-safe; double-free is a host bug, not UB
 * here (second free of the same pointer is still UB by Rust aliasing
 * rules; H-001 tests the null and misuse paths that are defined).
 * # Safety
 *
 * The handle must come from `active_module_parse` and must not have
 * been freed before; null is accepted. Use confines to one thread.
 */
void active_module_free(struct ActiveModule *handle);

/**
 * Check a module without producing values (F-013/F-019 shape): returns an
 * owned set, empty when clean. Mirrors `active check` strictness
 * (warnings fail downstream; the set carries them).
 * # Safety
 *
 * The module must be a live handle on the calling thread. The budget
 * pointer must be null or point to a valid `ActiveBudget` whose
 * struct_size field was set by the caller.
 */
struct ActiveDiagnosticSet *active_check(const struct ActiveModule *module,
                                         const struct ActiveBudget *budget);

/**
 * Evaluate a module to an owned value handle, or null on error with the
 * diagnostic available through a follow-up `active_check` (F-014).
 * # Safety
 *
 * Same obligations as `active_check`: live handle, calling thread,
 * null-or-valid budget with a truthful size word.
 */
struct ActiveValue *active_eval(const struct ActiveModule *module,
                                const struct ActiveBudget *budget);

/**
 * Number of diagnostics in the set.
 * # Safety
 *
 * The set must be a live set from `active_check`/`active_validate`;
 * null yields 0 without dereference.
 */
uintptr_t active_diagnostics_len(const struct ActiveDiagnosticSet *set);

/**
 * Code string at index, borrowed from the set (null on OOB).
 * # Safety
 *
 * Live set; the returned pointer borrows the set and dies with it.
 * Out-of-bounds index yields null without dereference.
 */
const char *active_diagnostic_code(const struct ActiveDiagnosticSet *set, uintptr_t index);

/**
 * Severity at index: 1 error, 2 warning, 3 info, 4 hint; 0 on OOB.
 * # Safety
 *
 * Live set; out-of-bounds index yields 0 without dereference.
 */
uint32_t active_diagnostic_severity(const struct ActiveDiagnosticSet *set, uintptr_t index);

/**
 * Span at index into out[4] (line, col, end_line, end_col); false when the
 * diagnostic carries no span or the index is OOB.
 * # Safety
 *
 * Live set plus 4 writable `u32` slots at `out` when non-null;
 * null `out` fails closed. Out-of-bounds or spanless index yields
 * false without dereference.
 */
bool active_diagnostic_span(const struct ActiveDiagnosticSet *set, uintptr_t index, uint32_t *out);

/**
 * Message string at index, borrowed from the set (null on OOB).
 * # Safety
 *
 * Live set; the returned pointer borrows the set and dies with it.
 * Out-of-bounds index yields null without dereference.
 */
const char *active_diagnostic_message(const struct ActiveDiagnosticSet *set, uintptr_t index);

/**
 * Free a diagnostic set. Null-safe.
 * # Safety
 *
 * The set must come from `active_check`/`active_validate` and must
 * not have been freed before; null is accepted.
 */
void active_diagnostics_free(struct ActiveDiagnosticSet *set);

/**
 * Free a value handle. Null-safe.
 * # Safety
 *
 * The value must come from `active_eval` and must not have been
 * freed before; null is accepted.
 */
void active_value_free(struct ActiveValue *value);

/**
 * JSON export with secrets redacted by construction (F-015). Returns an
 * owned string the host frees with `active_string_free`.
 * # Safety
 *
 * The value must be a live handle. The returned string is Model A
 * owned: free exactly once with `active_string_free`.
 */
char *active_value_to_json(const struct ActiveValue *value);

/**
 * Taint query for hosts (F-015): true exactly for secret values, at any
 * depth only when the value itself is secret material.
 * # Safety
 *
 * The value must be a live handle; null yields false without
 * dereference.
 */
bool active_value_is_secret(const struct ActiveValue *value);

/**
 * Coarse type code (F-015): 0 null, 1 bool, 2 int, 3 float, 4 string,
 * 5 duration, 6 quantity, 7 array, 8 table, 9 secret; u32::MAX on null.
 * # Safety
 *
 * The value must be a live handle; null yields `u32::MAX` without
 * dereference.
 */
uint32_t active_value_type(const struct ActiveValue *value);

/**
 * Validate a module against its in-file schema, returning schema
 * diagnostics (F-020): the generator feedback signal without parse/type
 * noise. Runs the single shipped validator through evaluation and keeps
 * T002-T006; a fatal evaluation failure that prevents validation surfaces
 * with its true code instead of a lying empty set (empty means
 * schema-clean AND evaluable: run `active_check` for the full verdict).
 * Cross-document schemas ride includes (F-016 loader).
 * # Safety
 *
 * Same obligations as `active_check`: live handle, calling thread,
 * null-or-valid budget with a truthful size word.
 */
struct ActiveDiagnosticSet *active_validate(const struct ActiveModule *module,
                                            const struct ActiveBudget *budget);

/**
 * Format a module to canonical text (F-021): style 0 brace, 1 indent,
 * 2 preserve; other values fail closed with BAD_ARGUMENT. Powers the
 * playground brace/indent toggle; hosts never reimplement layout.
 * Includes stay unresolved (format preserves statements, like active-fmt).
 * # Safety
 *
 * The module must be a live handle. The returned string is Model A
 * owned: free exactly once with `active_string_free`.
 */
char *active_format(const struct ActiveModule *module, uint32_t style);

/**
 * Free a string returned by `active_value_to_json`. Null-safe (Model A).
 * # Safety
 *
 * The pointer must come from `active_value_to_json`/`active_format`
 * and must not have been freed before; null is accepted.
 */
void active_string_free(char *s);

#endif  /* ACTIVE_H */

#ifdef __cplusplus
}
#endif
