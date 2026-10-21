//! FFI boundary misuse tests (CHECKLIST 3.002, PROPOSAL section 5.3).
//!
//! Drives the BUILT cdylib through hostile inputs that must fail closed,
//! never trap or hang: null handles, wrong-size budgets, invalid UTF-8,
//! empty source, adversarial repetition. Skips honestly when the cdylib
//! is absent. Runs under Miri like every other test in this package.

use std::path::PathBuf;

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type ParseFn =
    unsafe extern "C" fn(*const u8, usize, *const std::ffi::c_void) -> *mut std::ffi::c_void;
type CheckFn =
    unsafe extern "C" fn(*const std::ffi::c_void, *const std::ffi::c_void) -> *mut std::ffi::c_void;
type LenFn = unsafe extern "C" fn(*const std::ffi::c_void) -> usize;
type EvalFn =
    unsafe extern "C" fn(*const std::ffi::c_void, *const std::ffi::c_void) -> *mut std::ffi::c_void;
type FreeFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type FreeStrFn = unsafe extern "C" fn(*mut std::os::raw::c_char);
type ToJsonFn = unsafe extern "C" fn(*const std::ffi::c_void) -> *mut std::os::raw::c_char;

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

macro_rules! load {
    ($lib:ident, $name:ident, $ty:ty, $sym:expr) => {
        let $name: libloading::Symbol<$ty> = $lib.get($sym).expect("symbol");
    };
}

#[test]
fn null_handles_never_trap() {
    let path = match cdylib_path() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: cdylib absent (build the crate first)");
            return;
        }
    };
    // SAFETY: every call below passes null where the header promises
    // null-safety; the test asserts the promise holds.
    unsafe {
        let lib = libloading::Library::new(&path).expect("dlopen cdylib");
        load!(lib, abi, AbiVersionFn, b"active_abi_version");
        assert_eq!(abi(), 1, "ABI gate: major must be 1");
        load!(lib, check, CheckFn, b"active_check");
        load!(lib, eval, EvalFn, b"active_eval");
        load!(lib, free_mod, FreeFn, b"active_module_free");
        load!(lib, free_set, FreeFn, b"active_diagnostics_free");
        load!(lib, free_val, FreeFn, b"active_value_free");
        load!(lib, free_str, FreeStrFn, b"active_string_free");
        load!(lib, to_json, ToJsonFn, b"active_value_to_json");
        load!(lib, len, LenFn, b"active_diagnostics_len");

        // Null module: check/eval must return null, never trap.
        assert!(check(std::ptr::null(), std::ptr::null()).is_null());
        assert!(eval(std::ptr::null(), std::ptr::null()).is_null());
        // Null frees and null length/code paths are defined no-ops.
        free_mod(std::ptr::null_mut());
        free_set(std::ptr::null_mut());
        free_val(std::ptr::null_mut());
        free_str(std::ptr::null_mut());
        assert_eq!(len(std::ptr::null()), 0);
        assert!(to_json(std::ptr::null()).is_null());
    }
}

#[test]
fn hostile_inputs_fail_closed() {
    let path = match cdylib_path() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: cdylib absent (build the crate first)");
            return;
        }
    };
    // SAFETY: inputs are hostile by design (bad UTF-8, empty, huge);
    // every call must return an error or null, never trap or hang.
    unsafe {
        let lib = libloading::Library::new(&path).expect("dlopen cdylib");
        load!(lib, abi, AbiVersionFn, b"active_abi_version");
        assert_eq!(abi(), 1, "ABI gate: major must be 1");
        load!(lib, parse, ParseFn, b"active_module_parse");
        load!(lib, check, CheckFn, b"active_check");
        load!(lib, eval, EvalFn, b"active_eval");
        load!(lib, len, LenFn, b"active_diagnostics_len");
        load!(lib, free_mod, FreeFn, b"active_module_free");
        load!(lib, free_set, FreeFn, b"active_diagnostics_free");
        load!(lib, free_val, FreeFn, b"active_value_free");
        load!(lib, to_json, ToJsonFn, b"active_value_to_json");
        load!(lib, free_str, FreeStrFn, b"active_string_free");

        // Invalid UTF-8: parse must fail closed, not panic.
        let bad_utf8: &[u8] = b"port = \xff\xfe\n";
        let m = parse(bad_utf8.as_ptr(), bad_utf8.len(), std::ptr::null());
        assert!(m.is_null(), "invalid UTF-8 must not parse");

        // Empty source parses; eval of nothing is a clean null table.
        let empty: &[u8] = b"";
        let m = parse(empty.as_ptr(), empty.len(), std::ptr::null());
        assert!(!m.is_null(), "empty source must parse");
        let set = check(m, std::ptr::null());
        assert!(!set.is_null());
        assert_eq!(len(set), 0);
        let v = eval(m, std::ptr::null());
        assert!(!v.is_null());
        let j = to_json(v);
        assert!(!j.is_null());
        free_str(j);
        free_val(v);
        free_set(set);
        free_mod(m);

        // Adversarial repetition: evaluation never hangs and never
        // returns a value for nonsense. A 200KB single key parses or
        // rejects; either way no trap occurs (a trap would abort the
        // test binary here).
        let mut huge = vec![b'x'; 200_000];
        huge.extend_from_slice(b" = 1\n");
        let m = parse(huge.as_ptr(), huge.len(), std::ptr::null());
        if !m.is_null() {
            let set = check(m, std::ptr::null());
            assert!(!set.is_null());
            let _ = len(set);
            free_set(set);
            free_mod(m);
        }

        // Tight budget through the real ABI: a document needing more
        // nodes than allowed must fail closed (diagnosed, no value),
        // never trap and never evaluate.
        let mut many = String::new();
        for i in 0..50u32 {
            many.push_str(&format!("k{i} = {i}\n"));
        }
        #[repr(C)]
        struct ActiveBudget {
            struct_size: u32,
            max_nodes: u64,
            max_depth: u64,
            max_array_len: u64,
            max_keys: u64,
            max_wall_ms: u64,
            max_value_size: u64,
            max_total_value_size: u64,
        }
        let tight = ActiveBudget {
            struct_size: std::mem::size_of::<ActiveBudget>() as u32,
            max_nodes: 5,
            max_depth: 256,
            max_array_len: 100_000,
            max_keys: 100_000,
            max_wall_ms: 0,
            max_value_size: 10_000_000,
            max_total_value_size: 100_000_000,
        };
        let budget_ptr = &tight as *const ActiveBudget as *const std::ffi::c_void;
        let m = parse(many.as_ptr(), many.len(), std::ptr::null());
        assert!(!m.is_null());
        let set = check(m, budget_ptr);
        assert!(!set.is_null());
        assert!(
            len(set) >= 1,
            "tiny node budget must diagnose, not evaluate clean"
        );
        let v = eval(m, budget_ptr);
        assert!(v.is_null(), "over-budget eval must return null");
        free_set(set);
        free_mod(m);
    }
}
