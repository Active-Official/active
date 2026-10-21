# Ruby Binding — Fiddle smoke over the frozen C ABI (CHECKLIST 2.005)

`smoke.rb` drives `libactive_ffi` through Ruby Fiddle: opaque handles as
integers (explicit zero-checks, since Fiddle returns Integer 0, not nil,
for NULL), ABI major gate at load, Allocator Model A with `ensure`
blocks on every path. Run from the repo root:

```bash
ruby bindings/ruby/smoke.rb path/to/libactive_ffi.so
```

Proven in a Linux container (Ruby Fiddle, Linux shared library):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` wraps the call for the shared harness and
skips cleanly without Ruby or a native shared library.
