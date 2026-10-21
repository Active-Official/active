# PHP Binding — ext-ffi smoke over the frozen C ABI (CHECKLIST 2.006)

`smoke.php` drives `libactive_ffi` through PHP 8+ FFI (run with
`php -d ffi.enable=1`): opaque handles as `FFI\CData`, ABI major gate at
load, Allocator Model A with try/finally on every path. Input strings
cross as explicit `uint8_t[]` buffers via `FFI::memcpy`; output strings
are read with explicit lengths (no `FFI::string` before PHP 8.4). Run:

```bash
php -d ffi.enable=1 bindings/php/smoke.php path/to/libactive_ffi.so
```

Proven in a Linux container (PHP 8.3, ext-ffi, Linux shared library):
ABI gate, clean check, eval value, T001 refusal, redaction with zero raw
leakage. `run-smoke.py` wraps the call for the shared harness and skips
cleanly without PHP or a native shared library.
