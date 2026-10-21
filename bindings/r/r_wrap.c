/* C wrapper for the R smoke binding: exposes the frozen C ABI through
   .Call entry points returning R values. Ownership stays Model A: owned
   JSON is copied to an R string then freed before return; borrowed
   codes copy on return and are never freed; every handle is freed on
   all paths (cleanup runs before any R error unwinds). */
#include <R.h>
#include <Rinternals.h>

#include <string.h>

#include "active.h"

SEXP r_abi_version(void) { return ScalarInteger((int)active_abi_version()); }

static ActiveModule *r_parse_or_null(const char *src) {
  return active_module_parse((const uint8_t *)src, strlen(src), NULL);
}

/* Context for the unwind-protected fill callbacks below: R allocations
   (allocVector/mkChar) can longjmp on OOM, which would otherwise skip
   the active_*_free calls. R_UnwindProtect runs the cleanup on both
   the success and the jump path, so every handle is freed exactly once. */
typedef struct {
  ActiveModule *mod;
  ActiveDiagnosticSet *set;
  ActiveValue *val;
  uintptr_t n;
} RCheckCtx;

static void r_check_cleanup(void *data, Rboolean jump) {
  (void)jump;
  RCheckCtx *c = (RCheckCtx *)data;
  if (c->set)
    active_diagnostics_free(c->set);
  if (c->mod)
    active_module_free(c->mod);
  c->set = NULL;
  c->mod = NULL;
}

static SEXP r_check_fill(void *data) {
  RCheckCtx *c = (RCheckCtx *)data;
  SEXP out = PROTECT(allocVector(STRSXP, (R_xlen_t)c->n));
  for (uintptr_t i = 0; i < c->n; i++) {
    const char *code = active_diagnostic_code(c->set, i);
    SET_STRING_ELT(out, (R_xlen_t)i, mkChar(code ? code : ""));
  }
  UNPROTECT(1);
  return out;
}

SEXP r_check_codes(SEXP src_) {
  const char *src = CHAR(STRING_ELT(src_, 0));
  ActiveModule *m = r_parse_or_null(src);
  if (!m)
    return R_NilValue;
  ActiveDiagnosticSet *set = active_check(m, NULL);
  if (!set) {
    active_module_free(m);
    return R_NilValue;
  }
  RCheckCtx ctx;
  ctx.mod = m;
  ctx.set = set;
  ctx.val = NULL;
  ctx.n = active_diagnostics_len(set);
  return R_UnwindProtect(r_check_fill, &ctx, r_check_cleanup, &ctx, NULL);
}

static void r_eval_cleanup(void *data, Rboolean jump) {
  (void)jump;
  RCheckCtx *c = (RCheckCtx *)data;
  if (c->val)
    active_value_free(c->val);
  if (c->mod)
    active_module_free(c->mod);
  c->val = NULL;
  c->mod = NULL;
}

static SEXP r_eval_fill(void *data) {
  RCheckCtx *c = (RCheckCtx *)data;
  /* Copy the owned string into R-managed memory first, then free the
     original immediately: R_alloc memory is reclaimed automatically on
     return and on longjmp, so a later OOM in allocVector/mkChar leaks
     nothing. */
  char *json = active_value_to_json(c->val);
  size_t len = strlen(json ? json : "");
  char *buf = (char *)R_alloc(len + 1, sizeof(char));
  memcpy(buf, json ? json : "", len + 1);
  active_string_free(json);
  SEXP out = PROTECT(allocVector(STRSXP, 1));
  SET_STRING_ELT(out, 0, mkChar(buf));
  UNPROTECT(1);
  return out;
}

SEXP r_eval_json(SEXP src_) {
  const char *src = CHAR(STRING_ELT(src_, 0));
  ActiveModule *m = r_parse_or_null(src);
  if (!m)
    return R_NilValue;
  ActiveValue *v = active_eval(m, NULL);
  if (!v) {
    /* Refusals surface as a single NA, not null: a null eval means the
       document did not evaluate, which callers must not mistake for a
       missing module. The module is freed before allocating, so an OOM
       longjmp leaks nothing. */
    active_module_free(m);
    SEXP na = PROTECT(allocVector(STRSXP, 1));
    SET_STRING_ELT(na, 0, NA_STRING);
    UNPROTECT(1);
    return na;
  }
  RCheckCtx ctx;
  ctx.mod = m;
  ctx.set = NULL;
  ctx.val = v;
  ctx.n = 0;
  return R_UnwindProtect(r_eval_fill, &ctx, r_eval_cleanup, &ctx, NULL);
}
