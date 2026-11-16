/*
 * Call every exported function through the COMMITTED header (LANG-G-001).
 *
 * The drift gate compares the committed header with fresh cbindgen output,
 * so it cannot catch a declaration that is generated consistently but is
 * uncallable from C (an opaque struct instead of a function-pointer
 * typedef, an incomplete parameter type, a wrong arity). This file is
 * compiled by `active-ffi/tests/header_callable.rs` with the committed
 * header on the include path, so an uncallable declaration fails CI.
 *
 * Nothing here runs: the file only has to COMPILE. Every expression is
 * written so the compiler type-checks the call and its arguments.
 */

#include "../active.h"

#include <stdbool.h>
#include <stdint.h>

/* A callback with the typedef's exact signature. */
static char *lookup_cb(const char *path, void *userdata) {
  (void)path;
  (void)userdata;
  return (char *)0;
}

/* Reference every exported symbol and every declared call shape. */
int active_ffi_call_every_export(void) {
  const struct ActiveBudget *budget = (const struct ActiveBudget *)0;
  const char *source = "port = 1\n";
  const char *path = "port";
  uint32_t span[4] = {0, 0, 0, 0};

  /* Process-wide and version surface. */
  (void)active_parse_limits_set((uintptr_t)1048576, (uint64_t)1000);
  (void)active_abi_version();
  (void)active_last_error();

  /* Parse, then register callbacks on the live handle. NULL unregisters. */
  struct ActiveModule *module = active_module_parse(
      (const uint8_t *)source, (uintptr_t)9, (const void *)0);
  active_set_include_loader(module, lookup_cb, (void *)0);
  active_set_env_lookup(module, lookup_cb, (void *)0);
  active_set_include_loader(module, (ActiveLookupFn)0, (void *)0);
  active_set_env_lookup(module, (ActiveLookupFn)0, (void *)0);

  /* Diagnostics: every accessor through a real set. */
  struct ActiveDiagnosticSet *checked = active_check(module, budget);
  uintptr_t diagnostics = active_diagnostics_len(checked);
  for (uintptr_t i = 0; i < diagnostics; i++) {
    (void)active_diagnostic_code(checked, i);
    (void)active_diagnostic_severity(checked, i);
    (void)active_diagnostic_span(checked, i, span);
    (void)active_diagnostic_message(checked, i);
  }
  active_diagnostics_free(checked);

  struct ActiveDiagnosticSet *validated = active_validate(module, budget);
  active_diagnostics_free(validated);

  /* Values: evaluate, then every value surface. */
  struct ActiveValue *value = active_eval(module, budget);
  (void)active_value_type(value);
  (void)active_value_is_secret(value);
  (void)active_value_is_tainted(value);
  char *json = active_value_to_json(value);
  active_string_free(json);
  char *projected = active_value_project(value, path, (uintptr_t)4);
  active_string_free(projected);
  active_value_free(value);

  /* Formatting through the module. */
  char *formatted = active_format(module, (uint32_t)0);
  active_string_free(formatted);

  active_module_free(module);
  return (int)diagnostics;
}
