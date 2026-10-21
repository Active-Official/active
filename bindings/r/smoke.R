# R smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
# A small C wrapper (r_wrap.c, compiled with R CMD SHLIB and dyn.load-ed)
# exposes the frozen C ABI through .Call: codes come back as a character
# vector (empty means clean), eval as a JSON string or NA on refusal.
# Ownership stays Model A inside the wrapper; R only sees copies.
# Run: Rscript smoke.R (run-smoke.py builds the wrapper first)
args <- commandArgs(trailingOnly = TRUE)
stopifnot(length(args) == 1L)
dyn.load(args[1])

fail <- function(msg) { cat(paste0("SMOKE-FAIL: ", msg, "\n"), file = stderr()); quit(status = 1) }

abi <- .Call("r_abi_version")
if (abi != 1L) fail("ABI major mismatch")
cat("abi: 1 ok\n")

codes <- .Call("r_check_codes", "port = 8080\n")
if (is.null(codes)) fail("parse returned null")
if (length(codes) != 0L) fail("expected clean")
cat("check clean ok\n")

json <- .Call("r_eval_json", "port = 8080\n")
if (is.null(json) || is.na(json)) fail("eval returned null")
if (!grepl("8080", json, fixed = TRUE)) fail(paste0("value missing: ", json))
cat("eval port=8080 ok\n")

json <- .Call("r_eval_json", "timeout = 5s + 512MiB\n")
if (!is.na(json)) fail("units mixed evaluated clean")
codes <- .Call("r_check_codes", "timeout = 5s + 512MiB\n")
if (is.null(codes) || length(codes) == 0L) fail("expected diagnostics")
if (codes[1] != "T001") fail(paste0("expected T001, got ", codes[1]))
cat("units refused ok\n")

json <- .Call("r_eval_json", "token = secret(\"hunter2\")\n")
if (is.null(json) || is.na(json)) fail("eval returned null")
if (!grepl("***SECRET***", json, fixed = TRUE) || grepl("hunter2", json, fixed = TRUE))
  fail(paste0("redaction broken: ", json))
cat("secret redacted ok\n")
cat("SMOKE-OK\n")
