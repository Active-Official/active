// V smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
// C interop over the frozen C ABI: opaque handles stay voidptr, every
// owned return is freed exactly once, borrowed code strings are copied
// with cstring_to_vstring and never freed.
// Build: v -cflags -I<active-ffi> -ldflags <libactive_ffi.a> -o smoke smoke.v
// (run-smoke.py fills in the real paths; no absolute paths baked in)
#include "active.h"

fn C.active_abi_version() u32
fn C.active_module_parse(src &u8, len usize, opts voidptr) voidptr
fn C.active_module_free(h voidptr)
fn C.active_check(m voidptr, budget voidptr) voidptr
fn C.active_eval(m voidptr, budget voidptr) voidptr
fn C.active_diagnostics_len(s voidptr) usize
fn C.active_diagnostic_code(s voidptr, i usize) &u8
fn C.active_diagnostics_free(s voidptr)
fn C.active_value_free(v voidptr)
fn C.active_value_to_json(v voidptr) &u8
fn C.active_string_free(s &u8)

fn check(cond bool, msg string) {
	if !cond {
		eprintln('SMOKE-FAIL: ${msg}')
		exit(1)
	}
}

fn owned_json(v voidptr) string {
	p := C.active_value_to_json(v)
	check(p != unsafe { nil }, 'to_json returned null')
	json := unsafe { cstring_to_vstring(&char(p)) }
	C.active_string_free(p)
	return json
}

fn with_module(src string, body fn (m voidptr)) {
	m := C.active_module_parse(src.str, usize(src.len), unsafe { nil })
	check(m != unsafe { nil }, 'parse returned null')
	defer {
		C.active_module_free(m)
	}
	body(m)
}

fn main() {
	check(C.active_abi_version() == 1, 'ABI major mismatch')
	println('abi: 1 ok')
	with_module('port = 8080\n', fn (m voidptr) {
		s := C.active_check(m, unsafe { nil })
		check(s != unsafe { nil }, 'check returned null')
		n := C.active_diagnostics_len(s)
		C.active_diagnostics_free(s)
		check(n == 0, 'expected clean')
	})
	println('check clean ok')
	with_module('port = 8080\n', fn (m voidptr) {
		v := C.active_eval(m, unsafe { nil })
		check(v != unsafe { nil }, 'eval returned null')
		json := owned_json(v)
		C.active_value_free(v)
		check(json.contains('8080'), 'value missing: ${json}')
	})
	println('eval port=8080 ok')
	with_module('timeout = 5s + 512MiB\n', fn (m voidptr) {
		v := C.active_eval(m, unsafe { nil })
		check(v == unsafe { nil }, 'units mixed evaluated clean')
		s := C.active_check(m, unsafe { nil })
		check(s != unsafe { nil }, 'check returned null')
		n := C.active_diagnostics_len(s)
		mut code := ''
		if n > 0 {
			code = unsafe { cstring_to_vstring(&char(C.active_diagnostic_code(s, 0))) }
		}
		C.active_diagnostics_free(s)
		check(n > 0, 'expected diagnostics')
		check(code == 'T001', 'expected T001, got ${code}')
	})
	println('units refused ok')
	with_module('token = secret("hunter2")\n', fn (m voidptr) {
		v := C.active_eval(m, unsafe { nil })
		check(v != unsafe { nil }, 'eval returned null')
		json := owned_json(v)
		C.active_value_free(v)
		check(json.contains('***SECRET***') && !json.contains('hunter2'),
			'redaction broken: ${json}')
	})
	println('secret redacted ok')
	println('SMOKE-OK')
}
