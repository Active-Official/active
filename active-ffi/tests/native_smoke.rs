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
