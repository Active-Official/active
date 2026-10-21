/* C stubs for the OCaml smoke binding. OCaml externals cannot traffic
   raw C pointers as values (the GC would chase them), so every handle
   crosses as a nativeint and these stubs convert at the boundary.
   Ownership stays Model A: owned JSON is copied to an OCaml string and
   freed before return; borrowed codes copy on return and are never
   freed. */
#include <caml/alloc.h>
#include <caml/memory.h>
#include <caml/mlvalues.h>
#include <string.h>

#include "active.h"

#define MOD(v) ((ActiveModule *)(intnat)Nativeint_val(v))
#define SET(v) ((ActiveDiagnosticSet *)(intnat)Nativeint_val(v))
#define VAL(v) ((ActiveValue *)(intnat)Nativeint_val(v))
#define PTR(p) caml_copy_nativeint((intnat)(p))

CAMLprim value ml_abi(value unit) {
  (void)unit;
  return caml_copy_int32(active_abi_version());
}

CAMLprim value ml_parse(value src, value len) {
  CAMLparam2(src, len);
  ActiveModule *m = active_module_parse((const uint8_t *)String_val(src),
                                        (uintptr_t)Long_val(len), NULL);
  CAMLreturn(PTR(m));
}

CAMLprim value ml_mod_free(value m) {
  CAMLparam1(m);
  active_module_free(MOD(m));
  CAMLreturn(Val_unit);
}

CAMLprim value ml_check(value m) {
  CAMLparam1(m);
  CAMLreturn(PTR(active_check(MOD(m), NULL)));
}

CAMLprim value ml_eval(value m) {
  CAMLparam1(m);
  CAMLreturn(PTR(active_eval(MOD(m), NULL)));
}

CAMLprim value ml_len(value s) {
  CAMLparam1(s);
  CAMLreturn(Val_long((intnat)active_diagnostics_len(SET(s))));
}

CAMLprim value ml_code(value s, value i) {
  CAMLparam2(s, i);
  const char *code = active_diagnostic_code(SET(s), (uintptr_t)Long_val(i));
  CAMLreturn(caml_copy_string(code ? code : ""));
}

CAMLprim value ml_set_free(value s) {
  CAMLparam1(s);
  active_diagnostics_free(SET(s));
  CAMLreturn(Val_unit);
}

CAMLprim value ml_val_free(value v) {
  CAMLparam1(v);
  active_value_free(VAL(v));
  CAMLreturn(Val_unit);
}

CAMLprim value ml_eval_json(value v) {
  CAMLparam1(v);
  if (!VAL(v))
    caml_invalid_argument("active_value_to_json: null value");
  char *s = active_value_to_json(VAL(v));
  value r = caml_copy_string(s ? s : "");
  active_string_free(s);
  CAMLreturn(r);
}
