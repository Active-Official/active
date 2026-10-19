//! `active-ffi`: C ABI for embedding Active (docs/FFI.md is the contract).
//!
//! Opaque handles only; every export is panic-isolated; Model A owns every
//! buffer crossing the boundary (free with the matching `active_*_free`).

use active_core::{parse, Diagnostic, Engine, EvalError, Module, Severity, Value};
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::slice;
use std::str;

/// ABI version line (docs/FFI.md section 8). Bindings fail closed on major
/// mismatch (H-002).
pub const ACTIVE_ABI_VERSION: u32 = 1;

/// Error codes (docs/FFI.md section 3; codes are API, never reused).
pub const ACTIVE_ERR_OK: i32 = 0;
pub const ACTIVE_ERR_PARSE: i32 = 1;
pub const ACTIVE_ERR_UNRESOLVED_REF: i32 = 2;
pub const ACTIVE_ERR_CYCLE: i32 = 3;
pub const ACTIVE_ERR_UNIT_MISMATCH: i32 = 4;
pub const ACTIVE_ERR_SECRET_REFUSAL: i32 = 5;
pub const ACTIVE_ERR_BUDGET_EXCEEDED: i32 = 6;
pub const ACTIVE_ERR_INCLUDE_FAIL: i32 = 7;
pub const ACTIVE_ERR_SCHEMA_FAIL: i32 = 8;
pub const ACTIVE_ERR_NULL_HANDLE: i32 = 9;
pub const ACTIVE_ERR_PANIC_CAUGHT: i32 = 10;
pub const ACTIVE_ERR_ABI_MISMATCH: i32 = 11;
pub const ACTIVE_ERR_BAD_BUDGET: i32 = 12;
pub const ACTIVE_ERR_UTF8: i32 = 13;

thread_local! {
    static LAST_ERROR: Cell<i32> = const { Cell::new(ACTIVE_ERR_OK) };
}

fn set_last(code: i32) {
    LAST_ERROR.with(|c| c.set(code));
}

/// Central panic isolation (F-011): no unwind ever crosses the ABI; a
/// caught panic becomes a stable code.
fn guard<R>(default: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            set_last(ACTIVE_ERR_PANIC_CAUGHT);
            default
        }
    }
}

/// Opaque parsed module (F-012). The source text is owned here so spans
/// stay valid for the handle lifetime.
pub struct ActiveModule {
    #[allow(dead_code)]
    source: String,
    #[allow(dead_code)]
    module: Module,
}

/// ABI version constant for bindings to check at load time.
#[no_mangle]
pub extern "C" fn active_abi_version() -> u32 {
    ACTIVE_ABI_VERSION
}

/// Code from the last failed call on this thread (OK when clean).
#[no_mangle]
pub extern "C" fn active_last_error() -> i32 {
    guard(ACTIVE_ERR_PANIC_CAUGHT, || LAST_ERROR.with(|c| c.get()))
}

/// Parse UTF-8 source bytes into an owned module handle (null on error;
/// detail via `active_last_error`). Null-safe on the source pointer.
#[no_mangle]
pub extern "C" fn active_module_parse(
    source: *const u8,
    len: usize,
    _options: *const std::ffi::c_void,
) -> *mut ActiveModule {
    guard(std::ptr::null_mut(), || {
        if source.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return std::ptr::null_mut();
        }
        // SAFETY: caller guarantees `len` readable bytes at `source`
        // for this call (documented in docs/FFI.md section 1).
        let bytes = unsafe { slice::from_raw_parts(source, len) };
        let text = match str::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => {
                set_last(ACTIVE_ERR_UTF8);
                return std::ptr::null_mut();
            }
        };
        match parse(text, "ffi") {
            Ok(module) => {
                set_last(ACTIVE_ERR_OK);
                Box::into_raw(Box::new(ActiveModule {
                    source: text.to_owned(),
                    module,
                }))
            }
            Err(_) => {
                set_last(ACTIVE_ERR_PARSE);
                std::ptr::null_mut()
            }
        }
    })
}

/// Free a module handle. Null-safe; double-free is a host bug, not UB
/// here (second free of the same pointer is still UB by Rust aliasing
/// rules; H-001 tests the null and misuse paths that are defined).
#[no_mangle]
pub extern "C" fn active_module_free(handle: *mut ActiveModule) {
    guard((), || {
        if handle.is_null() {
            return;
        }
        // SAFETY: handle came from `active_module_parse` and was not
        // freed before (documented ownership, docs/FFI.md section 1).
        unsafe {
            drop(Box::from_raw(handle));
        }
    });
}

