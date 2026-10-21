//! `active-ffi`: C ABI for embedding Active (docs/FFI.md is the contract).
//!
//! Opaque handles only; every export is panic-isolated; Model A owns every
//! buffer crossing the boundary (free with the matching `active_*_free`).

use active_core::{Diagnostic, Engine, EvalError, Severity, Value};
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
pub const ACTIVE_ERR_BAD_ARGUMENT: i32 = 14;

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

/// Opaque parsed module (F-012/F-016). Stores the SOURCE plus registered
/// host callbacks, not a parsed tree: every check/eval re-parses fresh so
/// callbacks observe the current registrations and no stale state survives
/// across calls. Deterministic for fixed inputs and callbacks.
pub struct ActiveModule {
    source: String,
    include_cb: Option<ActiveCallback>,
    env_cb: Option<ActiveCallback>,
}

/// A registered host callback: C function plus userdata (F-016).
/// The function returns a malloc'd NUL-terminated string the core copies
/// immediately and frees with `free`, or null for miss. Runs on the calling
/// thread and must not re-enter evaluation on the same module.
#[derive(Clone, Copy)]
struct ActiveCallback {
    f: unsafe extern "C" fn(
        *const std::os::raw::c_char,
        *mut std::ffi::c_void,
    ) -> *mut std::os::raw::c_char,
    userdata: *mut std::ffi::c_void,
}

/// Host callback type for include and env lookup (F-016).
pub type ActiveLookupFn = unsafe extern "C" fn(
    *const std::os::raw::c_char,
    *mut std::ffi::c_void,
) -> *mut std::os::raw::c_char;

/// ABI version constant for bindings to check at load time.
#[no_mangle]
/// # Safety
///
/// No preconditions; always safe to call.
pub unsafe extern "C" fn active_abi_version() -> u32 {
    ACTIVE_ABI_VERSION
}

/// Code from the last failed call on this thread (OK when clean).
#[no_mangle]
/// # Safety
///
/// No preconditions; always safe to call.
pub unsafe extern "C" fn active_last_error() -> i32 {
    guard(ACTIVE_ERR_PANIC_CAUGHT, || LAST_ERROR.with(|c| c.get()))
}

/// Syntax-only parse for handle creation: includes recorded, never
/// resolved, so documents with includes still get a handle and the host
/// can register a loader afterwards. Mirrors active-fmt's preservation
/// mode; resolution happens per check/eval call.
fn parse_record_mode(text: &str) -> Result<(), Vec<active_core::Diagnostic>> {
    let options = active_core::ParseOptions {
        enable_includes: true,
        resolve_includes: false,
        ..active_core::ParseOptions::default()
    };
    active_core::parse_with(text, "ffi", options).map(|_| ())
}

/// Register the host include loader on a live handle (F-016). Null `cb`
/// unregisters (legacy hermetic behavior returns). Null module is a
/// no-op miss recorded as NULL_HANDLE for the next `active_last_error`.
#[no_mangle]
/// # Safety
///
/// The module must be a live handle from `active_module_parse` on the
/// calling thread. The callback and userdata must outlive every later
/// check/eval on this handle, and the callback must not re-enter
/// evaluation on the same module. Buffers the callback returns must
/// be malloc-allocated (the core copies then frees with `free`).
pub unsafe extern "C" fn active_set_include_loader(
    module: *mut ActiveModule,
    cb: Option<ActiveLookupFn>,
    userdata: *mut std::ffi::c_void,
) {
    guard((), || {
        if module.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return;
        }
        // SAFETY: live handle per docs/FFI.md section 1; registration
        // mutates only this handle on the calling thread (section 6).
        let handle = unsafe { &mut *module };
        handle.include_cb = cb.map(|f| ActiveCallback { f, userdata });
        set_last(ACTIVE_ERR_OK);
    })
}

