//! Reachable FFI error codes (LANG-G-007).
//!
//! Codes 2 (`UNRESOLVED_REF`) and 7 (`INCLUDE_FAIL`) were defined in the
//! header and documented in `docs/FFI.md`, yet no path ever passed them to
//! `set_last`: a C host switching on `active_last_error()` could never see
//! them. These tests drive the BUILT cdylib through the exported symbols
//! and assert each code is observable from the shipped behaviour:
//!
//! - an unresolved identifier warns (W001) and `active_check` reports 2;
//! - a document whose include cannot be resolved reports 7 from both
//!   `active_check` and `active_eval`, never the generic parse code 1;
//! - a clean document keeps reporting 0, so the mapping cannot pass by
//!   turning every check into a failure.
//!
//! Skips honestly when the cdylib is absent (build the crate first), the
//! same convention as `parse_limits.rs`.

use std::path::PathBuf;
use std::ptr;

type ParseFn =
    unsafe extern "C" fn(*const u8, usize, *const std::ffi::c_void) -> *mut std::ffi::c_void;
type CheckFn =
    unsafe extern "C" fn(*const std::ffi::c_void, *const std::ffi::c_void) -> *mut std::ffi::c_void;
type EvalFn =
    unsafe extern "C" fn(*const std::ffi::c_void, *const std::ffi::c_void) -> *mut std::ffi::c_void;
type LenFn = unsafe extern "C" fn(*const std::ffi::c_void) -> usize;
type CodeFn = unsafe extern "C" fn(*const std::ffi::c_void, usize) -> *const std::os::raw::c_char;
type SeverityFn = unsafe extern "C" fn(*const std::ffi::c_void, usize) -> u32;
type FreeFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type LastErrorFn = unsafe extern "C" fn() -> i32;

const OK: i32 = 0;
const UNRESOLVED_REF: i32 = 2;
const INCLUDE_FAIL: i32 = 7;

fn cdylib_path() -> Option<PathBuf> {
    if cfg!(miri) {
        // Miri cannot dlopen a host cdylib: libloading calls OS APIs the
        // interpreter does not model. The library-level tests still run.
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

/// Everything the code assertions need from one loaded cdylib.
struct Api {
    parse: libloading::Symbol<'static, ParseFn>,
    check: libloading::Symbol<'static, CheckFn>,
    eval: libloading::Symbol<'static, EvalFn>,
    len: libloading::Symbol<'static, LenFn>,
    code: libloading::Symbol<'static, CodeFn>,
    severity: libloading::Symbol<'static, SeverityFn>,
    value_free: libloading::Symbol<'static, FreeFn>,
    diagnostics_free: libloading::Symbol<'static, FreeFn>,
    module_free: libloading::Symbol<'static, FreeFn>,
    last_error: libloading::Symbol<'static, LastErrorFn>,
}

fn load() -> Option<Api> {
    let path = cdylib_path()?;
    // SAFETY: the library stays alive (leaked) for the whole test process,
    // so every symbol borrow is genuinely 'static.
    unsafe {
        let lib: &'static libloading::Library = Box::leak(Box::new(
            libloading::Library::new(&path).expect("dlopen cdylib"),
        ));
        Some(Api {
            parse: lib.get(b"active_module_parse").expect("symbol"),
            check: lib.get(b"active_check").expect("symbol"),
            eval: lib.get(b"active_eval").expect("symbol"),
            len: lib.get(b"active_diagnostics_len").expect("symbol"),
            code: lib.get(b"active_diagnostic_code").expect("symbol"),
            severity: lib.get(b"active_diagnostic_severity").expect("symbol"),
            value_free: lib.get(b"active_value_free").expect("symbol"),
            diagnostics_free: lib.get(b"active_diagnostics_free").expect("symbol"),
            module_free: lib.get(b"active_module_free").expect("symbol"),
            last_error: lib.get(b"active_last_error").expect("symbol"),
        })
    }
}

