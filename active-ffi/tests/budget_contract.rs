//! FFI budget and options contract (LANG-G-008, DEC-005).
//!
//! The documented contract is tighten-only budgets plus an explicit
//! trusted-mode flag:
//!
//! - a zero budget field keeps the current limit; a non-zero field may only
//!   lower it, so `u64::MAX` cannot be smuggled in as "unlimited";
//! - trusted mode (a real `ActiveOptions` struct) is the only way to raise
//!   the wall-clock ceiling, and it raises nothing else;
//! - a non-null options pointer is never ignored: a wrong size word or an
//!   unknown `trusted` value fails closed with `BAD_ARGUMENT`.
//!
//! Tests drive the BUILT cdylib through its exported symbols, so they check
//! the shipped ABI rather than a re-implementation. Skips honestly when the
//! cdylib is absent (build the crate first), as `parse_limits.rs` does.

use std::path::PathBuf;
use std::ptr;

type ParseFn = unsafe extern "C" fn(*const u8, usize, *const Options) -> *mut std::ffi::c_void;
type EvalFn = unsafe extern "C" fn(*const std::ffi::c_void, *const Budget) -> *mut std::ffi::c_void;
type FreeFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type LastErrorFn = unsafe extern "C" fn() -> i32;

const OK: i32 = 0;
const BAD_BUDGET: i32 = 12;
const BAD_ARGUMENT: i32 = 14;

/// Mirrors `ActiveOptions` in the header (field zero is the size word).
#[repr(C)]
struct Options {
    struct_size: u32,
    trusted: u32,
}

impl Options {
    fn new(trusted: u32) -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>() as u32,
            trusted,
        }
    }
}

/// Mirrors `ActiveBudget` in the header (field zero is the size word).
#[repr(C)]
struct Budget {
    struct_size: u32,
    max_nodes: u64,
    max_depth: u64,
    max_array_len: u64,
    max_keys: u64,
    max_wall_ms: u64,
    max_value_size: u64,
    max_total_value_size: u64,
}

impl Budget {
    /// All-zero fields plus the size word: the shipped limits.
    fn shipped() -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>() as u32,
            max_nodes: 0,
            max_depth: 0,
            max_array_len: 0,
            max_keys: 0,
            max_wall_ms: 0,
            max_value_size: 0,
            max_total_value_size: 0,
        }
    }
}

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

struct Api {
    parse: libloading::Symbol<'static, ParseFn>,
    eval: libloading::Symbol<'static, EvalFn>,
    value_free: libloading::Symbol<'static, FreeFn>,
    module_free: libloading::Symbol<'static, FreeFn>,
    last_error: libloading::Symbol<'static, LastErrorFn>,
}

fn load() -> Option<Api> {
    let path = cdylib_path()?;
    // SAFETY: the library is leaked so it outlives every symbol borrow.
    unsafe {
        let lib: &'static libloading::Library = Box::leak(Box::new(
            libloading::Library::new(&path).expect("dlopen cdylib"),
        ));
        Some(Api {
            parse: lib.get(b"active_module_parse").expect("symbol"),
            eval: lib.get(b"active_eval").expect("symbol"),
            value_free: lib.get(b"active_value_free").expect("symbol"),
            module_free: lib.get(b"active_module_free").expect("symbol"),
            last_error: lib.get(b"active_last_error").expect("symbol"),
        })
    }
}

const DOC: &str = "port = 8080\n";

/// Parse the control document with an optional options pointer.
unsafe fn parse(api: &Api, options: *const Options) -> *mut std::ffi::c_void {
    (api.parse)(DOC.as_ptr(), DOC.len(), options)
}

