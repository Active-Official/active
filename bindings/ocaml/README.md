# OCaml Binding — external-declaration smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`smoke.ml` plus a small C stub (`ocaml_stubs.c`) drives `libactive_ffi`: handles cross the stub boundary as nativeints so the GC never chases raw C pointers, ABI major gate at startup, Allocator Model A (owned JSON copied to an OCaml string inside the stub then freed exactly once; borrowed codes copied on return and never freed). Build and run from the repo root:

```bash
ocamlopt -o smoke -ccopt -Iactive-ffi bindings/ocaml/ocaml_stubs.c bindings/ocaml/smoke.ml path/to/libactive_ffi.a -cclib -lm -cclib -ldl -cclib -lpthread && ./smoke
```

Proven in a Linux container (OCaml 4.14.1, Linux staticlib):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` wraps the call for the shared harness and
skips cleanly without ocamlopt or a native static library.
