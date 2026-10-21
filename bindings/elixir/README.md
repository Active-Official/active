# Elixir Binding — NIF smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`active_nif.c` wraps the frozen C ABI in Erlang NIFs and `smoke.exs` asserts the contract: module handles live in a resource type whose destructor calls `active_module_free` (freed exactly once even if the owner dies), ABI major gate at load, owned JSON copied to a binary then freed before return, borrowed codes copied and never freed. Build and run from the repo root:

```bash
gcc -shared -fPIC -I$(elixir -e "IO.puts :code.lib_dir(:erts)")/include -Iactive-ffi bindings/elixir/active_nif.c -o active_nif.so path/to/libactive_ffi.a
ACTIVE_NIF_SO=$PWD/active_nif elixir bindings/elixir/smoke.exs
```

(`ACTIVE_NIF_SO` is the path without `.so`; `:erlang.load_nif` appends the extension itself.)

Proven in a Linux container (Elixir 1.14 on OTP 24, Linux staticlib):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` wraps the call for the shared harness and
skips cleanly without elixir, gcc, or Erlang NIF headers.
