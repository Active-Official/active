// Declarations for the Swift smoke shim (see active_swift.c). Uses only
// plain C pointer and integer types so Swift can name everything; the
// frozen C ABI handle types stay behind the shim.
#ifndef ACTIVE_SWIFT_SHIM_H
#define ACTIVE_SWIFT_SHIM_H

#include <stdint.h>

uint32_t sw_abi(void);
void *sw_parse(const uint8_t *src, uintptr_t len);
void sw_mod_free(void *m);
void *sw_check(const void *m);
void *sw_eval(const void *m);
uintptr_t sw_len(const void *s);
const char *sw_code(const void *s, uintptr_t i);
void sw_set_free(void *s);
void sw_val_free(void *v);
char *sw_to_json(const void *v);
void sw_str_free(char *s);

#endif
