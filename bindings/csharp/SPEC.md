# C# Binding SPEC (CHECKLIST 2.002, PROPOSAL section 2.1)

Status: smoke-proven Tier-2. The smoke program exercises the full
agent-relevant surface (parse, check with codes, eval with JSON,
secret redaction, refusal paths) against the frozen ABI v1 with
single-ownership discipline.

Entry points are bound by exact C name (`EntryPoint = "active_*"`):
P/Invoke name mangling must never silently rebind. Handles are opaque
`IntPtr`; the runtime marshals UTF-8 sources as `byte[]` with explicit
lengths (embedded NULs cannot truncate). Diagnostic codes are borrowed
and never freed; every owned handle has exactly one free call on every
path including exceptions (`try/finally` throughout).

Conformance path: the smoke asserts the same vectors the shared harness
runs (clean eval, T001 refusal, redacted secret). Full manifest wiring
(harness `csharp` path) is live.