/// Snapshot of one diagnostic with owned strings (F-013). Pointers handed
/// out by the accessors borrow from this set and die with it (Model A).
struct DiagSnapshot {
    code: std::ffi::CString,
    severity: u32,
    message: std::ffi::CString,
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
    has_span: bool,
}

fn severity_code(sev: &Severity) -> u32 {
    match sev {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Info => 3,
        Severity::Hint => 4,
    }
}

fn cstring_lossy(text: &str) -> std::ffi::CString {
    std::ffi::CString::new(text.replace('\0', "\u{FFFD}")).unwrap_or_default()
}

fn snapshot(diag: &Diagnostic) -> DiagSnapshot {
    let (line, column, end_line, end_column, has_span) = match &diag.span {
        Some(s) => (s.line, s.column, s.end_line, s.end_column, true),
        None => (0, 0, 0, 0, false),
    };
    DiagSnapshot {
        code: cstring_lossy(&diag.code),
        severity: severity_code(&diag.severity),
        message: cstring_lossy(&diag.message),
        line,
        column,
        end_line,
        end_column,
        has_span,
    }
}

fn snapshot_error(err: &EvalError) -> DiagSnapshot {
    let (line, column, end_line, end_column, has_span) = match err.span() {
        Some(s) => (s.line, s.column, s.end_line, s.end_column, true),
        None => (0, 0, 0, 0, false),
    };
    DiagSnapshot {
        code: cstring_lossy(err.code()),
        severity: 1,
        message: cstring_lossy(&err.to_string()),
        line,
        column,
        end_line,
        end_column,
        has_span,
    }
}

/// Owned diagnostic set from check/eval (F-013). Empty means clean.
pub struct ActiveDiagnosticSet {
    items: Vec<DiagSnapshot>,
}

/// Owned evaluated value handle (F-014/F-015).
pub struct ActiveValue {
    value: Value,
}

/// C layout of the budget struct (docs/FFI.md section 4). Field zero is
/// the size word (G-016); every other zero selects its shipped default.
#[repr(C)]
pub struct ActiveBudget {
    pub struct_size: u32,
    pub max_nodes: u64,
    pub max_depth: u64,
    pub max_array_len: u64,
    pub max_keys: u64,
    pub max_wall_ms: u64,
    pub max_value_size: u64,
    pub max_total_value_size: u64,
}

fn hermetic_engine() -> Engine {
    // Empty environment snapshot: the host process ambient must never leak
    // across the ABI (docs/FFI.md section 7). Host env arrives only through
    // the F-016 lookup callback.
    Engine::new().with_env(std::collections::HashMap::new())
}

fn apply_budget(engine: &mut Engine, budget: *const ActiveBudget) -> Result<(), i32> {
    if budget.is_null() {
        return Ok(());
    }
    // SAFETY: caller passes a valid ActiveBudget (or null); the size word
    // is validated before any other field is read (G-016).
    let b = unsafe { &*budget };
    if b.struct_size as usize != std::mem::size_of::<ActiveBudget>() {
        set_last(ACTIVE_ERR_BAD_BUDGET);
        return Err(ACTIVE_ERR_BAD_BUDGET);
    }
    let shipped = active_core::eval::Budget::default();
    let get = |v: u64, d: u64| if v == 0 { d } else { v };
    engine.budget = active_core::eval::Budget {
        max_nodes: get(b.max_nodes, shipped.max_nodes),
        max_depth: get(b.max_depth, shipped.max_depth),
        max_array_len: get(b.max_array_len, shipped.max_array_len),
        max_keys: get(b.max_keys, shipped.max_keys),
        max_wall_ms: get(b.max_wall_ms, shipped.max_wall_ms),
        max_value_size: get(b.max_value_size, shipped.max_value_size),
        max_total_value_size: get(b.max_total_value_size, shipped.max_total_value_size),
    };
    Ok(())
}

fn collect_check(
    module: &ActiveModule,
    budget: *const ActiveBudget,
) -> Result<Vec<DiagSnapshot>, i32> {
    let mut engine = hermetic_engine();
    apply_budget(&mut engine, budget)?;
    match engine.evaluate(&module.module) {
        Ok((_, diags)) => Ok(diags.iter().map(snapshot).collect()),
        Err(EvalError::ValidationFailed { diagnostics }) => {
            Ok(diagnostics.iter().map(snapshot).collect())
        }
        Err(e) => Ok(vec![snapshot_error(&e)]),
    }
}

