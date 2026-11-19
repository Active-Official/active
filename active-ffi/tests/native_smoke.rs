//! Native smoke tests (F-018): load the BUILT cdylib through the real C
//! ABI (no Rust-level calls), parse a fixture, eval, assert the JSON and a
//! deliberate failure code. Skips honestly when the cdylib is absent.

use std::path::PathBuf;

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type ParseFn =
    unsafe extern "C" fn(*const u8, usize, *const std::ffi::c_void) -> *mut std::ffi::c_void;
type CheckFn =
    unsafe extern "C" fn(*const std::ffi::c_void, *const std::ffi::c_void) -> *mut std::ffi::c_void;
type LenFn = unsafe extern "C" fn(*const std::ffi::c_void) -> usize;
type EvalFn =
    unsafe extern "C" fn(*const std::ffi::c_void, *const std::ffi::c_void) -> *mut std::ffi::c_void;
type ToJsonFn = unsafe extern "C" fn(*const std::ffi::c_void) -> *mut std::os::raw::c_char;
type FreeFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type FreeStrFn = unsafe extern "C" fn(*mut std::os::raw::c_char);

fn cdylib_path() -> Option<PathBuf> {
    if cfg!(miri) {
        // Miri cannot dlopen a host cdylib: libloading calls OS APIs the
        // interpreter does not model. The library-level tests still run.
        return None;
    }
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let name = if cfg!(target_os = "windows") {
        "active_ffi.dll"
    } else if cfg!(target_os = "macos") {
        "libactive_ffi.dylib"
    } else {
        "libactive_ffi.so"
    };
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join(profile)
        .join(name);
    path.is_file().then_some(path)
}

fn cstr(ptr: *const std::os::raw::c_char) -> String {
    // SAFETY: test-only read of a Model A string the ABI just returned.
    unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

#[test]
fn cdylib_parse_eval_json_roundtrip() {
    let path = match cdylib_path() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: cdylib absent (build the crate first)");
            return;
        }
    };
    // SAFETY: test loads the just-built crate ABI per docs/FFI.md.
    unsafe {
        let lib = libloading::Library::new(&path).expect("dlopen cdylib");
        let abi: libloading::Symbol<AbiVersionFn> =
            lib.get(b"active_abi_version").expect("abi symbol");
        assert_eq!(abi(), 1);
        let parse: libloading::Symbol<ParseFn> = lib.get(b"active_module_parse").expect("parse");
        let check: libloading::Symbol<CheckFn> = lib.get(b"active_check").expect("check");
        let len: libloading::Symbol<LenFn> = lib.get(b"active_diagnostics_len").expect("len");
        let eval: libloading::Symbol<EvalFn> = lib.get(b"active_eval").expect("eval");
        let to_json: libloading::Symbol<ToJsonFn> = lib.get(b"active_value_to_json").expect("json");
        let free_mod: libloading::Symbol<FreeFn> =
            lib.get(b"active_module_free").expect("free mod");
        let free_set: libloading::Symbol<FreeFn> =
            lib.get(b"active_diagnostics_free").expect("free set");
        let free_val: libloading::Symbol<FreeFn> = lib.get(b"active_value_free").expect("free val");
        let free_str: libloading::Symbol<FreeStrFn> =
            lib.get(b"active_string_free").expect("free str");

        let src = b"port = 8080\n";
        let module = parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(!module.is_null());
        let set = check(module, std::ptr::null());
        assert!(!set.is_null());
        assert_eq!(len(set), 0);
        let value = eval(module, std::ptr::null());
        assert!(!value.is_null());
        let json = cstr(to_json(value));
        assert!(json.contains("8080"), "{json}");
        free_str(to_json(value));
        free_val(value);
        free_set(set);

        let bad = b"timeout = 5s + 512MiB\n";
        let module = parse(bad.as_ptr(), bad.len(), std::ptr::null());
        assert!(!module.is_null());
        let set = check(module, std::ptr::null());
        assert!(!set.is_null());
        assert!(len(set) >= 1, "broken units must diagnose");
        let value = eval(module, std::ptr::null());
        assert!(value.is_null(), "broken units must not evaluate");
        free_set(set);
        free_mod(module);
    }
}

