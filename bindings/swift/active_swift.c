/* C shim for the Swift smoke binding: Swift cannot name the header's
   forward-declared handle types, so this shim re-exposes the frozen C
   ABI through void* (UnsafeMutableRawPointer) and char* (CChar string
   pointers). No logic duplicated: every function forwards once, and
   ownership stays Model A (owned JSON copied to a Swift string, then
   freed before return). */
#include <stdint.h>
#include <stdlib.h>

#include "active.h"

uint32_t sw_abi(void) { return active_abi_version(); }

void *sw_parse(const uint8_t *src, uintptr_t len) {
  return active_module_parse(src, len, NULL);
}

void sw_mod_free(void *m) { active_module_free((ActiveModule *)m); }

void *sw_check(const void *m) {
  return active_check((const ActiveModule *)m, NULL);
}

void *sw_eval(const void *m) {
  return active_eval((const ActiveModule *)m, NULL);
}

uintptr_t sw_len(const void *s) {
  return active_diagnostics_len((const ActiveDiagnosticSet *)s);
}

const char *sw_code(const void *s, uintptr_t i) {
  return active_diagnostic_code((const ActiveDiagnosticSet *)s, i);
}

void sw_set_free(void *s) { active_diagnostics_free((ActiveDiagnosticSet *)s); }

void sw_val_free(void *v) { active_value_free((ActiveValue *)v); }

char *sw_to_json(const void *v) {
  return active_value_to_json((const ActiveValue *)v);
}

void sw_str_free(char *s) { active_string_free(s); }
