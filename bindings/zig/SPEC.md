# Zig Binding SPEC (CHECKLIST 2.003, PROPOSAL section 2.1)

Status: smoke-proven Tier-2 candidate. The smoke program exercises the
full agent-relevant surface (parse, check with codes, eval with JSON,
secret redaction, refusal paths) against the frozen ABI v1 with
single-ownership discipline.

Ownership mapping: every `active_*` owned return is freed exactly once
with its matching free function (`active_module_free`,
`active_diagnostics_free`, `active_value_free`, `active_string_free`);
diagnostic codes are borrowed and never freed. Modules are confined to
the calling thread per docs/FFI.md.

Conformance path: the Zig program asserts the same vectors the shared
harness runs (clean eval, T001 refusal, redacted secret). Full manifest
wiring (harness `zig` path) follows the same tally protocol as the Go
smoke once the binding graduates to Tier-1 scope.