#[test]
fn a_resilient_schema_error_refuses_at_the_c_abi() {
    // DEC-004 / LANG-G-002: the resilient engine returns a tree plus an
    // error-severity diagnostic for a closed-table violation. The C
    // boundary must still return null and the mapped numeric code; the old
    // smoke case (a unit TypeError) already returned null before the fix,
    // so it could not catch this.
    let path = match cdylib_path() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: cdylib absent (build the crate first)");
            return;
        }
    };
    type LastErrorFn = unsafe extern "C" fn() -> i32;
    // SAFETY: test loads the just-built crate ABI per docs/FFI.md.
    unsafe {
        let lib = libloading::Library::new(&path).expect("dlopen cdylib");
        let parse: libloading::Symbol<ParseFn> = lib.get(b"active_module_parse").expect("parse");
        let eval: libloading::Symbol<EvalFn> = lib.get(b"active_eval").expect("eval");
        let last_error: libloading::Symbol<LastErrorFn> =
            lib.get(b"active_last_error").expect("last error");
        let free_mod: libloading::Symbol<FreeFn> =
            lib.get(b"active_module_free").expect("free mod");
        let free_val: libloading::Symbol<FreeFn> = lib.get(b"active_value_free").expect("free val");

        // conformance/cases/0010-closed-table.active: an extra key in a
        // closed schema is T005 (Error) under the resilient engine.
        let src = b"schema = { srv = Table { host = String, closed = true } }\n[srv]\nhost = \"h\"\nextra = 1\n";
        let module = parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(!module.is_null(), "the document parses");
        let value = eval(module, std::ptr::null());
        assert!(
            value.is_null(),
            "an error-severity diagnostic must refuse at the C ABI"
        );
        assert_eq!(last_error(), 8, "T005 maps to ACTIVE_ERR_SCHEMA_FAIL (8)");
        free_val(value);
        free_mod(module);
    }
}

#[test]
fn validate_reports_a_failed_check_as_schema_fail() {
    let path = match cdylib_path() {
        Some(path) => path,
        None => {
            eprintln!("SKIP: cdylib absent (build the crate first)");
            return;
        }
    };
    // SAFETY: test loads the just-built crate ABI per docs/FFI.md.
    unsafe {
        type ValidateFn = unsafe extern "C" fn(
            *const std::ffi::c_void,
            *const std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
        type LastErrorFn = unsafe extern "C" fn() -> i32;
        let lib = libloading::Library::new(&path).expect("dlopen cdylib");
        let parse: libloading::Symbol<ParseFn> = lib.get(b"active_module_parse").expect("parse");
        let validate: libloading::Symbol<ValidateFn> =
            lib.get(b"active_validate").expect("validate");
        let len: libloading::Symbol<LenFn> = lib.get(b"active_diagnostics_len").expect("len");
        let last_error: libloading::Symbol<LastErrorFn> =
            lib.get(b"active_last_error").expect("last error");
        let free_mod: libloading::Symbol<FreeFn> =
            lib.get(b"active_module_free").expect("free mod");
        let free_set: libloading::Symbol<FreeFn> =
            lib.get(b"active_diagnostics_free").expect("free set");

        let src = b"schema { check { ok = port > 0 } }\nport = -1\n";
        let module = parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(!module.is_null(), "the document parses");
        let diagnostics = validate(module, std::ptr::null());
        assert!(!diagnostics.is_null(), "a diagnostic set is returned");
        assert!(len(diagnostics) >= 1, "T009 is in the schema set");
        assert_eq!(last_error(), 8, "T009 maps to ACTIVE_ERR_SCHEMA_FAIL (8)");
        free_set(diagnostics);
        free_mod(module);
    }
}
