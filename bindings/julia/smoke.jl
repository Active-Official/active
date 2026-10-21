# Julia smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
# `ccall` over the frozen C ABI: opaque handles stay Ptr{Cvoid}, every
# owned return is freed exactly once, borrowed code strings are copied
# with unsafe_string and never freed. Run: julia smoke.jl <libactive_ffi>
lib = ARGS[1]

function fail(msg)
    println(stderr, "SMOKE-FAIL: ", msg)
    exit(1)
end

abi = ccall((:active_abi_version, lib), UInt32, ())
abi == 1 || fail("ABI major mismatch")
println("abi: 1 ok")

function with_module(f, src)
    mod = ccall((:active_module_parse, lib), Ptr{Cvoid},
                (Cstring, Csize_t, Ptr{Cvoid}), src, sizeof(src), C_NULL)
    mod == C_NULL && fail("parse returned null")
    try
        f(mod)
    finally
        ccall((:active_module_free, lib), Cvoid, (Ptr{Cvoid},), mod)
    end
end

function owned_json(val)
    p = ccall((:active_value_to_json, lib), Ptr{UInt8}, (Ptr{Cvoid},), val)
    p == C_NULL && fail("to_json returned null")
    s = unsafe_string(p)
    ccall((:active_string_free, lib), Cvoid, (Ptr{UInt8},), p)
    return s
end

function codes(set)
    n = ccall((:active_diagnostics_len, lib), Csize_t, (Ptr{Cvoid},), set)
    out = String[]
    for i in 0:(n - 1)
        p = ccall((:active_diagnostic_code, lib), Ptr{UInt8},
                  (Ptr{Cvoid}, Csize_t), set, i)
        push!(out, p == C_NULL ? "" : unsafe_string(p))
    end
    return out
end

with_module("port = 8080\n") do m
    set = ccall((:active_check, lib), Ptr{Cvoid}, (Ptr{Cvoid}, Ptr{Cvoid}), m, C_NULL)
    set == C_NULL && fail("check returned null")
    n = ccall((:active_diagnostics_len, lib), Csize_t, (Ptr{Cvoid},), set)
    ccall((:active_diagnostics_free, lib), Cvoid, (Ptr{Cvoid},), set)
    n == 0 || fail("expected clean")
end
println("check clean ok")

with_module("port = 8080\n") do m
    val = ccall((:active_eval, lib), Ptr{Cvoid}, (Ptr{Cvoid}, Ptr{Cvoid}), m, C_NULL)
    val == C_NULL && fail("eval returned null")
    json = owned_json(val)
    ccall((:active_value_free, lib), Cvoid, (Ptr{Cvoid},), val)
    occursin("8080", json) || fail("value missing: $json")
end
println("eval port=8080 ok")

with_module("timeout = 5s + 512MiB\n") do m
    val = ccall((:active_eval, lib), Ptr{Cvoid}, (Ptr{Cvoid}, Ptr{Cvoid}), m, C_NULL)
    val == C_NULL || fail("units mixed evaluated clean")
    set = ccall((:active_check, lib), Ptr{Cvoid}, (Ptr{Cvoid}, Ptr{Cvoid}), m, C_NULL)
    set == C_NULL && fail("check returned null")
    cs = codes(set)
    ccall((:active_diagnostics_free, lib), Cvoid, (Ptr{Cvoid},), set)
    (length(cs) > 0 && cs[1] == "T001") || fail("expected T001, got $cs")
end
println("units refused ok")

with_module("token = secret(\"hunter2\")\n") do m
    val = ccall((:active_eval, lib), Ptr{Cvoid}, (Ptr{Cvoid}, Ptr{Cvoid}), m, C_NULL)
    val == C_NULL && fail("eval returned null")
    json = owned_json(val)
    ccall((:active_value_free, lib), Cvoid, (Ptr{Cvoid},), val)
    (occursin("***SECRET***", json) && !occursin("hunter2", json)) ||
        fail("redaction broken: $json")
end
println("secret redacted ok")
println("SMOKE-OK")
