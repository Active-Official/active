# Crystal smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
# `lib`/`fun` declarations over the frozen C ABI (staticlib linked via
# ldflags on the build command): opaque handles stay Void*, every owned
# return is freed exactly once, borrowed code strings are copied with
# String.new and never freed.
# Build: crystal build --release --link-flags <libactive_ffi.a> -o smoke smoke.cr
lib Active
  fun active_abi_version : UInt32
  fun active_module_parse(src : UInt8*, len : LibC::SizeT, opts : Void*) : Void*
  fun active_module_free(handle : Void*) : Void
  fun active_check(mod : Void*, budget : Void*) : Void*
  fun active_eval(mod : Void*, budget : Void*) : Void*
  fun active_diagnostics_len(set : Void*) : LibC::SizeT
  fun active_diagnostic_code(set : Void*, index : LibC::SizeT) : UInt8*
  fun active_diagnostics_free(set : Void*) : Void
  fun active_value_free(value : Void*) : Void
  fun active_value_to_json(value : Void*) : UInt8*
  fun active_string_free(s : UInt8*) : Void
end

def check(cond : Bool, msg : String)
  unless cond
    STDERR.puts "SMOKE-FAIL: #{msg}"
    exit 1
  end
end

def with_module(src : String, & : Pointer(Void) ->)
  mod = Active.active_module_parse(src.to_unsafe.as(Pointer(UInt8)), src.bytesize, Pointer(Void).null)
  check(!mod.null?, "parse returned null")
  begin
    yield mod
  ensure
    Active.active_module_free(mod)
  end
end

def owned_json(val : Pointer(Void)) : String
  p = Active.active_value_to_json(val)
  check(!p.null?, "to_json returned null")
  json = String.new(p)
  Active.active_string_free(p)
  json
end

check(Active.active_abi_version == 1, "ABI major mismatch")
puts "abi: 1 ok"

with_module("port = 8080\n") do |m|
  set = Active.active_check(m, Pointer(Void).null)
  check(!set.null?, "check returned null")
  n = Active.active_diagnostics_len(set)
  Active.active_diagnostics_free(set)
  check(n == 0, "expected clean")
end
puts "check clean ok"

with_module("port = 8080\n") do |m|
  val = Active.active_eval(m, Pointer(Void).null)
  check(!val.null?, "eval returned null")
  json = owned_json(val)
  Active.active_value_free(val)
  check(json.includes?("8080"), "value missing: #{json}")
end
puts "eval port=8080 ok"

with_module("timeout = 5s + 512MiB\n") do |m|
  val = Active.active_eval(m, Pointer(Void).null)
  check(val.null?, "units mixed evaluated clean")
  set = Active.active_check(m, Pointer(Void).null)
  check(!set.null?, "check returned null")
  n = Active.active_diagnostics_len(set)
  code = n > 0 ? String.new(Active.active_diagnostic_code(set, 0)) : ""
  Active.active_diagnostics_free(set)
  check(n > 0, "expected diagnostics")
  check(code == "T001", "expected T001, got #{code}")
end
puts "units refused ok"

with_module("token = secret(\"hunter2\")\n") do |m|
  val = Active.active_eval(m, Pointer(Void).null)
  check(!val.null?, "eval returned null")
  json = owned_json(val)
  Active.active_value_free(val)
  check(json.includes?("***SECRET***") && !json.includes?("hunter2"), "redaction broken: #{json}")
end
puts "secret redacted ok"
puts "SMOKE-OK"
