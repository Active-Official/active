//! L-P28 smoke through the real C ABI: load the built cdylib, call the
//! sandboxed entry with a closure manifest, and free the result.

use std::path::PathBuf;

/// The sandboxed authority set is process-wide inside the loaded cdylib,
/// so tests that touch it must not overlap.
static SANDBOX_STATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

type EvalFn = unsafe extern "C" fn(
    *const std::os::raw::c_char,
    *const std::ffi::c_void,
) -> *mut std::ffi::c_void;
type FreeFn = unsafe extern "C" fn(*mut std::ffi::c_void);

fn cdylib_path() -> Option<PathBuf> {
    if cfg!(miri) {
        return None;
    }
    // An explicit override wins, mirroring
    // `conformance/harness/diff_vectors.py::find_cdylib`: a container or
    // CI build lands in an isolated `CARGO_TARGET_DIR`, and a stale
    // in-repo artifact must never decide which cdylib this test drives.
    if let Some(explicit) = std::env::var_os("ACTIVE_FFI_LIB") {
        let candidate = PathBuf::from(explicit);
        return candidate.is_file().then_some(candidate);
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

#[test]
fn sandboxed_entry_loads_and_evaluates_a_closure() {
    // Miri executes inside the test sandbox and cannot dlopen a host
    // cdylib; that is the one environment where this native smoke is
    // skipped. Every real build must have the artifact (no skip).
    if cfg!(miri) {
        return;
    }
    let _serial = SANDBOX_STATE.lock().expect("sandbox state lock");
    let path = cdylib_path()
        .expect("cdylib absent: build active-ffi before the native smoke (release gate builds it)");
    unsafe {
        let library = libloading::Library::new(path).expect("load cdylib");
        let eval: libloading::Symbol<EvalFn> =
            library.get(b"active_eval_sandboxed").expect("eval symbol");
        let free: libloading::Symbol<FreeFn> =
            library.get(b"active_value_free").expect("free symbol");
        let manifest = std::ffi::CString::new(
            r#"{"version":1,"document_b64":"cCA9IGVudi5QID86ICJkIgo=","source_name":"main.active","includes":[],"env":[{"name":"P","set":true,"value":"closed"}]}"#,
        )
        .unwrap();
        let value = eval(manifest.as_ptr(), std::ptr::null());
        assert!(!value.is_null(), "declared closure must evaluate");
        free(value);
    }
}

#[test]
fn ffi_capability_setter_denies_read_secrets_with_code_17() {
    if cfg!(miri) {
        return;
    }
    let path = cdylib_path().expect("cdylib");
    unsafe {
        let library = libloading::Library::new(path).expect("load cdylib");
        let parse: libloading::Symbol<
            unsafe extern "C" fn(
                *const u8,
                usize,
                *const std::ffi::c_void,
            ) -> *mut std::ffi::c_void,
        > = library.get(b"active_module_parse").expect("parse");
        let set: libloading::Symbol<
            unsafe extern "C" fn(*mut std::ffi::c_void, *const std::os::raw::c_char) -> i32,
        > = library.get(b"active_set_capabilities").expect("set caps");
        let eval: libloading::Symbol<
            unsafe extern "C" fn(
                *const std::ffi::c_void,
                *const std::ffi::c_void,
            ) -> *mut std::ffi::c_void,
        > = library.get(b"active_eval").expect("eval");
        let last: libloading::Symbol<unsafe extern "C" fn() -> i32> =
            library.get(b"active_last_error").expect("last");
        let free_module = library
            .get::<unsafe extern "C" fn(*mut std::ffi::c_void)>(b"active_module_free")
            .expect("free");
        let source = b"p = env.SECRET\n";
        let module = parse(source.as_ptr(), source.len(), std::ptr::null());
        assert!(!module.is_null());
        let caps = std::ffi::CString::new("evaluate").unwrap();
        assert_eq!(set(module, caps.as_ptr()), 0);
        let value = eval(module, std::ptr::null());
        assert!(value.is_null());
        assert_eq!(last(), 17, "E049 must be CAPABILITY_DENIED");
        free_module(module);
    }
}

#[test]
fn sandbox_setter_denies_read_secrets_with_code_17() {
    // Review M1: the sandboxed ABI has an authority dimension.
    if cfg!(miri) {
        return;
    }
    let _serial = SANDBOX_STATE.lock().expect("sandbox state lock");
    let path = cdylib_path().expect("cdylib");
    unsafe {
        let library = libloading::Library::new(path).expect("load cdylib");
        let set: libloading::Symbol<unsafe extern "C" fn(*const std::os::raw::c_char) -> i32> =
            library
                .get(b"active_set_sandbox_capabilities")
                .expect("sandbox caps setter");
        let eval: libloading::Symbol<
            unsafe extern "C" fn(
                *const std::os::raw::c_char,
                *const std::ffi::c_void,
            ) -> *mut std::ffi::c_void,
        > = library.get(b"active_eval_sandboxed").expect("eval");
        let last: libloading::Symbol<unsafe extern "C" fn() -> i32> =
            library.get(b"active_last_error").expect("last");
        let caps = std::ffi::CString::new("evaluate").unwrap();
        assert_eq!(set(caps.as_ptr()), 0);
        let manifest = std::ffi::CString::new(
            r#"{"version":1,"document_b64":"cCA9IGVudi5TRUNSRVQK","source_name":"main.active","includes":[],"env":[{"name":"SECRET","set":true,"value":"x"}]}"#,
        )
        .unwrap();
        let value = eval(manifest.as_ptr(), std::ptr::null());
        assert!(value.is_null());
        assert_eq!(last(), 17, "diagnostic E049 must map to 17");
        // Restore the default for any later test in this process.
        let all = std::ffi::CString::new("evaluate,read-secrets,inspect,apply,sign").unwrap();
        assert_eq!(set(all.as_ptr()), 0);
    }
}

#[test]
fn ffi_check_and_validate_map_capability_denial_to_code_17() {
    // L-P35 M6: the authority refusal must reach the C boundary through
    // the check and validate entry points too, not only active_eval
    // (collect_check classifies the E049 snapshot; active_validate runs
    // classify_check as well).
    if cfg!(miri) {
        return;
    }
    let path = cdylib_path().expect("cdylib");
    unsafe {
        let library = libloading::Library::new(path).expect("load cdylib");
        let parse: libloading::Symbol<
            unsafe extern "C" fn(
                *const u8,
                usize,
                *const std::ffi::c_void,
            ) -> *mut std::ffi::c_void,
        > = library.get(b"active_module_parse").expect("parse");
        let set: libloading::Symbol<
            unsafe extern "C" fn(*mut std::ffi::c_void, *const std::os::raw::c_char) -> i32,
        > = library.get(b"active_set_capabilities").expect("set caps");
        type EntryFn = unsafe extern "C" fn(
            *const std::ffi::c_void,
            *const std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
        let check: libloading::Symbol<EntryFn> = library.get(b"active_check").expect("check");
        let validate: libloading::Symbol<EntryFn> =
            library.get(b"active_validate").expect("validate");
        let diag_len: libloading::Symbol<unsafe extern "C" fn(*const std::ffi::c_void) -> usize> =
            library.get(b"active_diagnostics_len").expect("diag len");
        let diag_code: libloading::Symbol<
            unsafe extern "C" fn(*const std::ffi::c_void, usize) -> *const std::os::raw::c_char,
        > = library.get(b"active_diagnostic_code").expect("diag code");
        let diag_free: libloading::Symbol<unsafe extern "C" fn(*mut std::ffi::c_void)> =
            library.get(b"active_diagnostics_free").expect("diag free");
        let last: libloading::Symbol<unsafe extern "C" fn() -> i32> =
            library.get(b"active_last_error").expect("last");
        let free_module = library
            .get::<unsafe extern "C" fn(*mut std::ffi::c_void)>(b"active_module_free")
            .expect("free");

        let source = b"p = env.SECRET\n";
        let module = parse(source.as_ptr(), source.len(), std::ptr::null());
        assert!(!module.is_null());
        let caps = std::ffi::CString::new("evaluate").unwrap();
        assert_eq!(set(module, caps.as_ptr()), 0);

        let set_handle = check(module, std::ptr::null());
        assert!(!set_handle.is_null());
        assert_eq!(last(), 17, "active_check must report E049 as 17");
        assert!(diag_len(set_handle) >= 1);
        let code = std::ffi::CStr::from_ptr(diag_code(set_handle, 0))
            .to_string_lossy()
            .into_owned();
        assert_eq!(code, "E049");
        diag_free(set_handle);

        let set_handle = validate(module, std::ptr::null());
        assert!(!set_handle.is_null());
        assert_eq!(last(), 17, "active_validate must report E049 as 17");
        diag_free(set_handle);

        free_module(module);
    }
}
