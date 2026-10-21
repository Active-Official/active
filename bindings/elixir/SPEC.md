# Elixir Binding SPEC (CHECKLIST Phase 2, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (Elixir 1.14, NIF resource type with destructor against the Linux staticlib); run-smoke.py reproduces the proof, skipping without elixir/gcc/erlang-dev.

Mechanism: raw Erlang NIFs in C (no Rustler dependency).

A minimal NIF library (`active_nif.c`) owning module handles as a resource type with a destructor that calls `active_module_free`; ABI gate at load failing fast; check returns a code list, eval returns a binary or `:refused`, parse errors return `:parse_error`. Owned JSON is copied to a binary then freed before return; borrowed codes are copied and never freed.

Ownership mapping: every active_* owned return is freed exactly once with its matching free function; diagnostic codes are borrowed and never freed; modules stay thread-confined per docs/FFI.md. Agent-loop surface (check, eval, diagnostics with codes) mirrors bindings/python/active.py one to one. Harness path `elixir` runs the smoke through run-smoke.py; SKIPs openly without the toolchain.
