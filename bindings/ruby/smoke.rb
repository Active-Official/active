# Ruby smoke binding (CHECKLIST 2.005, PROPOSAL section 2.1).
# Fiddle over the frozen C ABI: opaque pointers, ABI major gate,
# Allocator Model A (every owned pointer freed; ensure blocks keep
# frees on the happy path, aborts end the process so nothing leaks
# mid-run). Run: ruby bindings/ruby/smoke.rb /path/to/libactive_ffi.so
require "fiddle"
require "fiddle/import"

LIB = ARGV[0] or abort("usage: smoke.rb <libactive_ffi.so>")
$lib = Fiddle.dlopen(LIB)
$libc = Fiddle.dlopen(nil)
$strlen = Fiddle::Function.new($libc["strlen"], [Fiddle::TYPE_VOIDP], Fiddle::TYPE_SIZE_T)

def cstr(ptr)
  return nil if ptr.nil? || ptr.to_i == 0
  ptr.to_s($strlen.call(ptr))
end

def call(name, args, ret)
  Fiddle::Function.new($lib[name], args, ret)
end

ABIVER = call("active_abi_version", [], Fiddle::TYPE_UINT)
MODPARSE = call("active_module_parse", [Fiddle::TYPE_VOIDP, Fiddle::TYPE_UINTPTR_T, Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOIDP)
MODFREE = call("active_module_free", [Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOID)
CHECK = call("active_check", [Fiddle::TYPE_VOIDP, Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOIDP)
LEN = call("active_diagnostics_len", [Fiddle::TYPE_VOIDP], Fiddle::TYPE_UINTPTR_T)
CODE = call("active_diagnostic_code", [Fiddle::TYPE_VOIDP, Fiddle::TYPE_UINTPTR_T], Fiddle::TYPE_VOIDP)
SETFREE = call("active_diagnostics_free", [Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOID)
EVAL = call("active_eval", [Fiddle::TYPE_VOIDP, Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOIDP)
VALFREE = call("active_value_free", [Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOID)
TOJSON = call("active_value_to_json", [Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOIDP)
STRFREE = call("active_string_free", [Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOID)
NULL = Fiddle::NULL

def is_null(p)
  p.nil? || p.to_i == 0
end

def check(cond, msg)
  unless cond
    $stderr.puts "SMOKE-FAIL: #{msg}"
    exit 1
  end
end

raise "ABI major mismatch" unless ABIVER.call() == 1
puts "abi: 1 ok"

def with_module(src)
  mod = MODPARSE.call(src, src.bytesize, NULL)
  raise "parse returned null" if is_null(mod)
  begin
    yield mod
  ensure
    MODFREE.call(mod)
  end
end

with_module("port = 8080\n") do |mod|
  set = CHECK.call(mod, NULL)
  check(!is_null(set), "check returned null")
  begin
    check(LEN.call(set) == 0, "expected clean")
  ensure
    SETFREE.call(set)
  end
end
puts "check clean ok"

with_module("port = 8080\n") do |mod|
  val = EVAL.call(mod, NULL)
  check(!is_null(val), "eval returned null")
  begin
    jp = TOJSON.call(val)
    check(!is_null(jp), "to_json returned null")
    begin
      json = cstr(jp)
      check(!json.nil? && json.include?("8080"), "value missing: #{json}")
    ensure
      STRFREE.call(jp)
    end
  ensure
    VALFREE.call(val)
  end
end
puts "eval port=8080 ok"

with_module("timeout = 5s + 512MiB\n") do |mod|
  val = EVAL.call(mod, NULL)
  check(is_null(val), "units mixed evaluated clean")
  set = CHECK.call(mod, NULL)
  check(!is_null(set), "check returned null")
  begin
    check(LEN.call(set) > 0, "expected diagnostics")
    code = cstr(CODE.call(set, 0))
    check(code == "T001", "expected T001, got #{code}")
  ensure
    SETFREE.call(set)
  end
end
puts "units refused ok"

with_module("token = secret(\"hunter2\")\n") do |mod|
  val = EVAL.call(mod, NULL)
  check(!is_null(val), "eval returned null")
  begin
    jp = TOJSON.call(val)
    check(!is_null(jp), "to_json returned null")
    begin
      json = cstr(jp)
      check(!json.nil? && json.include?("***SECRET***") && !json.include?("hunter2"), "redaction broken: #{json}")
    ensure
      STRFREE.call(jp)
    end
  ensure
    VALFREE.call(val)
  end
end
puts "secret redacted ok"
puts "SMOKE-OK"
