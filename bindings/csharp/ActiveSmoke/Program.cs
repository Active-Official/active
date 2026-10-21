// C# smoke binding (CHECKLIST 2.002, PROPOSAL section 2).
// Drives the frozen C ABI via P/Invoke: opaque IntPtr handles, ABI major
// gate, Allocator Model A (every owned pointer freed exactly once).
// Build and run from the repo root:
//   dotnet run --project bindings/csharp/ActiveSmoke (needs active_ffi.dll
//   beside the project output; copy from target/debug after
//   `cargo build -p active-ffi`).
using System;
using System.Runtime.InteropServices;
using System.Text;

static class Native
{
    const string Lib = "active_ffi";
    [DllImport(Lib, EntryPoint = "active_abi_version")] internal static extern uint ActiveAbiVersion();
    [DllImport(Lib, EntryPoint = "active_module_parse")] internal static extern IntPtr ActiveModuleParse(byte[] src, UIntPtr len, IntPtr opts);
    [DllImport(Lib, EntryPoint = "active_module_free")] internal static extern void ActiveModuleFree(IntPtr h);
    [DllImport(Lib, EntryPoint = "active_check")] internal static extern IntPtr ActiveCheck(IntPtr mod, IntPtr budget);
    [DllImport(Lib, EntryPoint = "active_diagnostics_len")] internal static extern UIntPtr ActiveDiagnosticsLen(IntPtr set);
    [DllImport(Lib, EntryPoint = "active_diagnostic_code")] internal static extern IntPtr ActiveDiagnosticCode(IntPtr set, UIntPtr i);
    [DllImport(Lib, EntryPoint = "active_diagnostics_free")] internal static extern void ActiveDiagnosticsFree(IntPtr set);
    [DllImport(Lib, EntryPoint = "active_eval")] internal static extern IntPtr ActiveEval(IntPtr mod, IntPtr budget);
    [DllImport(Lib, EntryPoint = "active_value_free")] internal static extern void ActiveValueFree(IntPtr v);
    [DllImport(Lib, EntryPoint = "active_value_to_json")] internal static extern IntPtr ActiveValueToJson(IntPtr v);
    [DllImport(Lib, EntryPoint = "active_string_free")] internal static extern void ActiveStringFree(IntPtr s);
}

static class Smoke
{
    static void Check(bool cond, string what)
    {
        if (!cond) { Console.Error.WriteLine("SMOKE-FAIL: " + what); Environment.Exit(1); }
    }

    static string PtrToString(IntPtr p) => Marshal.PtrToStringUTF8(p) ?? "";

    static int Main()
    {
        Check(Native.ActiveAbiVersion() == 1, "ABI major mismatch");
        Console.WriteLine("abi: 1 ok");

        void WithModule(string src, Action<IntPtr> body)
        {
            var bytes = Encoding.UTF8.GetBytes(src);
            var mod = Native.ActiveModuleParse(bytes, (UIntPtr)bytes.Length, IntPtr.Zero);
            Check(mod != IntPtr.Zero, "parse returned null");
            try { body(mod); }
            finally { Native.ActiveModuleFree(mod); }
        }

        WithModule("port = 8080\n", mod =>
        {
            var set = Native.ActiveCheck(mod, IntPtr.Zero);
            Check(set != IntPtr.Zero, "check returned null");
            try
            {
                Check(Native.ActiveDiagnosticsLen(set) == UIntPtr.Zero, "expected clean");
            }
            finally { Native.ActiveDiagnosticsFree(set); }
        });
        Console.WriteLine("check clean ok");

        WithModule("port = 8080\n", mod =>
        {
            var val = Native.ActiveEval(mod, IntPtr.Zero);
            Check(val != IntPtr.Zero, "eval returned null");
            try
            {
                var json = Native.ActiveValueToJson(val);
                Check(json != IntPtr.Zero, "to_json returned null");
                try
                {
                    var s = PtrToString(json);
                    Check(s.Contains("8080"), "value missing: " + s);
                }
                finally { Native.ActiveStringFree(json); }
            }
            finally { Native.ActiveValueFree(val); }
        });
        Console.WriteLine("eval port=8080 ok");

        WithModule("timeout = 5s + 512MiB\n", mod =>
        {
            var val = Native.ActiveEval(mod, IntPtr.Zero);
            Check(val == IntPtr.Zero, "units mixed evaluated clean");
            var set = Native.ActiveCheck(mod, IntPtr.Zero);
            Check(set != IntPtr.Zero, "check returned null");
            try
            {
                Check(Native.ActiveDiagnosticsLen(set) != UIntPtr.Zero, "expected diagnostics");
                var code = PtrToString(Native.ActiveDiagnosticCode(set, 0));
                Check(code == "T001", "expected T001, got " + code);
            }
            finally { Native.ActiveDiagnosticsFree(set); }
        });
        Console.WriteLine("units refused ok");

        WithModule("token = secret(\"hunter2\")\n", mod =>
        {
            var val = Native.ActiveEval(mod, IntPtr.Zero);
            Check(val != IntPtr.Zero, "eval returned null");
            try
            {
                var json = Native.ActiveValueToJson(val);
                Check(json != IntPtr.Zero, "to_json returned null");
                try
                {
                    var s = PtrToString(json);
                    Check(s.Contains("***SECRET***") && !s.Contains("hunter2"), "redaction broken: " + s);
                }
                finally { Native.ActiveStringFree(json); }
            }
            finally { Native.ActiveValueFree(val); }
        });
        Console.WriteLine("secret redacted ok");
        Console.WriteLine("SMOKE-OK");
        return 0;
    }
}