/// Register the host env lookup on a live handle (F-016). Semantics mirror
/// the snapshot map: consulted on miss before defaults, charged equally.
/// Null `cb` unregisters.
#[no_mangle]
/// # Safety
///
/// Same obligations as `active_set_include_loader`: live handle,
/// calling thread, outliving callback and userdata, no re-entrancy,
/// malloc-allocated return buffers.
pub unsafe extern "C" fn active_set_env_lookup(
    module: *mut ActiveModule,
    cb: Option<ActiveLookupFn>,
    userdata: *mut std::ffi::c_void,
) {
    guard((), || {
        if module.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return;
        }
        // SAFETY: live handle, calling thread (docs/FFI.md sections 1, 6).
        let handle = unsafe { &mut *module };
        handle.env_cb = cb.map(|f| ActiveCallback { f, userdata });
        set_last(ACTIVE_ERR_OK);
    })
}

/// Parse UTF-8 source bytes into an owned module handle (null on error;
/// detail via `active_last_error`). Null-safe on the source pointer.
#[no_mangle]
/// # Safety
///
/// When `source` is non-null, `len` bytes starting at `source` must be
/// readable for this call. `options` is reserved and must be null.
pub unsafe extern "C" fn active_module_parse(
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
        match parse_record_mode(text) {
            Ok(()) => {
                set_last(ACTIVE_ERR_OK);
                Box::into_raw(Box::new(ActiveModule {
                    source: text.to_owned(),
                    include_cb: None,
                    env_cb: None,
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
/// # Safety
///
/// The handle must come from `active_module_parse` and must not have
/// been freed before; null is accepted. Use confines to one thread.
pub unsafe extern "C" fn active_module_free(handle: *mut ActiveModule) {
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
    // Hermetic construction (never snapshot-then-discard): on platforms
    // without a process environment the ambient snapshot aborts, and the
    // host ambient must never leak across the ABI regardless. Host env
    // arrives only through the F-016 lookup callback.
    Engine::hermetic()
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

/// Take ownership of a host callback buffer (F-016 protocol): copy the
/// NUL-terminated bytes immediately, then free with libc `free`. The host
/// allocated with malloc/calloc (documented in docs/FFI.md); null means
/// miss. Runs inside the calling thread during parse/eval only.
unsafe fn take_buffer(ptr: *mut std::os::raw::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: host returned a malloc'd NUL-terminated string per protocol.
    let bytes = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_bytes();
    let owned = bytes.to_vec();
    // SAFETY: paired free for the host malloc above.
    unsafe {
        libc::free(ptr as *mut std::ffi::c_void);
    }
    String::from_utf8(owned).ok()
}

fn c_string(arg: &str) -> std::ffi::CString {
    cstring_lossy(arg)
}

/// Build parse options from a handle: loader present means includes
/// resolve through the host callback (F-016); absent means legacy
/// hermetic string parsing (includes fail E015, as before).
fn handle_options(handle: &ActiveModule) -> active_core::ParseOptions {
    let include_loader = handle.include_cb.map(|cb| {
        // `arc_with_non_send_sync` allowed with justification: the closure
        // captures only the calling-thread callback plus userdata and lives
        // inside a thread-confined handle (docs/FFI.md section 6); Arc is
        // intra-thread cloning into the parser, never cross-thread sharing.
        #[allow(clippy::arc_with_non_send_sync)]
        let loader: active_core::IncludeLoader = std::sync::Arc::new(move |path: &str| {
            let cpath = c_string(path);
            // SAFETY: calling-thread callback per docs/FFI.md section 6;
            // must not re-enter evaluation on this module.
            let raw = unsafe { (cb.f)(cpath.as_ptr(), cb.userdata) };
            // SAFETY: ownership transfer per protocol (take_buffer docs).
            match unsafe { take_buffer(raw) } {
                Some(content) => Ok(content),
                None => Err("host include loader returned null (miss)".to_string()),
            }
        });
        loader
    });
    active_core::ParseOptions {
        enable_includes: include_loader.is_some(),
        resolve_includes: true,
        include_loader,
        ..active_core::ParseOptions::default()
    }
}

/// Build a hermetic engine from a handle: empty ambient plus the lazy host
/// env callback when registered (F-016). Callback misses fall through to
/// defaults exactly like snapshot misses.
fn handle_engine(handle: &ActiveModule) -> Engine {
    let mut engine = hermetic_engine();
    if let Some(cb) = handle.env_cb {
        // Same justification as above: thread-confined handle, Arc for
        // intra-thread cloning into the engine, never shared across threads.
        #[allow(clippy::arc_with_non_send_sync)]
        let lookup: active_core::EnvLookup = std::sync::Arc::new(move |name: &str| {
            let cname = c_string(name);
            // SAFETY: calling-thread callback per docs/FFI.md section 6.
            let raw = unsafe { (cb.f)(cname.as_ptr(), cb.userdata) };
            // SAFETY: ownership transfer per protocol; non-UTF8 counts as
            // miss (ambient entries must be Unicode, same rule as Engine).
            unsafe { take_buffer(raw) }
        });
        engine = engine.with_env_fallback(lookup);
    }
    engine
}

/// Parse fresh from a handle (F-016): every check/eval sees current
/// callback registrations. Parse errors come back as snapshots.
fn parse_handle(handle: &ActiveModule) -> Result<active_core::Module, Vec<DiagSnapshot>> {
    let options = handle_options(handle);
    match active_core::parse_with(&handle.source, "ffi", options) {
        Ok(m) => Ok(m),
        Err(diags) => Err(diags.iter().map(snapshot).collect()),
    }
}

/// Engine from a handle plus budget (F-016): hermetic ambient, lazy host
/// env callback, validated budget struct.
fn build_engine(handle: &ActiveModule, budget: *const ActiveBudget) -> Result<Engine, i32> {
    let mut engine = handle_engine(handle);
    apply_budget(&mut engine, budget)?;
    Ok(engine)
}

fn collect_check(
    handle: &ActiveModule,
    budget: *const ActiveBudget,
) -> Result<Vec<DiagSnapshot>, i32> {
    let module = match parse_handle(handle) {
        Ok(m) => m,
        Err(diags) => return Ok(diags),
    };
    let engine = build_engine(handle, budget)?;
    match engine.evaluate(&module) {
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
/// # Safety
///
/// The module must be a live handle on the calling thread. The budget
/// pointer must be null or point to a valid `ActiveBudget` whose
/// struct_size field was set by the caller.
pub unsafe extern "C" fn active_check(
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
/// # Safety
///
/// Same obligations as `active_check`: live handle, calling thread,
/// null-or-valid budget with a truthful size word.
pub unsafe extern "C" fn active_eval(
    module: *const ActiveModule,
    budget: *const ActiveBudget,
) -> *mut ActiveValue {
    guard(std::ptr::null_mut(), || {
        if module.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return std::ptr::null_mut();
        }
        // SAFETY: live handle per docs/FFI.md section 1.
        let handle = unsafe { &*module };
        let parsed = match parse_handle(handle) {
            Ok(m) => m,
            Err(_) => {
                set_last(ACTIVE_ERR_PARSE);
                return std::ptr::null_mut();
            }
        };
        let engine = match build_engine(handle, budget) {
            Ok(e) => e,
            Err(_) => return std::ptr::null_mut(),
        };
        match engine.evaluate(&parsed) {
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
/// # Safety
///
/// The set must be a live set from `active_check`/`active_validate`;
/// null yields 0 without dereference.
pub unsafe extern "C" fn active_diagnostics_len(set: *const ActiveDiagnosticSet) -> usize {
    with_set(set, 0, |s| s.items.len())
}

/// Code string at index, borrowed from the set (null on OOB).
#[no_mangle]
/// # Safety
///
/// Live set; the returned pointer borrows the set and dies with it.
/// Out-of-bounds index yields null without dereference.
pub unsafe extern "C" fn active_diagnostic_code(
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
/// # Safety
///
/// Live set; out-of-bounds index yields 0 without dereference.
pub unsafe extern "C" fn active_diagnostic_severity(
    set: *const ActiveDiagnosticSet,
    index: usize,
) -> u32 {
    with_set(set, 0, |s| {
        s.items.get(index).map(|d| d.severity).unwrap_or(0)
    })
}

/// Span at index into out[4] (line, col, end_line, end_col); false when the
/// diagnostic carries no span or the index is OOB.
#[no_mangle]
/// # Safety
///
/// Live set plus 4 writable `u32` slots at `out` when non-null;
/// null `out` fails closed. Out-of-bounds or spanless index yields
/// false without dereference.
pub unsafe extern "C" fn active_diagnostic_span(
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
/// # Safety
///
/// Live set; the returned pointer borrows the set and dies with it.
/// Out-of-bounds index yields null without dereference.
pub unsafe extern "C" fn active_diagnostic_message(
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
/// # Safety
///
/// The set must come from `active_check`/`active_validate` and must
/// not have been freed before; null is accepted.
pub unsafe extern "C" fn active_diagnostics_free(set: *mut ActiveDiagnosticSet) {
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
/// # Safety
///
/// The value must come from `active_eval` and must not have been
/// freed before; null is accepted.
pub unsafe extern "C" fn active_value_free(value: *mut ActiveValue) {
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
/// # Safety
///
/// The value must be a live handle. The returned string is Model A
/// owned: free exactly once with `active_string_free`.
pub unsafe extern "C" fn active_value_to_json(
    value: *const ActiveValue,
) -> *mut std::os::raw::c_char {
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
/// # Safety
///
/// The value must be a live handle; null yields false without
/// dereference.
pub unsafe extern "C" fn active_value_is_secret(value: *const ActiveValue) -> bool {
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
/// # Safety
///
/// The value must be a live handle; null yields `u32::MAX` without
/// dereference.
pub unsafe extern "C" fn active_value_type(value: *const ActiveValue) -> u32 {
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

fn is_schema_code(code: &str) -> bool {
    matches!(code, "T002" | "T003" | "T004" | "T005" | "T006")
}

/// Validate a module against its in-file schema, returning schema
/// diagnostics (F-020): the generator feedback signal without parse/type
/// noise. Runs the single shipped validator through evaluation and keeps
/// T002-T006; a fatal evaluation failure that prevents validation surfaces
/// with its true code instead of a lying empty set (empty means
/// schema-clean AND evaluable: run `active_check` for the full verdict).
/// Cross-document schemas ride includes (F-016 loader).
#[no_mangle]
/// # Safety
///
/// Same obligations as `active_check`: live handle, calling thread,
/// null-or-valid budget with a truthful size word.
pub unsafe extern "C" fn active_validate(
    module: *const ActiveModule,
    budget: *const ActiveBudget,
) -> *mut ActiveDiagnosticSet {
    guard(std::ptr::null_mut(), || {
        if module.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return std::ptr::null_mut();
        }
        // SAFETY: live handle per docs/FFI.md section 1.
        let handle = unsafe { &*module };
        let parsed = match parse_handle(handle) {
            Ok(m) => m,
            Err(_) => {
                set_last(ACTIVE_ERR_PARSE);
                return std::ptr::null_mut();
            }
        };
        let engine = match build_engine(handle, budget) {
            Ok(e) => e,
            Err(_) => return std::ptr::null_mut(),
        };
        let typed: Vec<DiagSnapshot> = match engine.evaluate(&parsed) {
            Ok((_, diags)) => diags
                .iter()
                .filter(|d| is_schema_code(&d.code))
                .map(snapshot)
                .collect(),
            Err(EvalError::ValidationFailed { diagnostics }) => {
                diagnostics.iter().map(snapshot).collect()
            }
            Err(e) => vec![snapshot_error(&e)],
        };
        set_last(ACTIVE_ERR_OK);
        Box::into_raw(Box::new(ActiveDiagnosticSet { items: typed }))
    })
}

/// Format a module to canonical text (F-021): style 0 brace, 1 indent,
/// 2 preserve; other values fail closed with BAD_ARGUMENT. Powers the
/// playground brace/indent toggle; hosts never reimplement layout.
/// Includes stay unresolved (format preserves statements, like active-fmt).
#[no_mangle]
/// # Safety
///
/// The module must be a live handle. The returned string is Model A
/// owned: free exactly once with `active_string_free`.
pub unsafe extern "C" fn active_format(
    module: *const ActiveModule,
    style: u32,
) -> *mut std::os::raw::c_char {
    guard(std::ptr::null_mut(), || {
        if module.is_null() {
            set_last(ACTIVE_ERR_NULL_HANDLE);
            return std::ptr::null_mut();
        }
        let style = match style {
            0 => active_core::Style::Brace,
            1 => active_core::Style::Indent,
            2 => active_core::Style::Preserve,
            _ => {
                set_last(ACTIVE_ERR_BAD_ARGUMENT);
                return std::ptr::null_mut();
            }
        };
        // SAFETY: live handle per docs/FFI.md section 1.
        let handle = unsafe { &*module };
        let options = active_core::ParseOptions {
            enable_includes: true,
            resolve_includes: false,
            ..active_core::ParseOptions::default()
        };
        match active_core::parse_with(&handle.source, "ffi", options) {
            Ok(parsed) => {
                set_last(ACTIVE_ERR_OK);
                cstring_lossy(&active_core::fmt::format(&parsed, style)).into_raw()
            }
            Err(_) => {
                set_last(ACTIVE_ERR_PARSE);
                std::ptr::null_mut()
            }
        }
    })
}

/// Free a string returned by `active_value_to_json`. Null-safe (Model A).
#[no_mangle]
/// # Safety
///
/// The pointer must come from `active_value_to_json`/`active_format`
/// and must not have been freed before; null is accepted.
pub unsafe extern "C" fn active_string_free(s: *mut std::os::raw::c_char) {
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
        // SAFETY: no-deref entry point; always safe to call.
        assert_eq!(unsafe { active_abi_version() }, 1);
        assert_eq!(ACTIVE_ABI_VERSION, 1);
    }

    #[test]
    fn huge_source_is_bounded_not_fatal() {
        // H-001: a 2MB document must diagnose or evaluate, never hang or
        // trap. Budgets bound the walk; the call returns either way.
        // Miri executes every byte with shadow-memory instrumentation, so
        // the same linear scan costs ~10,000x wall time there; the
        // property under test is boundedness, not size, so Miri runs the
        // identical path on a 20KB document (native scaling is linear:
        // 20KB/200KB/2MB take 0.03/0.06/0.25s outside Miri).
        #[cfg(miri)]
        let digits = 20_000;
        #[cfg(not(miri))]
        let digits = 2_000_000;
        let mut src = vec![b'x', b' ', b'=', b' '];
        src.extend(std::iter::repeat_n(b'1', digits));
        src.push(b'\n');
        let module = unsafe { active_module_parse(src.as_ptr(), src.len(), std::ptr::null()) };
        // Parse may succeed or fail; either way the call returned.
        if module.is_null() {
            return;
        }
        let set = unsafe { active_check(module, std::ptr::null()) };
        assert!(!set.is_null());
        unsafe {
            active_diagnostics_free(set);
            active_module_free(module);
        }
    }

    #[test]
    fn thread_confined_full_cycle_on_spawned_thread() {
        // G-018: modules live on one thread at a time. A complete
        // parse/check/eval/free cycle on a spawned thread proves
        // thread-confined use; sharing without sync stays a host bug.
        let done = std::thread::spawn(|| unsafe {
            let src = b"port = 8080\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            let set = active_check(module, std::ptr::null());
            assert!(!set.is_null());
            assert_eq!(active_diagnostics_len(set), 0);
            let value = active_eval(module, std::ptr::null());
            assert!(!value.is_null());
            // Single ownership: the returned string is freed exactly once
            // (Model A); calling the getter twice would leak the first copy.
            let json_ptr = active_value_to_json(value);
            let json = std::ffi::CStr::from_ptr(json_ptr)
                .to_string_lossy()
                .into_owned();
            assert!(json.contains("8080"), "{json}");
            active_string_free(json_ptr);
            active_value_free(value);
            active_diagnostics_free(set);
            active_module_free(module);
        });
        done.join().expect("confined cycle completes");
    }

    static INCLUDE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static ENV_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn malloc_str(s: &[u8]) -> *mut std::os::raw::c_char {
        // SAFETY: test host allocates per protocol (malloc + NUL).
        unsafe {
            let ptr = libc::malloc(s.len() + 1) as *mut u8;
            assert!(!ptr.is_null());
            std::ptr::copy_nonoverlapping(s.as_ptr(), ptr, s.len());
            *ptr.add(s.len()) = 0;
            ptr as *mut std::os::raw::c_char
        }
    }

    unsafe extern "C" fn include_host(
        path: *const std::os::raw::c_char,
        _userdata: *mut std::ffi::c_void,
    ) -> *mut std::os::raw::c_char {
        INCLUDE_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // SAFETY: core passes a valid NUL-terminated path per protocol.
        let path = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy();
        if path.ends_with("lib.active") {
            malloc_str(b"answer = 42\n")
        } else {
            std::ptr::null_mut()
        }
    }

    unsafe extern "C" fn env_host(
        key: *const std::os::raw::c_char,
        _userdata: *mut std::ffi::c_void,
    ) -> *mut std::os::raw::c_char {
        ENV_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // SAFETY: core passes a valid NUL-terminated key per protocol.
        let key = unsafe { std::ffi::CStr::from_ptr(key) }.to_string_lossy();
        if key == "HOST_PORT" {
            malloc_str(b"9090")
        } else {
            std::ptr::null_mut()
        }
    }

    unsafe extern "C" fn garbage_host(
        _path: *const std::os::raw::c_char,
        _userdata: *mut std::ffi::c_void,
    ) -> *mut std::os::raw::c_char {
        malloc_str(&[0x66, 0x6f, 0x80, 0x6f])
    }

    #[test]
    fn include_loader_called_during_check_and_resolves() {
        unsafe {
            INCLUDE_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
            let src = b"include \"lib.active\"\n[out]\nval = 1\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            active_set_include_loader(module, Some(include_host), std::ptr::null_mut());
            let set = active_check(module, std::ptr::null());
            assert!(!set.is_null());
            let n = active_diagnostics_len(set);
            let mut dump = String::new();
            for i in 0..n {
                let code = std::ffi::CStr::from_ptr(active_diagnostic_code(set, i))
                    .to_string_lossy()
                    .into_owned();
                let msg = std::ffi::CStr::from_ptr(active_diagnostic_message(set, i))
                    .to_string_lossy()
                    .into_owned();
                dump.push_str(&format!("{code}: {msg}\n"));
            }
            assert_eq!(n, 0, "diags:\n{dump}");
            assert!(
                INCLUDE_CALLS.load(std::sync::atomic::Ordering::SeqCst) >= 1,
                "loader must run during check"
            );
            active_diagnostics_free(set);
            active_module_free(module);
        }
    }

    #[test]
    fn include_loader_garbage_bytes_refuse_loudly() {
        // H-001: non-UTF8 loader output must diagnose (E017), never crash
        // or smuggle bytes past decoding.
        unsafe {
            let src = b"include \"bin.active\"\n[out]\nval = 1\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            active_set_include_loader(module, Some(garbage_host), std::ptr::null_mut());
            let set = active_check(module, std::ptr::null());
            assert!(!set.is_null());
            assert!(active_diagnostics_len(set) >= 1);
            let code = std::ffi::CStr::from_ptr(active_diagnostic_code(set, 0))
                .to_string_lossy()
                .into_owned();
            assert_eq!(code, "E017");
            active_diagnostics_free(set);
            active_module_free(module);
        }
    }

    #[test]
    fn include_loader_miss_surfaces_e017() {
        unsafe {
            let src = b"include \"nope.active\"\n[out]\nval = 1\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            active_set_include_loader(module, Some(include_host), std::ptr::null_mut());
            let set = active_check(module, std::ptr::null());
            assert!(!set.is_null());
            assert!(active_diagnostics_len(set) >= 1);
            let code = std::ffi::CStr::from_ptr(active_diagnostic_code(set, 0))
                .to_string_lossy()
                .into_owned();
            assert_eq!(code, "E017");
            active_diagnostics_free(set);
            active_module_free(module);
        }
    }

    #[test]
    fn without_loader_includes_still_fail() {
        unsafe {
            let src = b"include \"lib.active\"\n[out]\nval = 1\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            let set = active_check(module, std::ptr::null());
            assert!(!set.is_null());
            assert!(active_diagnostics_len(set) >= 1);
            active_diagnostics_free(set);
            active_module_free(module);
        }
    }

    #[test]
    fn env_lookup_called_lazily_and_miss_falls_to_default() {
        unsafe {
            ENV_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
            let src = b"port = env.HOST_PORT ?: 1\nname = env.NOPE_XYZZY ?: \"dflt\"\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            active_set_env_lookup(module, Some(env_host), std::ptr::null_mut());
            let value = active_eval(module, std::ptr::null());
            assert!(!value.is_null());
            // Single ownership: the returned string is freed exactly once
            // (Model A); calling the getter twice would leak the first copy.
            let json_ptr = active_value_to_json(value);
            let json = std::ffi::CStr::from_ptr(json_ptr)
                .to_string_lossy()
                .into_owned();
            assert!(json.contains("9090"), "{json}");
            assert!(json.contains("dflt"), "{json}");
            assert!(
                ENV_CALLS.load(std::sync::atomic::Ordering::SeqCst) >= 2,
                "lookup must run during eval, including for the miss"
            );
            active_string_free(json_ptr);
            active_value_free(value);
            active_module_free(module);
        }
    }

    #[test]
    fn validate_closed_schema_refusal_proven() {
        unsafe {
            let src = b"schema = { srv = Table { host = String, closed = true } }\n[srv]\nhost = \"h\"\nextra = 1\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            let set = active_validate(module, std::ptr::null());
            assert!(!set.is_null());
            assert!(active_diagnostics_len(set) >= 1);
            let code = std::ffi::CStr::from_ptr(active_diagnostic_code(set, 0))
                .to_string_lossy()
                .into_owned();
            assert_eq!(code, "T005");
            active_diagnostics_free(set);
            active_module_free(module);
        }
    }

    #[test]
    fn validate_surfaces_fatal_instead_of_lying_empty() {
        unsafe {
            // A unit mismatch aborts evaluation: validate reports the fatal
            // T001 rather than an empty (lying) schema-clean set.
            let src = b"timeout = 5s + 512MiB\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            let set = active_validate(module, std::ptr::null());
            assert!(!set.is_null());
            assert_eq!(active_diagnostics_len(set), 1);
            let code = std::ffi::CStr::from_ptr(active_diagnostic_code(set, 0))
                .to_string_lossy()
                .into_owned();
            assert_eq!(code, "T001");
            active_diagnostics_free(set);
            active_module_free(module);
        }
    }

    #[test]
    fn format_brace_indent_roundtrip_and_bad_style() {
        unsafe {
            let src = b"[server]\nport = 8080\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            // Single ownership: each returned string is freed exactly once
            // (Model A); calling the getter again would leak the earlier copy.
            let brace_ptr = active_format(module, 0);
            let brace = std::ffi::CStr::from_ptr(brace_ptr)
                .to_string_lossy()
                .into_owned();
            assert!(brace.contains("[server]"), "{brace}");
            let indent_ptr = active_format(module, 1);
            let indent = std::ffi::CStr::from_ptr(indent_ptr)
                .to_string_lossy()
                .into_owned();
            assert!(indent.contains("server:"), "{indent}");
            assert!(active_format(module, 99).is_null());
            assert_eq!(active_last_error(), ACTIVE_ERR_BAD_ARGUMENT);
            active_string_free(brace_ptr);
            active_string_free(indent_ptr);
            active_module_free(module);
        }
    }

    #[test]
    fn check_clean_document_gives_empty_set() {
        unsafe {
            let src = b"port = 8080\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            let set = active_check(module, std::ptr::null());
            assert!(!set.is_null());
            assert_eq!(active_diagnostics_len(set), 0);
            active_diagnostics_free(set);
            active_module_free(module);
        }
    }

    #[test]
    fn check_broken_units_reports_t001_with_span() {
        unsafe {
            let src = b"timeout = 5s + 512MiB\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            let set = active_check(module, std::ptr::null());
            assert!(!set.is_null());
            assert!(active_diagnostics_len(set) >= 1);
            let code = std::ffi::CStr::from_ptr(active_diagnostic_code(set, 0))
                .to_string_lossy()
                .into_owned();
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
    }

    #[test]
    fn eval_json_redacts_secrets_and_taint_reports() {
        unsafe {
            let src = b"let pw = secret(\"hunter2\")\n[out]\npass = pw\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            let value = active_eval(module, std::ptr::null());
            assert!(!value.is_null());
            assert_eq!(active_value_type(value), 8);
            // Single ownership: the returned string is freed exactly once
            // (Model A); calling the getter twice would leak the first copy.
            let json_ptr = active_value_to_json(value);
            let json = std::ffi::CStr::from_ptr(json_ptr)
                .to_string_lossy()
                .into_owned();
            assert!(json.contains("***SECRET***"), "{json}");
            assert!(!json.contains("hunter2"), "{json}");
            assert!(!active_value_is_secret(value));
            active_string_free(json_ptr);
            active_value_free(value);
            active_module_free(module);
        }
    }

    #[test]
    fn eval_secret_member_redacts_at_root_table() {
        unsafe {
            // Root evaluates to a table; the secret member inside must redact.
            let src = b"pw = secret(\"hunter2\")\n";
            let module = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!module.is_null());
            let value = active_eval(module, std::ptr::null());
            assert!(!value.is_null());
            assert!(!active_value_is_secret(value));
            assert_eq!(active_value_type(value), 8);
            // Single ownership: the returned string is freed exactly once
            // (Model A); calling the getter twice would leak the first copy.
            let json_ptr = active_value_to_json(value);
            let json = std::ffi::CStr::from_ptr(json_ptr)
                .to_string_lossy()
                .into_owned();
            assert!(json.contains("***SECRET***"), "{json}");
            assert!(!json.contains("hunter2"), "{json}");
            active_string_free(json_ptr);
            active_value_free(value);
            active_module_free(module);
        }
    }

    #[test]
    fn bad_budget_size_fails_closed() {
        unsafe {
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
    }

    #[test]
    fn null_module_null_set_safety() {
        unsafe {
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
    }

    #[test]
    fn parse_and_free_roundtrip() {
        unsafe {
            let src = b"port = 8080\n";
            let handle = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(!handle.is_null());
            assert_eq!(active_last_error(), ACTIVE_ERR_OK);
            active_module_free(handle);
            active_module_free(std::ptr::null_mut());
        }
    }

    #[test]
    fn invalid_source_gives_null_plus_parse_code() {
        unsafe {
            let src = b"[unclosed\n";
            let handle = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(handle.is_null());
            assert_eq!(active_last_error(), ACTIVE_ERR_PARSE);
        }
    }

    #[test]
    fn non_utf8_gives_utf8_code() {
        unsafe {
            let src = [0x66, 0x6f, 0x80, 0x6f];
            let handle = active_module_parse(src.as_ptr(), src.len(), std::ptr::null());
            assert!(handle.is_null());
            assert_eq!(active_last_error(), ACTIVE_ERR_UTF8);
        }
    }

    #[test]
    fn null_source_gives_null_handle_code() {
        unsafe {
            let handle = active_module_parse(std::ptr::null(), 0, std::ptr::null());
            assert!(handle.is_null());
            assert_eq!(active_last_error(), ACTIVE_ERR_NULL_HANDLE);
        }
    }
}
