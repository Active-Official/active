// Zig smoke binding (CHECKLIST 2.003, PROPOSAL section 2).
// Drives the frozen C ABI directly: opaque handles, ABI major gate,
// Allocator Model A (every owned pointer freed exactly once).
// Build from the repo root:
//   zig build-exe bindings/zig/smoke.zig -I active-ffi -L target/debug -l active_ffi -lc
// Run: ./smoke (prints SMOKE-OK lines, exit 0 on success).
const std = @import("std");
const c = @cImport({
    @cInclude("active.h");
});

fn checkClean(source: []const u8) !void {
    const mod = c.active_module_parse(source.ptr, source.len, null) orelse {
        std.debug.print("SMOKE-FAIL: parse returned null\n", .{});
        return error.ParseNull;
    };
    defer c.active_module_free(mod);
    const set = c.active_check(mod, null) orelse {
        std.debug.print("SMOKE-FAIL: check returned null\n", .{});
        return error.CheckNull;
    };
    defer c.active_diagnostics_free(set);
    const n = c.active_diagnostics_len(set);
    if (n != 0) {
        const code = c.active_diagnostic_code(set, 0);
        std.debug.print("SMOKE-FAIL: expected clean, got {d}: {s}\n", .{
            n,
            if (code != null) std.mem.span(code) else "?",
        });
        return error.NotClean;
    }
}

fn evalJson(source: []const u8) ![]u8 {
    const mod = c.active_module_parse(source.ptr, source.len, null) orelse
        return error.ParseNull;
    defer c.active_module_free(mod);
    const val = c.active_eval(mod, null) orelse return error.EvalNull;
    defer c.active_value_free(val);
    const s = c.active_value_to_json(val) orelse return error.JsonNull;
    defer c.active_string_free(s);
    return try std.heap.page_allocator.dupe(u8, std.mem.span(s));
}

pub fn main() !void {
    if (c.active_abi_version() != c.ACTIVE_ABI_VERSION) {
        std.debug.print("SMOKE-FAIL: ABI major mismatch\n", .{});
        return error.AbiMismatch;
    }
    std.debug.print("abi: 1 ok\n", .{});
    try checkClean("port = 8080\n");
    std.debug.print("check clean ok\n", .{});
    const json = try evalJson("port = 8080\n");
    defer std.heap.page_allocator.free(json);
    if (std.mem.indexOf(u8, json, "8080") == null) {
        std.debug.print("SMOKE-FAIL: value missing: {s}\n", .{json});
        return error.BadValue;
    }
    std.debug.print("eval port=8080 ok\n", .{});
    // Units refusal: eval returns null, check carries T001.
    {
        const src = "timeout = 5s + 512MiB\n";
        const mod = c.active_module_parse(src.ptr, src.len, null) orelse
            return error.ParseNull;
        defer c.active_module_free(mod);
        const val = c.active_eval(mod, null);
        if (val != null) {
            c.active_value_free(val);
            std.debug.print("SMOKE-FAIL: units mixed evaluated clean\n", .{});
            return error.UnitsClean;
        }
        const set = c.active_check(mod, null) orelse return error.CheckNull;
        defer c.active_diagnostics_free(set);
        const code = c.active_diagnostic_code(set, 0);
        const code_str: []const u8 = if (code != null) std.mem.span(code) else "";
        if (!std.mem.eql(u8, code_str, "T001")) {
            std.debug.print("SMOKE-FAIL: expected T001, got {s}\n", .{code_str});
            return error.BadCode;
        }
    }
    std.debug.print("units refused ok\n", .{});
    // Secret redaction: value carries the marker, never the raw secret.
    {
        const sj = try evalJson("token = secret(\"hunter2\")\n");
        defer std.heap.page_allocator.free(sj);
        if (std.mem.indexOf(u8, sj, "***SECRET***") == null or
            std.mem.indexOf(u8, sj, "hunter2") != null)
        {
            std.debug.print("SMOKE-FAIL: redaction broken: {s}\n", .{sj});
            return error.RedactFail;
        }
    }
    std.debug.print("secret redacted ok\n", .{});
    std.debug.print("SMOKE-OK\n", .{});
}