/// Parse source bytes into a handle; asserts the shipped parser accepts it
/// (both fixtures are syntactically valid, includes are only recorded).
unsafe fn parse(api: &Api, source: &str) -> *mut std::ffi::c_void {
    let handle = (api.parse)(source.as_ptr(), source.len(), ptr::null());
    assert!(
        !handle.is_null(),
        "the parser records includes and accepts `{}`; last_error={}",
        source.lines().next().unwrap_or(""),
        (api.last_error)()
    );
    assert_eq!(
        (api.last_error)(),
        OK,
        "a successful parse reports OK before any check runs"
    );
    handle
}

/// Severity-1/2/3/4 rows of a diagnostic set, then frees it.
unsafe fn drain(api: &Api, set: *mut std::ffi::c_void) -> Vec<(String, u32)> {
    assert!(!set.is_null(), "check must return an owned set, never null");
    let len = (api.len)(set);
    let mut rows = Vec::with_capacity(len);
    for i in 0..len {
        let code = (api.code)(set, i);
        assert!(!code.is_null(), "every diagnostic carries a code");
        let code = std::ffi::CStr::from_ptr(code)
            .to_string_lossy()
            .into_owned();
        rows.push((code, (api.severity)(set, i)));
    }
    (api.diagnostics_free)(set);
    rows
}

#[test]
fn an_unresolved_identifier_reports_code_2() {
    let Some(api) = load() else {
        eprintln!("SKIP: cdylib absent (build the crate first)");
        return;
    };
    // SAFETY: every pointer is produced by the library under test; the
    // handles are freed exactly once at the end of each branch.
    unsafe {
        let handle = parse(&api, "x = nosuch_name\n");
        let rows = drain(&api, (api.check)(handle, ptr::null()));
        assert!(
            rows.iter().any(|(code, sev)| code == "W001" && *sev == 2),
            "an unresolved identifier warns (W001, severity 2); got {rows:?}"
        );
        assert_eq!(
            (api.last_error)(),
            UNRESOLVED_REF,
            "a check that carries W001 reports code 2; got {}",
            (api.last_error)()
        );
        (api.module_free)(handle);
    }
}

#[test]
fn an_unresolvable_include_reports_code_7_from_check_and_eval() {
    let Some(api) = load() else {
        eprintln!("SKIP: cdylib absent (build the crate first)");
        return;
    };
    // SAFETY: see `an_unresolved_identifier_reports_code_2`.
    unsafe {
        // No include loader is registered, so the recorded include cannot
        // be resolved: the documented outcome is E015 and code 7.
        let handle = parse(&api, "include \"missing.active\"\nport = 8080\n");

        let rows = drain(&api, (api.check)(handle, ptr::null()));
        assert!(
            rows.iter()
                .any(|(code, sev)| code.starts_with("E0") && *sev == 1),
            "the include failure is an error-severity E-series diagnostic; got {rows:?}"
        );
        assert_eq!(
            (api.last_error)(),
            INCLUDE_FAIL,
            "an unresolvable include reports code 7, never the generic parse code 1"
        );

        let value = (api.eval)(handle, ptr::null());
        assert!(
            value.is_null(),
            "eval cannot produce a value when the include is unresolvable"
        );
        assert_eq!(
            (api.last_error)(),
            INCLUDE_FAIL,
            "eval agrees with check on the include class"
        );
        (api.module_free)(handle);
    }
}

#[test]
fn a_clean_document_reports_code_0() {
    let Some(api) = load() else {
        eprintln!("SKIP: cdylib absent (build the crate first)");
        return;
    };
    // SAFETY: see `an_unresolved_identifier_reports_code_2`.
    unsafe {
        let handle = parse(&api, "port = 8080\n");
        let rows = drain(&api, (api.check)(handle, ptr::null()));
        assert!(rows.is_empty(), "a clean document yields no diagnostics");
        assert_eq!(
            (api.last_error)(),
            OK,
            "the mapping must not turn every check into a failure"
        );

        let value = (api.eval)(handle, ptr::null());
        assert!(!value.is_null(), "a clean document evaluates");
        (api.value_free)(value);
        assert_eq!((api.last_error)(), OK);
        (api.module_free)(handle);
    }
}
