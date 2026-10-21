# Fortran Binding SPEC (CHECKLIST 4.001 stretch, PROPOSAL section 2.1)

Status: smoke-proven in a Linux container (gfortran 13 against the
Linux static library); run-smoke.py reproduces the proof, skipping
without gfortran or a native static library.

Mechanism: ISO_C_BINDING interfaces with `bind(C)` — the cleanest FFI
in the matrix, since the frozen C ABI maps one to one onto
interoperable Fortran declarations. Opaque handles stay `TYPE(C_PTR)`
and are never dereferenced; strings cross as explicit byte arrays with
`C_NULL_CHAR` termination (Fortran has no `\n` escapes in literals, so
`new_line("a")` builds line endings).

Agent-loop surface mirrors bindings/python/active.py one to one: check
returns a code list, eval returns the redacted document string or stops
the image on refusal, secrets never print.
