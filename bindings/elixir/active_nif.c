/* NIF shim for the Elixir smoke binding: wraps the frozen C ABI in
   Erlang NIFs. Module handles live in a resource type whose destructor
   calls active_module_free, so handles are freed exactly once even if
   the owning process dies; owned JSON is copied to a binary then freed
   before return; borrowed codes copy on return and are never freed. */
#include <string.h>

#include <erl_nif.h>

#include "active.h"

static ErlNifResourceType *mod_type;

static void mod_dtor(ErlNifEnv *env, void *obj) {
  (void)env;
  active_module_free(*(ActiveModule **)obj);
}

static int load(ErlNifEnv *env, void **priv, ERL_NIF_TERM info) {
  (void)priv;
  (void)info;
  mod_type = enif_open_resource_type(env, NULL, "active_module", mod_dtor,
                                     ERL_NIF_RT_CREATE, NULL);
  return mod_type ? 0 : 1;
}

static ERL_NIF_TERM nif_abi(ErlNifEnv *env, int argc,
                            const ERL_NIF_TERM argv[]) {
  (void)argc;
  (void)argv;
  return enif_make_uint(env, active_abi_version());
}

static ERL_NIF_TERM nif_parse(ErlNifEnv *env, int argc,
                              const ERL_NIF_TERM argv[]) {
  ErlNifBinary src;
  (void)argc;
  if (!enif_inspect_binary(env, argv[0], &src))
    return enif_make_badarg(env);
  ActiveModule *m = active_module_parse(src.data, src.size, NULL);
  if (!m)
    return enif_make_atom(env, "parse_error");
  ActiveModule **res =
      (ActiveModule **)enif_alloc_resource(mod_type, sizeof(ActiveModule *));
  *res = m;
  ERL_NIF_TERM out = enif_make_resource(env, res);
  enif_release_resource(res);
  return out;
}

static ERL_NIF_TERM nif_check(ErlNifEnv *env, int argc,
                              const ERL_NIF_TERM argv[]) {
  ActiveModule **resp;
  (void)argc;
  if (!enif_get_resource(env, argv[0], mod_type, (void **)&resp))
    return enif_make_badarg(env);
  ActiveDiagnosticSet *set = active_check(*resp, NULL);
  if (!set)
    return enif_make_atom(env, "check_failed");
  uintptr_t n = active_diagnostics_len(set);
  ERL_NIF_TERM *items =
      (ERL_NIF_TERM *)enif_alloc(sizeof(ERL_NIF_TERM) * (n ? n : 1));
  for (uintptr_t i = 0; i < n; i++) {
    const char *code = active_diagnostic_code(set, i);
    size_t len = strlen(code ? code : "");
    ERL_NIF_TERM bin;
    unsigned char *buf = enif_make_new_binary(env, len, &bin);
    if (!buf)
      return enif_make_atom(env, "alloc_failed");
    memcpy(buf, code ? code : "", len);
    items[i] = bin;
  }
  ERL_NIF_TERM out = enif_make_list_from_array(env, items, (unsigned)n);
  enif_free(items);
  active_diagnostics_free(set);
  return out;
}

static ERL_NIF_TERM nif_eval(ErlNifEnv *env, int argc,
                             const ERL_NIF_TERM argv[]) {
  ActiveModule **resp;
  (void)argc;
  if (!enif_get_resource(env, argv[0], mod_type, (void **)&resp))
    return enif_make_badarg(env);
  ActiveValue *v = active_eval(*resp, NULL);
  if (!v)
    return enif_make_atom(env, "refused");
  char *json = active_value_to_json(v);
  size_t len = strlen(json ? json : "");
  ERL_NIF_TERM bin;
  unsigned char *buf = enif_make_new_binary(env, len, &bin);
  if (!buf) {
    active_string_free(json);
    active_value_free(v);
    return enif_make_atom(env, "alloc_failed");
  }
  memcpy(buf, json ? json : "", len);
  active_string_free(json);
  active_value_free(v);
  return bin;
}

static ErlNifFunc funcs[] = {
    {"abi", 0, nif_abi},
    {"parse", 1, nif_parse},
    {"check", 1, nif_check},
    {"eval_json", 1, nif_eval},
};

ERL_NIF_INIT(Elixir.ActiveNif, funcs, load, NULL, NULL, NULL)