#[test]
fn a_tightened_budget_evaluates_and_a_loosened_one_is_rejected() {
    let Some(api) = load() else {
        eprintln!("SKIP: cdylib absent (build the crate first)");
        return;
    };
    let shipped = active_core::eval::Budget::default();
    // SAFETY: every pointer comes from the library under test and is freed
    // exactly once; the options pointer is null (the safe default).
    unsafe {
        let handle = parse(&api, ptr::null());
        assert!(!handle.is_null(), "the control document parses");

        let mut tight = Budget::shipped();
        tight.max_nodes = shipped.max_nodes - 1;
        let value = (api.eval)(handle, &tight);
        assert!(!value.is_null(), "a tightened node limit still evaluates");
        assert_eq!((api.last_error)(), OK);
        (api.value_free)(value);

        let mut loose = Budget::shipped();
        loose.max_nodes = shipped.max_nodes + 1;
        let value = (api.eval)(handle, &loose);
        assert!(
            value.is_null(),
            "a budget above the shipped ceiling must be rejected, never applied"
        );
        assert_eq!((api.last_error)(), BAD_BUDGET);

        let mut unlimited = Budget::shipped();
        unlimited.max_nodes = u64::MAX;
        let value = (api.eval)(handle, &unlimited);
        assert!(
            value.is_null(),
            "u64::MAX is a loosening like any other, not an unlimited request"
        );
        assert_eq!((api.last_error)(), BAD_BUDGET);

        (api.module_free)(handle);
    }
}

#[test]
fn trusted_mode_is_the_only_way_to_raise_the_wall_clock() {
    let Some(api) = load() else {
        eprintln!("SKIP: cdylib absent (build the crate first)");
        return;
    };
    let shipped = active_core::eval::Budget::default();
    // SAFETY: see `a_tightened_budget_evaluates_and_a_loosened_one_is_rejected`.
    unsafe {
        let strict = parse(&api, ptr::null());
        assert!(!strict.is_null());
        let trusted_opts = Options::new(1);
        let trusted = parse(&api, &trusted_opts);
        assert!(!trusted.is_null(), "trusted mode is a valid option");

        let mut longer = Budget::shipped();
        longer.max_wall_ms = shipped.max_wall_ms * 10;
        let value = (api.eval)(strict, &longer);
        assert!(
            value.is_null(),
            "a strict handle cannot raise the wall clock through the budget"
        );
        assert_eq!((api.last_error)(), BAD_BUDGET);

        let value = (api.eval)(trusted, &longer);
        assert!(
            !value.is_null(),
            "trusted mode may extend the wall clock, the one raisable field"
        );
        assert_eq!((api.last_error)(), OK);
        (api.value_free)(value);

        // Trusted mode raises the clock only: count limits stay tighten-only.
        let mut loose = Budget::shipped();
        loose.max_nodes = shipped.max_nodes + 1;
        let value = (api.eval)(trusted, &loose);
        assert!(value.is_null(), "trusted mode cannot raise a count limit");
        assert_eq!((api.last_error)(), BAD_BUDGET);

        (api.module_free)(strict);
        (api.module_free)(trusted);
    }
}

#[test]
fn a_non_null_options_pointer_is_validated_never_ignored() {
    let Some(api) = load() else {
        eprintln!("SKIP: cdylib absent (build the crate first)");
        return;
    };
    // SAFETY: each options pointer is valid for the call; the handles that
    // are produced are freed once.
    unsafe {
        let wrong_size = Options {
            struct_size: std::mem::size_of::<Options>() as u32 - 1,
            trusted: 1,
        };
        let handle = parse(&api, &wrong_size);
        assert!(
            handle.is_null(),
            "a wrong size word fails closed instead of being ignored"
        );
        assert_eq!((api.last_error)(), BAD_ARGUMENT);

        let unknown = Options::new(7);
        let handle = parse(&api, &unknown);
        assert!(
            handle.is_null(),
            "an unknown trusted value fails closed instead of guessing"
        );
        assert_eq!((api.last_error)(), BAD_ARGUMENT);

        let ok = Options::new(0);
        let handle = parse(&api, &ok);
        assert!(!handle.is_null(), "trusted = 0 is tighten-only and valid");
        assert_eq!((api.last_error)(), OK);
        (api.module_free)(handle);
    }
}