/// Check a module without producing values (F-013/F-019 shape): returns an
/// owned set, empty when clean. Mirrors `active check` strictness
/// (warnings fail downstream; the set carries them).
#[no_mangle]
pub extern "C" fn active_check(
    module: *const ActiveModule,
    budget: *const ActiveBudget,
) -> *mut ActiveDiagnosticSet {
    guard(std::ptr::null_mut(), || {
        if module.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return std::ptr::null_mut();
        }
        // SAFETY: handle came from `active_module_parse` and is live
        // (docs/FFI.md section 1).
        let module = unsafe { &*module };
        match collect_check(module, budget) {
            Ok(items) => {
                set_last(ACTIVE_ERR_OK);
                Box::into_raw(Box::new(ActiveDiagnosticSet { items }))
            }
            Err(code) => {
                set_last(code);
                std::ptr::null_mut()
            }
        }
    })
}

/// Evaluate a module to an owned value handle, or null on error with the
/// diagnostic available through a follow-up `active_check` (F-014).
#[no_mangle]
pub extern "C" fn active_eval(
    module: *const ActiveModule,
    budget: *const ActiveBudget,
) -> *mut ActiveValue {
    guard(std::ptr::null_mut(), || {
        if module.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return std::ptr::null_mut();
        }
        // SAFETY: live handle per docs/FFI.md section 1.
        let module = unsafe { &*module };
        let mut engine = hermetic_engine();
        if apply_budget(&mut engine, budget).is_err() {
            return std::ptr::null_mut();
        }
        match engine.evaluate(&module.module) {
            Ok((value, _)) => {
                set_last(ACTIVE_ERR_OK);
                Box::into_raw(Box::new(ActiveValue { value }))
            }
            Err(EvalError::ValidationFailed { .. }) => {
                set_last(ACTIVE_ERR_SCHEMA_FAIL);
                std::ptr::null_mut()
            }
            Err(EvalError::BudgetExceeded { .. }) => {
                set_last(ACTIVE_ERR_BUDGET_EXCEEDED);
                std::ptr::null_mut()
            }
            Err(EvalError::CycleDetected) => {
                set_last(ACTIVE_ERR_CYCLE);
                std::ptr::null_mut()
            }
            Err(EvalError::SecretLeak { .. }) => {
                set_last(ACTIVE_ERR_SECRET_REFUSAL);
                std::ptr::null_mut()
            }
            Err(EvalError::TypeError { .. }) => {
                set_last(ACTIVE_ERR_UNIT_MISMATCH);
                std::ptr::null_mut()
            }
            Err(_) => {
                set_last(ACTIVE_ERR_PARSE);
                std::ptr::null_mut()
            }
        }
    })
}

fn with_set<F, R>(set: *const ActiveDiagnosticSet, default: R, f: F) -> R
where
    F: FnOnce(&ActiveDiagnosticSet) -> R,
    R: Clone,
{
    guard(default.clone(), || {
        if set.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return default;
        }
        // SAFETY: live set per docs/FFI.md section 1.
        f(unsafe { &*set })
    })
}

/// Number of diagnostics in the set.
#[no_mangle]
pub extern "C" fn active_diagnostics_len(set: *const ActiveDiagnosticSet) -> usize {
    with_set(set, 0, |s| s.items.len())
}

/// Code string at index, borrowed from the set (null on OOB).
#[no_mangle]
pub extern "C" fn active_diagnostic_code(
    set: *const ActiveDiagnosticSet,
    index: usize,
) -> *const std::os::raw::c_char {
    with_set(set, std::ptr::null(), |s| match s.items.get(index) {
        Some(d) => d.code.as_ptr(),
        None => std::ptr::null(),
    })
}

/// Severity at index: 1 error, 2 warning, 3 info, 4 hint; 0 on OOB.
#[no_mangle]
pub extern "C" fn active_diagnostic_severity(set: *const ActiveDiagnosticSet, index: usize) -> u32 {
    with_set(set, 0, |s| {
        s.items.get(index).map(|d| d.severity).unwrap_or(0)
    })
}

/// Span at index into out[4] (line, col, end_line, end_col); false when the
/// diagnostic carries no span or the index is OOB.
#[no_mangle]
pub extern "C" fn active_diagnostic_span(
    set: *const ActiveDiagnosticSet,
    index: usize,
    out: *mut u32,
) -> bool {
    guard(false, || {
        if set.is_null() || out.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return false;
        }
        // SAFETY: live set plus 4 writable u32s per docs/FFI.md section 1.
        let set = unsafe { &*set };
        match set.items.get(index).filter(|d| d.has_span) {
            Some(d) => {
                unsafe {
                    *out.add(0) = d.line;
                    *out.add(1) = d.column;
                    *out.add(2) = d.end_line;
                    *out.add(3) = d.end_column;
                }
                true
            }
            None => false,
        }
    })
}

