# C# Binding — Smoke over the frozen C ABI (CHECKLIST 2.002)

`ActiveSmoke/Program.cs` drives `active-ffi` via P/Invoke: opaque
`IntPtr` handles, ABI major gate at startup, Allocator Model A (every
owned pointer freed exactly once, `try/finally` on every path).

## Build and run

From the repo root (needs the `active-ffi` cdylib built and .NET 8+):

```bash
cargo build -p active-ffi
python bindings/csharp/run-smoke.py   # builds, stages the DLL, runs, checks SMOKE-OK
```

No NuGet packages are used, so the build works fully offline. `run-smoke.py`
copies `target/debug/active_ffi.dll` beside the project output before
running; the harness path `csharp` calls this script.
