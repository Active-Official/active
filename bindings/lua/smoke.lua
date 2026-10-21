-- Lua smoke binding (CHECKLIST 2.016, PROPOSAL section 2.1).
-- LuaJIT FFI over the frozen C ABI: opaque handles as cdata, ABI major
-- gate at load, Allocator Model A (every owned pointer freed exactly
-- once; ffi.gc finalizers as backstop). Run from the repo root:
--   luajit bindings/lua/smoke.lua /path/to/libactive_ffi.so <wasm-or-nil>
-- Prints SMOKE-OK lines, exit 0 on success.
local ffi = require("ffi")

ffi.cdef([[
typedef struct ActiveModule ActiveModule;
typedef struct ActiveDiagnosticSet ActiveDiagnosticSet;
typedef struct ActiveValue ActiveValue;
typedef struct ActiveBudget {
    uint32_t struct_size;
    uint64_t max_nodes;
    uint64_t max_depth;
    uint64_t max_array_len;
    uint64_t max_keys;
    uint64_t max_wall_ms;
    uint64_t max_value_size;
    uint64_t max_total_value_size;
} ActiveBudget;
uint32_t active_abi_version(void);
ActiveModule* active_module_parse(const uint8_t* source, uintptr_t len, const void* options);
void active_module_free(ActiveModule* handle);
ActiveDiagnosticSet* active_check(const ActiveModule* module, const ActiveBudget* budget);
uintptr_t active_diagnostics_len(const ActiveDiagnosticSet* set);
const char* active_diagnostic_code(const ActiveDiagnosticSet* set, uintptr_t index);
void active_diagnostics_free(ActiveDiagnosticSet* set);
ActiveValue* active_eval(const ActiveModule* module, const ActiveBudget* budget);
char* active_value_to_json(const ActiveValue* value);
void active_value_free(ActiveValue* value);
void active_string_free(char* s);
]])

local libpath = arg[1] or error("usage: smoke.lua <libactive_ffi.so>");
local lib = ffi.load(libpath)
assert(lib.active_abi_version() == 1, "ABI major mismatch")
print("abi: 1 ok")

-- Owned handles are wrapped in ffi.gc immediately so an assert failure
-- anywhere below still releases them (smoke exits, but library users
-- copying this pattern stay leak-free on every path).
local function owned(ptr, free)
    assert(ptr ~= nil, "parse returned null")
    return ffi.gc(ptr, free)
end

local function check_clean(src)
    local mod = owned(lib.active_module_parse(src, #src, nil), lib.active_module_free)
    local set = owned(lib.active_check(mod, nil), lib.active_diagnostics_free)
    return tonumber(lib.active_diagnostics_len(set)) == 0
end

local function eval_json(src)
    local mod = owned(lib.active_module_parse(src, #src, nil), lib.active_module_free)
    local raw = lib.active_eval(mod, nil)
    if raw == nil then
        return nil
    end
    local val = ffi.gc(raw, lib.active_value_free)
    local s = lib.active_value_to_json(val)
    if s == nil then
        return nil
    end
    return ffi.string(ffi.gc(s, lib.active_string_free))
end

assert(check_clean("port = 8080\n"), "expected clean")
print("check clean ok")
local json = assert(eval_json("port = 8080\n"), "eval returned null")
assert(json:find("8080", 1, true) ~= nil, "value missing 8080: " .. json)
print("eval port=8080 ok")
assert(eval_json("timeout = 5s + 512MiB\n") == nil, "units mixed evaluated clean")
print("units refused ok")
local sj = assert(eval_json('token = secret("hunter2")\n'), "eval returned null")
assert(sj:find("***SECRET***", 1, true) ~= nil, "redaction missing")
assert(sj:find("hunter2", 1, true) == nil, "raw secret leaked: " .. sj)
print("secret redacted ok")

-- T001 code check through the diagnostics path.
local function first_code(src)
    local mod = owned(lib.active_module_parse(src, #src, nil), lib.active_module_free)
    local set = owned(lib.active_check(mod, nil), lib.active_diagnostics_free)
    local n = tonumber(lib.active_diagnostics_len(set))
    local code = nil
    if n > 0 then
        local c = lib.active_diagnostic_code(set, 0)
        if c ~= nil then code = ffi.string(c) end
    end
    return code
end
assert(first_code("timeout = 5s + 512MiB\n") == "T001", "expected T001")
print("units code ok")
print("SMOKE-OK")