/// Message string at index, borrowed from the set (null on OOB).
#[no_mangle]
pub extern "C" fn active_diagnostic_message(
    set: *const ActiveDiagnosticSet,
    index: usize,
) -> *const std::os::raw::c_char {
    with_set(set, std::ptr::null(), |s| match s.items.get(index) {
        Some(d) => d.message.as_ptr(),
        None => std::ptr::null(),
    })
}

/// Free a diagnostic set. Null-safe.
#[no_mangle]
pub extern "C" fn active_diagnostics_free(set: *mut ActiveDiagnosticSet) {
    guard((), || {
        if set.is_null() {
            return;
        }
        // SAFETY: live set per docs/FFI.md section 1.
        unsafe {
            drop(Box::from_raw(set));
        }
    })
}

/// Free a value handle. Null-safe.
#[no_mangle]
pub extern "C" fn active_value_free(value: *mut ActiveValue) {
    guard((), || {
        if value.is_null() {
            return;
        }
        // SAFETY: live handle per docs/FFI.md section 1.
        unsafe {
            drop(Box::from_raw(value));
        }
    })
}

/// JSON export with secrets redacted by construction (F-015). Returns an
/// owned string the host frees with `active_string_free`.
#[no_mangle]
pub extern "C" fn active_value_to_json(value: *const ActiveValue) -> *mut std::os::raw::c_char {
    guard(std::ptr::null_mut(), || {
        if value.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return std::ptr::null_mut();
        }
        // SAFETY: live handle per docs/FFI.md section 1.
        let value = unsafe { &*value };
        let json = active_core::convert::value_to_pretty_json(&value.value);
        set_last(ACTIVE_ERR_OK);
        cstring_lossy(&json).into_raw()
    })
}

/// Taint query for hosts (F-015): true exactly for secret values, at any
/// depth only when the value itself is secret material.
#[no_mangle]
pub extern "C" fn active_value_is_secret(value: *const ActiveValue) -> bool {
    guard(false, || {
        if value.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return false;
        }
        // SAFETY: live handle per docs/FFI.md section 1.
        let value = unsafe { &*value };
        matches!(value.value, Value::Secret(_))
    })
}

/// Coarse type code (F-015): 0 null, 1 bool, 2 int, 3 float, 4 string,
/// 5 duration, 6 quantity, 7 array, 8 table, 9 secret; u32::MAX on null.
#[no_mangle]
pub extern "C" fn active_value_type(value: *const ActiveValue) -> u32 {
    guard(u32::MAX, || {
        if value.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return u32::MAX;
        }
        // SAFETY: live handle per docs/FFI.md section 1.
        let value = unsafe { &*value };
        match &value.value {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Int(_) => 2,
            Value::Float(_) => 3,
            Value::String(_) => 4,
            Value::DurationMs(_) => 5,
            Value::Quantity(_, _) => 6,
            Value::Array(_) => 7,
            Value::Table(_) => 8,
            Value::Secret(_) => 9,
        }
    })
}

