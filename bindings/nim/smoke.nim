# Nim smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
# importc over the frozen C ABI (header + passL staticlib on the build
# command): opaque handles stay `pointer`, every owned return is freed
# exactly once, borrowed code strings are copied with `$` and never freed.
# Build: nim c -d:release --passC:-I<active-ffi> --passL:<libactive_ffi.a> -o:smoke smoke.nim

import std/[os, strutils]

proc active_abi_version(): uint32 {.importc, header: "active.h".}
proc active_module_parse(src: cstring, len: csize_t, opts: pointer): pointer {.importc, header: "active.h".}
proc active_module_free(h: pointer) {.importc, header: "active.h".}
proc active_check(m: pointer, budget: pointer): pointer {.importc, header: "active.h".}
proc active_eval(m: pointer, budget: pointer): pointer {.importc, header: "active.h".}
proc active_diagnostics_len(s: pointer): csize_t {.importc, header: "active.h".}
proc active_diagnostic_code(s: pointer, i: csize_t): cstring {.importc, header: "active.h".}
proc active_diagnostics_free(s: pointer) {.importc, header: "active.h".}
proc active_value_free(v: pointer) {.importc, header: "active.h".}
proc active_value_to_json(v: pointer): cstring {.importc, header: "active.h".}
proc active_string_free(s: cstring) {.importc, header: "active.h".}

proc check(cond: bool, msg: string) =
  if not cond:
    stderr.writeLine("SMOKE-FAIL: " & msg)
    quit(1)

template withModule(src: string, body: untyped) =
  block:
    let m {.inject.} = active_module_parse(src.cstring, src.len.csize_t, nil)
    check(not m.isNil, "parse returned null")
    try:
      body
    finally:
      active_module_free(m)

proc ownedJson(v: pointer): string =
  let p = active_value_to_json(v)
  check(not p.isNil, "to_json returned null")
  result = $p
  active_string_free(p)

check(active_abi_version() == 1, "ABI major mismatch")
echo "abi: 1 ok"

withModule("port = 8080\n"):
  let s = active_check(m, nil)
  check(not s.isNil, "check returned null")
  let n = active_diagnostics_len(s)
  active_diagnostics_free(s)
  check(n == 0, "expected clean")
echo "check clean ok"

withModule("port = 8080\n"):
  let v = active_eval(m, nil)
  check(not v.isNil, "eval returned null")
  let json = ownedJson(v)
  active_value_free(v)
  check("8080" in json, "value missing: " & json)
echo "eval port=8080 ok"

withModule("timeout = 5s + 512MiB\n"):
  let v = active_eval(m, nil)
  check(v.isNil, "units mixed evaluated clean")
  let s = active_check(m, nil)
  check(not s.isNil, "check returned null")
  let n = active_diagnostics_len(s)
  var code = ""
  if n > 0: code = $active_diagnostic_code(s, 0)
  active_diagnostics_free(s)
  check(n > 0, "expected diagnostics")
  check(code == "T001", "expected T001, got " & code)
echo "units refused ok"

withModule("token = secret(\"hunter2\")\n"):
  let v = active_eval(m, nil)
  check(not v.isNil, "eval returned null")
  let json = ownedJson(v)
  active_value_free(v)
  check("***SECRET***" in json and "hunter2" notin json, "redaction broken: " & json)
echo "secret redacted ok"
echo "SMOKE-OK"
