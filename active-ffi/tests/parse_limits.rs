//! FFI parse-bound configuration (LANG-C-003).
//!
//! Drives the BUILT cdylib: the default parse cap rejects a document over
//! 1 MiB with PARSE, `active_parse_limits_set` raises it for later parses,
//! and resetting to `0` restores the shipped default. Runs on its own test
//! binary because the setting is process-wide.
//!
//! Skips honestly when the cdylib is absent (build the crate first), the
//! same convention as `misuse.rs`.

use std::path::PathBuf;

type SetLimitsFn = unsafe extern "C" fn(usize, u64) -> i32;
type ParseFn =
    unsafe extern "C" fn(*const u8, usize, *const std::ffi::c_void) -> *mut std::ffi::c_void;
type FreeFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type LastErrorFn = unsafe extern "C" fn() -> i32;

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

/// A clean document just over the 1 MiB default cap.
fn oversize_document() -> String {
    let mut src = String::new();
    let mut i = 0usize;
    while src.len() <= 1024 * 1024 {
        src.push_str(&format!("k{i} = {i}\n"));
        i += 1;
    }
    src
}

#[test]
fn parse_limits_set_raises_and_restores_the_default_cap() {
    let path = match cdylib_path() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: cdylib absent (build the crate first)");
            return;
        }
    };
    // SAFETY: every call passes pointers the callee documents as valid;
    // the loaded library is kept alive for the whole test.
    unsafe {
        let lib = libloading::Library::new(&path).expect("dlopen cdylib");
        let set_limits: libloading::Symbol<SetLimitsFn> =
            lib.get(b"active_parse_limits_set").expect("symbol");
        let parse: libloading::Symbol<ParseFn> = lib.get(b"active_module_parse").expect("symbol");
        let free: libloading::Symbol<FreeFn> = lib.get(b"active_module_free").expect("symbol");
        let last_error: libloading::Symbol<LastErrorFn> =
            lib.get(b"active_last_error").expect("symbol");

        let src = oversize_document();
        assert!(src.len() > 1024 * 1024);

        // Default cap: the same document fails to parse.
        let handle = parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(
            handle.is_null(),
            "the default 1 MiB cap must reject an oversize document"
        );
        assert_eq!(last_error(), 1, "PARSE is the coded failure");

        // A larger explicit cap accepts it.
        assert_eq!(set_limits(src.len() + 1, 0), 0, "setter reports OK");
        let handle = parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(
            !handle.is_null(),
            "an explicit cap above the document size must accept it"
        );
        free(handle);

        // Resetting to 0 restores the shipped default.
        assert_eq!(set_limits(0, 0), 0);
        let handle = parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(
            handle.is_null(),
            "reset must restore the shipped default cap"
        );
        assert_eq!(last_error(), 1);
    }
}