/// Free a string returned by `active_value_to_json`. Null-safe (Model A).
#[no_mangle]
pub extern "C" fn active_string_free(s: *mut std::os::raw::c_char) {
    guard((), || {
        if s.is_null() {
            return;
        }
        // SAFETY: pointer came from `active_value_to_json` (Model A).
        unsafe {
            drop(std::ffi::CString::from_raw(s));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_constant_is_one() {
        assert_eq!(active_abi_version(), 1);
        assert_eq!(ACTIVE_ABI_VERSION, 1);
    }

    #[test]
    fn check_clean_document_gives_empty_set() {
        let src = b"port = 8080\n";
        let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(!module.is_null());
        let set = active_check(module, std::ptr::null());
        assert!(!set.is_null());
        assert_eq!(active_diagnostics_len(set), 0);
        active_diagnostics_free(set);
        active_module_free(module);
    }

    #[test]
    fn check_broken_units_reports_t001_with_span() {
        let src = b"timeout = 5s + 512MiB\n";
        let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(!module.is_null());
        let set = active_check(module, std::ptr::null());
        assert!(!set.is_null());
        assert!(active_diagnostics_len(set) >= 1);
        let code = unsafe {
            std::ffi::CStr::from_ptr(active_diagnostic_code(set, 0))
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(code, "T001");
        assert_eq!(active_diagnostic_severity(set, 0), 1);
        let mut span = [0u32; 4];
        assert!(active_diagnostic_span(set, 0, span.as_mut_ptr()));
        assert_eq!((span[0], span[1]), (1, 14));
        assert!(active_diagnostic_code(set, 99).is_null());
        assert_eq!(active_diagnostic_severity(set, 99), 0);
        active_diagnostics_free(set);
        active_module_free(module);
    }

    #[test]
    fn eval_json_redacts_secrets_and_taint_reports() {
        let src = b"let pw = secret(\"hunter2\")\n[out]\npass = pw\n";
        let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(!module.is_null());
        let value = active_eval(module, std::ptr::null());
        assert!(!value.is_null());
        assert_eq!(active_value_type(value), 8);
        let json = unsafe {
            std::ffi::CStr::from_ptr(active_value_to_json(value))
                .to_string_lossy()
                .into_owned()
        };
        assert!(json.contains("***SECRET***"), "{json}");
        assert!(!json.contains("hunter2"), "{json}");
        assert!(!active_value_is_secret(value));
        active_string_free(active_value_to_json(value));
        active_value_free(value);
        active_module_free(module);
    }

    #[test]
    fn eval_secret_member_redacts_at_root_table() {
        // Root evaluates to a table; the secret member inside must redact.
        let src = b"pw = secret(\"hunter2\")\n";
        let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(!module.is_null());
        let value = active_eval(module, std::ptr::null());
        assert!(!value.is_null());
        assert!(!active_value_is_secret(value));
        assert_eq!(active_value_type(value), 8);
        let json = unsafe {
            std::ffi::CStr::from_ptr(active_value_to_json(value))
                .to_string_lossy()
                .into_owned()
        };
        assert!(json.contains("***SECRET***"), "{json}");
        assert!(!json.contains("hunter2"), "{json}");
        active_string_free(active_value_to_json(value));
        active_value_free(value);
        active_module_free(module);
    }

    #[test]
    fn bad_budget_size_fails_closed() {
        let src = b"port = 8080\n";
        let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(!module.is_null());
        let bad = ActiveBudget {
            struct_size: 0,
            max_nodes: 0,
            max_depth: 0,
            max_array_len: 0,
            max_keys: 0,
            max_wall_ms: 0,
            max_value_size: 0,
            max_total_value_size: 0,
        };
        let set = active_check(module, &bad);
        assert!(set.is_null());
        assert_eq!(active_last_error(), ACTIVE_ERR_BAD_BUDGET);
        let good = ActiveBudget {
            struct_size: std::mem::size_of::<ActiveBudget>() as u32,
            max_nodes: 0,
            max_depth: 0,
            max_array_len: 0,
            max_keys: 0,
            max_wall_ms: 0,
            max_value_size: 0,
            max_total_value_size: 0,
        };
        let set = active_check(module, &good);
        assert!(!set.is_null());
        assert_eq!(active_diagnostics_len(set), 0);
        active_diagnostics_free(set);
        active_module_free(module);
    }

    #[test]
    fn null_module_null_set_safety() {
        assert!(active_check(std::ptr::null(), std::ptr::null()).is_null());
        assert_eq!(active_last_error(), ACTIVE_ERR_NULL_HANDLE);
        assert!(active_eval(std::ptr::null(), std::ptr::null()).is_null());
        assert_eq!(active_diagnostics_len(std::ptr::null()), 0);
        assert!(active_diagnostic_code(std::ptr::null(), 0).is_null());
        assert!(!active_diagnostic_span(
            std::ptr::null(),
            0,
            std::ptr::null_mut()
        ));
        active_diagnostics_free(std::ptr::null_mut());
        active_value_free(std::ptr::null_mut());
        active_string_free(std::ptr::null_mut());
    }

    #[test]
    fn parse_and_free_roundtrip() {
        let src = b"port = 8080\n";
        let handle = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(!handle.is_null());
        assert_eq!(active_last_error(), ACTIVE_ERR_OK);
        active_module_free(handle);
        active_module_free(std::ptr::null_mut());
    }

    #[test]
    fn invalid_source_gives_null_plus_parse_code() {
        let src = b"[unclosed\n";
        let handle = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(handle.is_null());
        assert_eq!(active_last_error(), ACTIVE_ERR_PARSE);
    }

    #[test]
    fn non_utf8_gives_utf8_code() {
        let src = [0x66, 0x6f, 0x80, 0x6f];
        let handle = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
        assert!(handle.is_null());
        assert_eq!(active_last_error(), ACTIVE_ERR_UTF8);
    }

    #[test]
    fn null_source_gives_null_handle_code() {
        let handle = active_module_parse(std::ptr::null(), 0, std::ptr::null());
        assert!(handle.is_null());
        assert_eq!(active_last_error(), ACTIVE_ERR_NULL_HANDLE);
    }
}
