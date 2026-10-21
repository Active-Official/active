# R Binding SPEC (CHECKLIST Phase 2 stretch, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (R 4.3.3, small C wrapper compiled with `R CMD SHLIB` against the Linux staticlib); run-smoke.py reproduces the proof, skipping without Rscript.

Mechanism: a thin C wrapper (`r_wrap.c`) exposes the frozen C ABI through `.Call` entry points returning ordinary R values (`r_abi`, `r_check_codes`, `r_eval_json`), following the same pattern as CRAN packages that wrap C libraries. Ownership stays Model A inside the wrapper: owned JSON is copied to an R string then freed before return, borrowed codes copy on return and are never freed, every handle is freed on all paths (cleanup runs before any R error unwinds). ABI gate at load failing on mismatch. Agent-loop surface mirrors bindings/python/active.py one to one: check returns a character vector of codes (empty means clean), eval returns a JSON string or NA on refusal, units mismatches surface T001, secrets never print.

Build/run: `R CMD SHLIB r_wrap.c` with `PKG_LIBS` pointing at the Linux staticlib, then `Rscript smoke.R ./r_wrap.so`. The shared harness drives it through `bindings/r/run-smoke.py`, which builds the wrapper in a temp dir (never in the repo) and skips openly without Rscript or a native static library.
