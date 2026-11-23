//! L-P28 smoke through the real C ABI: load the built cdylib, call the
//! sandboxed entry with a closure manifest, and free the result.

use std::path::PathBuf;

type EvalFn = unsafe extern "C" fn(
    *const std::os::raw::c_char,
    *const std::ffi::c_void,
) -> *mut std::ffi::c_void;
type FreeFn = unsafe extern "C" fn(*mut std::ffi::c_void);

fn cdylib_path() -> Option<PathBuf> {
    if cfg!(miri) {
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

#[test]
fn sandboxed_entry_loads_and_evaluates_a_closure() {
    // Miri executes inside the test sandbox and cannot dlopen a host
    // cdylib; that is the one environment where this native smoke is
    // skipped. Every real build must have the artifact (no skip).
    if cfg!(miri) {
        return;
    }
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
