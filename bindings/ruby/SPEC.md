# Ruby Binding SPEC (CHECKLIST 2.005, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (Ruby Fiddle over the Linux
shared library); run-smoke.py reproduces the proof, skipping without
Ruby or a native shared library.

Mechanism: Fiddle. Opaque handles as integers, ABI gate at load,
Allocator Model A (owned pointers freed exactly once; `ensure` blocks
keep frees on the happy path, aborts end the process). Note: Fiddle
returns Integer 0 (not nil) for NULL pointers; always test truthiness
with an explicit zero check.

Agent-loop surface mirrors bindings/python/active.py one to one: check
returns diagnostic codes, eval returns the redacted value or nil on
refusal, secrets never leak.
