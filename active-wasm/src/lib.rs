//! `active-wasm`: the Active evaluator for WebAssembly hosts (W-001).
//!
//! Same logical API as the C ABI, minus callbacks (WASM hosts pass
//! self-contained documents; includes and env resolve through the host
//! before calling in, exactly like the playground shell does). Linear
//! memory discipline: the host writes UTF-8 source via `active_alloc`,
//! calls `active_eval_json`, reads the returned envelope, and frees both
//! with `active_free`. Panics become JSON error envelopes, never traps.
//!
//! Deliberate non-duplication note: the parse/evaluate/collect
//! orchestration mirrors active-ffi by shape but differs in memory
//! discipline (linear-memory envelope vs handle/ownership transfer).
//! A shared internal crate waits for a third embedding (rule of three).

use active_core::diag::{Diagnostic, Severity};
use active_core::{parse, Engine, EvalError};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::slice;
use std::str;

#[cfg(target_arch = "wasm32")]
mod wasm_random {
    /// getrandom custom backend (see Cargo.toml): wasm32-unknown-unknown
    /// has no OS entropy without JS imports, and this module ships zero
    /// imports by design. Fail loud; the core memo falls back and keygen
    /// errors honestly. Registration affects unsupported targets only.
    fn unavailable(_buf: &mut [u8]) -> Result<(), getrandom::Error> {
        Err(getrandom::Error::UNSUPPORTED)
    }

    getrandom::register_custom_getrandom!(unavailable);
}

use std::cell::RefCell;
use std::collections::HashMap;

// Live-buffer registry: every pointer handed to the host keys its exact
// owning Vec, so `active_free` drops the true allocation (length AND
// capacity) instead of reconstructing a guessed layout. Reconstructing
// `Vec::from_raw_parts(ptr, 0, len)` would free the wrong size whenever
// capacity exceeded length (heap corruption; caught by the secret-envelope
// case in node smoke). Single-threaded by construction (WASM has one
// thread here; native tests are per-thread via thread_local).
thread_local! {
    static LIVE: RefCell<HashMap<usize, Vec<u8>>> = RefCell::new(HashMap::new());
}

fn retain(mut bytes: Vec<u8>) -> (*mut u8, usize) {
    let len = bytes.len();
    let ptr = bytes.as_mut_ptr();
    LIVE.with(|live| {
        live.borrow_mut().insert(ptr as usize, bytes);
    });
    (ptr, len)
}

fn release(ptr: *mut u8) -> bool {
    if ptr.is_null() {
        return false;
    }
    LIVE.with(|live| live.borrow_mut().remove(&(ptr as usize)).is_some())
}

/// Length of the live allocation starting at `ptr`, when the host owns one.
/// Used by byte-taking exports to reject an out-of-bounds read instead of
/// constructing a slice past its allocation.
fn live_len(ptr: *const u8) -> Option<usize> {
    LIVE.with(|live| live.borrow().get(&(ptr as usize)).map(Vec::len))
}

/// Why a host-provided source slice must be refused, if it must be. A
/// length beyond the live allocation is UB in `from_raw_parts`, not a
/// catchable panic, so every byte-taking export checks it first. A null
/// pointer is the caller's separate error.
fn source_bounds_error(src_ptr: *const u8, src_len: usize) -> Option<&'static str> {
    if src_len > MAX_ALLOC_BYTES {
        return Some("source length exceeds the transport cap");
    }
    match live_len(src_ptr) {
        Some(allocated) if src_len <= allocated => None,
        _ => Some("source length exceeds the live allocation"),
    }
}

/// Largest single allocation the transport accepts (LANG-C-018): 64 MiB,
/// four times the default parse input cap and far above any legitimate
/// source buffer. `active_alloc(size)` is capped at 67108864 bytes; larger
/// requests return null instead of allocating.
pub const MAX_ALLOC_BYTES: usize = 64 * 1024 * 1024;

/// Allocate `size` bytes of linear memory for the host to fill. Free with
/// `active_free` using the same pointer. `size == 0` still returns a
/// distinct, free-able allocation (a zero-length `Vec`'s dangling pointer
/// would alias every other zero-length request).
///
/// `active_alloc(size)` is capped at 67108864 bytes (64 MiB); larger
/// requests return null instead of allocating, so a bad length cannot abort
/// the instance.
#[no_mangle]
pub extern "C" fn active_alloc(size: usize) -> *mut u8 {
    if size > MAX_ALLOC_BYTES {
        return std::ptr::null_mut();
    }
    let len = size.max(1);
    // `try_reserve_exact` reports allocation failure instead of aborting
    // the instance; the resize below uses the reserved capacity.
    let mut bytes: Vec<u8> = Vec::new();
    if bytes.try_reserve_exact(len).is_err() {
        return std::ptr::null_mut();
    }
    bytes.resize(len, 0);
    let (ptr, _) = retain(bytes);
    ptr
}

/// Free memory previously returned by `active_alloc`/`active_eval_json`.
/// Unknown pointers are ignored in every build profile (a host bug, never
/// a trap: a release build already ignored them, so a debug-only assert
/// made the two profiles disagree). Null is accepted. The registry owns
/// the length, so `size` is advisory; a mismatch frees the registered
/// buffer rather than trapping.
#[no_mangle]
pub extern "C" fn active_free(ptr: *mut u8, size: usize) {
    if ptr.is_null() {
        return;
    }
    let _freed = release(ptr);
    let _ = size;
}

fn severity_str(s: &Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

/// Structured diagnostic items for the envelope: code, message, severity,
/// and span (line/column/end_line/end_column, or null when spanless).
/// The legacy `diagnostics` text line is unchanged; `items` lets hosts
/// compare codes AND spans strictly without parsing prose.
fn diag_items(diags: &[Diagnostic]) -> String {
    let parts: Vec<String> = diags
        .iter()
        .map(|d| {
            let span = match &d.span {
                Some(s) => format!(
                    "{{\"line\":{},\"column\":{},\"end_line\":{},\"end_column\":{}}}",
                    s.line, s.column, s.end_line, s.end_column
                ),
                None => "null".to_string(),
            };
            format!(
                "{{\"code\":{},\"message\":{},\"severity\":\"{}\",\"span\":{}}}",
                serde_json::json!(d.code),
                serde_json::json!(d.message),
                severity_str(&d.severity),
                span
            )
        })
        .collect();
    format!("[{}]", parts.join(","))
}

/// Whether this build enforces a wall-clock deadline (LANG-C-012).
///
/// On wasm32 there is no monotonic clock, so evaluation runs without a time
/// backstop. The envelope reports it on every response so a host can detect
/// the degradation instead of discovering it under load.
fn wall_supported() -> bool {
    active_core::Budget::wall_supported()
}

fn envelope_ok(json: &str, diagnostics: &str, items: &str) -> String {
    let mut out = String::from("{\"ok\":true,\"wall_supported\":");
    out.push_str(if wall_supported() { "true" } else { "false" });
    out.push_str(",\"json\":");
    out.push_str(json);
    out.push_str(",\"diagnostics\":");
    out.push_str(diagnostics);
    out.push_str(",\"items\":");
    out.push_str(items);
    out.push('}');
    out
}

fn envelope_err_diagnostics(diagnostics: &str, items: &str) -> String {
    // Real diagnostic lines already carry their codes; no synthetic prefix
    // (a wrapper code here would pollute code-set comparisons downstream).
    format!(
        "{{\"ok\":false,\"wall_supported\":{},\"json\":\"\",\"diagnostics\":{},\"items\":{}}}",
        wall_supported(),
        serde_json::json!(diagnostics),
        items
    )
}

fn envelope_err(code: &str, message: &str, span: &str) -> String {
    // Bare codes are always [A-Z0-9]+ by construction; the message is
    // escaped, and span arrives pre-rendered (object or null).
    format!(
        "{{\"ok\":false,\"wall_supported\":{},\"json\":\"\",\"diagnostics\":\"[{code}] {message}\",\"items\":[{{\"code\":\"{code}\",\"message\":{},\"severity\":\"error\",\"span\":{span}}}]}}",
        wall_supported(),
        serde_json::json!(message),
        code = code,
        message = escape_json(message),
        span = span,
    )
}

fn span_json(span: Option<&active_core::diag::Span>) -> String {
    match span {
        Some(s) => format!(
            "{{\"line\":{},\"column\":{},\"end_line\":{},\"end_column\":{}}}",
            s.line, s.column, s.end_line, s.end_column
        ),
        None => "null".to_string(),
    }
}

fn escape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn eval_manifest_inner(manifest: &str) -> String {
    let closure = match active_core::InputClosure::from_manifest(manifest, None, |path| {
        Err(format!("WASM closures must embed include `{path}` bytes"))
    }) {
        Ok(closure) => closure,
        Err(error) => {
            let code = if matches!(error, active_core::ClosureError::Manifest(_)) {
                "PARSE"
            } else {
                "E048"
            };
            return envelope_err(code, &error.to_string(), "null");
        }
    };
    let sandbox = match active_core::Sandbox::new(closure) {
        Ok(sandbox) => sandbox,
        Err(error) => return envelope_err("E048", &error.to_string(), "null"),
    };
    let module = match sandbox.parse(active_core::ParseOptions::default()) {
        Ok(module) => module,
        Err(diags) => {
            let lines: Vec<String> = diags
                .iter()
                .map(|d| format!("[{}] {}", d.code, d.message))
                .collect();
            return envelope_err_diagnostics(&lines.join("\n"), &diag_items(&diags));
        }
    };
    let engine = Engine::hermetic();
    match sandbox.evaluate_redacted(&engine, &module) {
        Ok((value, diags)) => {
            let json = active_core::convert::value_to_pretty_json(&value);
            let lines: Vec<String> = diags
                .iter()
                .map(|d| format!("[{}] {}", d.code, d.message))
                .collect();
            let diagnosed_error = diags
                .iter()
                .any(|d| d.severity == active_core::Severity::Error);
            let envelope = envelope_ok(
                &json,
                &serde_json::json!(lines.join("\n")).to_string(),
                &diag_items(&diags),
            );
            if diagnosed_error {
                return envelope_err_diagnostics(&lines.join("\n"), &diag_items(&diags));
            }
            envelope
        }
        Err(error) => envelope_err(error.code(), &format!("{error}"), "null"),
    }
}

fn eval_inner(source: &str, pure: bool) -> String {
    let module = match parse(source, "wasm") {
        Ok(m) => m,
        Err(diags) => {
            let lines: Vec<String> = diags
                .iter()
                .map(|d| format!("[{}] {}", d.code, d.message))
                .collect();
            return envelope_err_diagnostics(&lines.join("\n"), &diag_items(&diags));
        }
    };
    // Hermetic construction: Engine::new() snapshots the process ambient,
    // which aborts on wasm32-unknown-unknown. Hosts resolve env before
    // calling in (same rule as the FFI boundary).
    let engine = Engine::hermetic().with_pure(pure);
    match engine.evaluate_with_seeds(&module) {
        Ok((value, diags, seeds)) => {
            // Structural material (plain locator sources and derived
            // values) is redacted before serialization, matching the
            // CLI and FFI exporters. Seeds cover a secret bound to a
            // `let` name that never enters the exported tree.
            let profile = active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds);
            let redacted = active_core::convert::redact_value(&profile, &value);
            let json = active_core::convert::value_to_pretty_json(&redacted);
            let lines: Vec<String> = diags
                .iter()
                .map(|d| format!("[{}] {}", d.code, d.message))
                .collect();
            // DEC-004 / LANG-G-002: success is computed from the
            // diagnostics, not from the Result. An error-severity item
            // makes the envelope `ok: false` (same verdict as the CLI's
            // exit code) while the JSON and item list stay byte-identical,
            // so diagnosed-kind value assertions still compare.
            let diagnosed_error = diags
                .iter()
                .any(|d| d.severity == active_core::Severity::Error);
            let envelope = envelope_ok(
                &json,
                &serde_json::json!(lines.join("\n")).to_string(),
                &diag_items(&diags),
            );
            if diagnosed_error {
                envelope.replacen("\"ok\":true", "\"ok\":false", 1)
            } else {
                envelope
            }
        }
        Err(e) => match e {
            // Validation failures keep every diagnostic (codes AND spans),
            // not just the first code: strict hosts compare the full list.
            EvalError::ValidationFailed { diagnostics } => {
                let lines: Vec<String> = diagnostics
                    .iter()
                    .map(|d| format!("[{}] {}", d.code, d.message))
                    .collect();
                envelope_err_diagnostics(&lines.join("\n"), &diag_items(&diagnostics))
            }
            _ => envelope_err(e.code(), &e.to_string(), &span_json(e.span())),
        },
    }
}

/// Evaluate UTF-8 source to a JSON envelope
/// `{"ok":bool,"json":...,"diagnostics":"...",`items":[...]}`. The
/// `diagnostics` text line stays for prose readers; `items` carries one
/// `{code,message,severity,span}` object per diagnostic so hosts can
/// compare codes AND spans strictly without parsing prose. Returns the
/// envelope bytes (registry-owned, freed with `active_free`); `out_len`
/// receives the length. Secrets redact by construction; panics become
/// error envelopes.
///
/// # Safety
///
/// When `src_ptr` is non-null, `src_len` bytes starting at `src_ptr` must
/// be readable for this call; `out_len` is REQUIRED and must point to a
/// writable `usize` (the envelope length has no other channel, so a null
/// `out_len` returns null instead of an unmeasurable pointer). All pointers
/// obey the registry discipline (allocate here, free here, same thread).
#[no_mangle]
pub unsafe extern "C" fn active_eval_json(
    src_ptr: *const u8,
    src_len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    if out_len.is_null() {
        // LANG-G-021: a pointer the host cannot measure is useless; fail
        // closed with null rather than handing back an envelope.
        return std::ptr::null_mut();
    }
    let envelope = match catch_unwind(AssertUnwindSafe(|| {
        if src_ptr.is_null() {
            return envelope_err("ABI", "null pointer argument", "null");
        }
        if let Some(message) = source_bounds_error(src_ptr, src_len) {
            return envelope_err("ABI", message, "null");
        }
        // SAFETY: the registry proves `src_len` bytes are live at `src_ptr`.
        let bytes = unsafe { slice::from_raw_parts(src_ptr, src_len) };
        match str::from_utf8(bytes) {
            Ok(text) => eval_inner(text, false),
            Err(_) => envelope_err("UTF8", "source is not valid UTF-8", "null"),
        }
    })) {
        Ok(env) => env,
        Err(_) => envelope_err("PANIC", "evaluation panicked; input rejected", "null"),
    };
    let bytes = envelope.into_bytes();
    let len = bytes.len();
    let (ptr, _) = retain(bytes);
    // SAFETY: out_len was checked non-null at entry.
    unsafe {
        *out_len = len;
    }
    ptr
}

/// Evaluate UTF-8 source under pure mode (L-P05): env reads, unpinned
/// includes, and profile overrides are refused with E047 before any
/// work. This symbol is always pure (there is no flag to pass 0 through
/// it); the default mode stays [`active_eval_json`]. The envelope shape
/// is identical.
///
/// # Safety
///
/// Same obligations as [`active_eval_json`].
#[no_mangle]
pub unsafe extern "C" fn active_eval_json_pure(
    src_ptr: *const u8,
    src_len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    let envelope = match catch_unwind(AssertUnwindSafe(|| {
        if src_ptr.is_null() {
            return envelope_err("ABI", "null pointer argument", "null");
        }
        if let Some(message) = source_bounds_error(src_ptr, src_len) {
            return envelope_err("ABI", message, "null");
        }
        // SAFETY: the registry proves `src_len` bytes are live at `src_ptr`.
        let bytes = unsafe { slice::from_raw_parts(src_ptr, src_len) };
        match str::from_utf8(bytes) {
            Ok(text) => eval_inner(text, true),
            Err(_) => envelope_err("UTF8", "source is not valid UTF-8", "null"),
        }
    })) {
        Ok(env) => env,
        Err(_) => envelope_err("PANIC", "evaluation panicked; input rejected", "null"),
    };
    let bytes = envelope.into_bytes();
    let len = bytes.len();
    let (ptr, _) = retain(bytes);
    // SAFETY: out_len was checked non-null at entry.
    unsafe {
        *out_len = len;
    }
    ptr
}

/// ABI version constant for hosts to check at load time.
#[no_mangle]
pub extern "C" fn active_abi_version() -> u32 {
    1
}

/// BLAKE3 hex digest of arbitrary bytes (L-P47 playground snippet
/// identity). The host allocates the source with `active_alloc`, fills it,
/// then passes the pointer and length; the return is a 64-byte ASCII hex
/// allocation, freed with `active_free(ptr, 64)`. A null source pointer or
/// allocation failure returns null; the digest itself never fails.
///
/// # Safety
///
/// `src_ptr` must point to `src_len` readable bytes in this instance's
/// linear memory (the WIRING contract, same as `active_eval_json`).
#[no_mangle]
pub unsafe extern "C" fn active_blake3(src_ptr: *const u8, src_len: usize) -> *mut u8 {
    if src_ptr.is_null() || src_len > MAX_ALLOC_BYTES {
        return std::ptr::null_mut();
    }
    // Reject a length past the host's live allocation: `from_raw_parts`
    // with an out-of-bounds length is UB, not a catchable panic. The host
    // normally passes the exact allocation length; a shorter sub-slice is
    // still inside it.
    let Some(allocated) = live_len(src_ptr) else {
        return std::ptr::null_mut();
    };
    if src_len > allocated {
        return std::ptr::null_mut();
    }
    let hashed = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the registry proves `src_len` bytes are live at `src_ptr`.
        let bytes = unsafe { slice::from_raw_parts(src_ptr, src_len) };
        active_core::blob::blake3_hex(bytes)
    }));
    match hashed {
        Ok(hex) => {
            let (ptr, _) = retain(hex.into_bytes());
            ptr
        }
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope_of(src: &str) -> serde_json::Value {
        let out = eval_inner(src, false);
        serde_json::from_str(&out).expect("envelope is JSON")
    }

    #[test]
    fn pure_mode_refuses_an_env_read() {
        // L-P05: the AA path carries the same refusal as the CLI/FFI.
        let out = eval_inner("v = env.PURITY_WASM ?: 1\n", true);
        let env: serde_json::Value = serde_json::from_str(&out).expect("envelope");
        assert_eq!(env["ok"], false);
        assert!(
            env["diagnostics"].as_str().unwrap().contains("E047"),
            "{env}"
        );
        assert!(env["diagnostics"].as_str().unwrap().contains("PURITY_WASM"));
        // The same source evaluates under the default mode.
        let plain = envelope_of("v = env.PURITY_WASM ?: 1\n");
        assert_eq!(plain["ok"], true);
        assert_eq!(plain["json"]["v"], 1);
    }

    #[test]
    fn zero_length_allocations_are_distinct_and_free_safe() {
        // Round-3 M1: a zero-length Vec's dangling pointer would alias
        // every other zero-length request.
        let a = active_alloc(0);
        let b = active_alloc(0);
        assert!(!a.is_null() && !b.is_null());
        assert_ne!(a, b, "zero-length allocations must not alias");
        active_free(a, 0);
        active_free(b, 0);
        // Over the transport cap: null, never a trap.
        assert!(active_alloc(MAX_ALLOC_BYTES + 1).is_null());
    }

    #[test]
    fn pure_export_symbol_refuses_through_the_abi() {
        // F5: the exported symbol itself (not just eval_inner) refuses,
        // and a null out_len stays the documented null. The source must
        // come from `active_alloc` (the ABI contract): the bounds guard
        // now validates it against the live allocation.
        let bytes = b"v = env.PURITY_ABI ?: 1\n";
        let src = active_alloc(bytes.len());
        assert!(!src.is_null());
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), src, bytes.len()) };
        let mut len = 0usize;
        let ptr = unsafe { active_eval_json_pure(src, bytes.len(), &mut len) };
        assert!(!ptr.is_null());
        let text = unsafe { std::slice::from_raw_parts(ptr, len) };
        let env: serde_json::Value = serde_json::from_slice(text).expect("envelope");
        assert_eq!(env["ok"], false);
        assert!(
            env["diagnostics"].as_str().unwrap().contains("E047"),
            "{env}"
        );
        active_free(ptr, len);
        let null = unsafe { active_eval_json_pure(src, bytes.len(), std::ptr::null_mut()) };
        assert!(null.is_null());
        active_free(src, bytes.len());
    }

    #[test]
    fn envelope_accepts_clean_document() {
        let env = envelope_of("port = 8080\n");
        assert_eq!(env["ok"], true);
        assert_eq!(env["json"]["port"], 8080);
        assert_eq!(env["diagnostics"], "");
    }

    #[test]
    fn envelope_carries_diagnostic_codes() {
        let env = envelope_of("timeout = 5s + 512MiB\n");
        assert_eq!(env["ok"], false);
        assert!(env["diagnostics"].as_str().unwrap().contains("T001"));
    }

    #[test]
    fn envelope_items_carry_codes_and_spans() {
        // Structured items mirror the text line one to one: strict hosts
        // compare these, never prose.
        let env = envelope_of("timeout = 5s + 512MiB\n");
        let items = env["items"].as_array().expect("items array");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["code"], "T001");
        assert_eq!(items[0]["severity"], "error");
        let span = &items[0]["span"];
        assert!(span.is_object(), "units refusal carries a span");
        assert_eq!(span["line"], 1);
        let clean = envelope_of("port = 8080\n");
        assert_eq!(clean["items"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn envelope_items_cover_spanless_warnings_and_repeats() {
        // S001 (cycle) is spanless: null span, still an item.
        let cycle = envelope_of("a = b\nb = a\n");
        assert_eq!(cycle["ok"], false);
        let items = cycle["items"].as_array().expect("items array");
        assert!(!items.is_empty());
        assert!(items.iter().all(|it| it["code"] == "S001"));
        assert!(items.iter().all(|it| it["span"].is_null()));
        // W001 carries warning severity with a span (conformance 0002).
        let norway = envelope_of("[country]\nname = NO\n");
        let items = norway["items"].as_array().expect("items array");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["code"], "W001");
        assert_eq!(items[0]["severity"], "warning");
        assert!(items[0]["span"].is_object());
        // Recovery is now single-diagnostic per over-deep construct
        // (LANG-A-006): the 65-deep fixture yields exactly one E004, not
        // one per bracket. Occurrence preservation below the depth bound
        // is still pinned by the parser's own tests.
        let deep_src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../conformance/cases/0006-budget-depth.active"
        ))
        .expect("budget fixture readable");
        let deep = envelope_of(&deep_src);
        let items = deep["items"].as_array().expect("items array");
        assert_eq!(items.len(), 1, "one root-cause item: {items:?}");
        assert_eq!(items[0]["code"], "E004");
    }

    #[test]
    fn envelope_flood_input_items_are_bounded() {
        // 300 unclosed brackets through the shipped envelope: fail-closed
        // with E004 items, total bounded by the parser diagnostic cap.
        // Contract guard (passes via the cap): prevents unbounded
        // envelope growth from reaching JSON hosts.
        let src = format!("value = {}", "[".repeat(300));
        let env = envelope_of(&src);
        assert_eq!(env["ok"], false);
        let items = env["items"].as_array().expect("items array");
        assert!(!items.is_empty(), "flood input must fail");
        assert!(
            items.len() <= 1024,
            "envelope items bounded, got {}",
            items.len()
        );
        assert!(
            items.iter().any(|it| it["code"] == "E004"),
            "depth refusal present"
        );
        assert!(
            items
                .iter()
                .all(|it| it["code"] == "E004" || it["code"] == "E001"),
            "only the depth family (E004 over-depth, E001 unclosed)"
        );
    }

    #[test]
    fn envelope_redacts_secrets() {
        let env = envelope_of("let pw = secret(\"hunter2\")\n[out]\npass = pw\n");
        assert_eq!(env["ok"], true);
        let flat = env.to_string();
        assert!(flat.contains("***SECRET***"), "{flat}");
        assert!(!flat.contains("hunter2"), "{flat}");
    }

    #[test]
    fn envelope_rejects_invalid_utf8_shape() {
        // Non-UTF8 is exercised at the byte boundary; the inner path
        // rejects unparseable source with a coded envelope instead.
        let env = envelope_of("[unclosed\n");
        assert_eq!(env["ok"], false);
        assert!(!env["diagnostics"].as_str().unwrap().is_empty());
    }

    /// LANG-C-012: the envelope reports whether this build enforces a wall
    /// clock, on success and failure alike, so a host can detect the
    /// degradation instead of discovering it under load.
    /// LANG-C-018: a request over the cap returns null instead of trying
    /// the allocation (which would abort the instance).
    #[test]
    fn an_absurd_allocation_returns_null_instead_of_aborting() {
        assert!(!active_alloc(64).is_null());
        assert!(active_alloc(MAX_ALLOC_BYTES + 1).is_null());
        assert!(active_alloc(usize::MAX).is_null());
    }

    #[test]
    fn a_null_out_len_returns_null_instead_of_an_unmeasurable_envelope() {
        // LANG-G-021: without the length slot the host cannot read the
        // envelope, so the call fails closed. With a real slot the same
        // source returns a readable envelope.
        let bytes = b"port = 8080\n";
        let src = active_alloc(bytes.len());
        assert!(!src.is_null());
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), src, bytes.len()) };
        let ptr = unsafe { active_eval_json(src, bytes.len(), std::ptr::null_mut()) };
        assert!(ptr.is_null(), "null out_len must not produce an envelope");
        let mut len = 0usize;
        let ptr = unsafe { active_eval_json(src, bytes.len(), &mut len) };
        assert!(!ptr.is_null());
        assert!(len > 0);
        let envelope = unsafe { slice::from_raw_parts(ptr, len) };
        let parsed: serde_json::Value = serde_json::from_slice(envelope).expect("json");
        assert_eq!(parsed["ok"], serde_json::json!(true));
        active_free(ptr, len);
        active_free(src, bytes.len());
    }

    #[test]
    fn freeing_an_unknown_pointer_is_a_noop_in_every_profile() {
        // LANG-G-013: a release build ignored unknown pointers while a
        // debug_assert trapped in debug, so the profiles disagreed. The
        // documented contract is "ignored"; the registry must stay intact
        // and a real allocation must still free afterwards.
        active_free(0xdead_beef as *mut u8, 8);
        let ptr = active_alloc(16);
        assert!(!ptr.is_null());
        active_free(ptr, 16);
        // Double-free of the same (now unknown) pointer is a host bug and
        // still a no-op, never a trap.
        active_free(ptr, 16);
    }

    #[test]
    fn envelopes_report_the_wall_clock_capability() {
        let expected = active_core::Budget::wall_supported();
        let ok: serde_json::Value =
            serde_json::from_str(&envelope_ok("{}", "[]", "[]")).expect("ok envelope is json");
        assert_eq!(ok["wall_supported"], serde_json::json!(expected));
        assert_eq!(ok["ok"], serde_json::json!(true));

        let err: serde_json::Value =
            serde_json::from_str(&envelope_err("E001", "synthetic", "null"))
                .expect("error envelope is json");
        assert_eq!(err["wall_supported"], serde_json::json!(expected));
        assert_eq!(err["ok"], serde_json::json!(false));
    }

    /// The evaluator still returns a coded envelope for a budget trip.
    #[test]
    fn a_pathological_document_still_returns_a_coded_envelope() {
        // A 4 KiB literal doubled twelve times (each alias interpolates
        // the PREVIOUS alias) is 16 MiB of materialised string: the size
        // accountant must trip X006 (hermetic engine, no host environment).
        let mut source = String::from("s = \"");
        source.push_str(&"x".repeat(4096));
        source.push_str("\"\n");
        let mut prev = "s".to_string();
        for i in 0..12 {
            source.push_str(&format!("a{i} = \"${{{prev}}}${{{prev}}}\"\n"));
            prev = format!("a{i}");
        }
        let envelope = eval_inner(&source, false);
        let parsed: serde_json::Value = serde_json::from_str(&envelope).expect("json");
        assert_eq!(parsed["ok"], serde_json::json!(false), "{envelope}");
        assert!(
            parsed["diagnostics"].as_str().unwrap_or("").contains("X0"),
            "a budget trip is coded: {envelope}"
        );
    }
}

#[cfg(test)]
mod abi_guard_tests {
    use super::*;

    fn read_envelope(out: *mut u8, len: usize) -> String {
        let bytes = unsafe { slice::from_raw_parts(out, len).to_vec() };
        String::from_utf8(bytes).expect("envelope is UTF-8")
    }

    #[test]
    fn an_oversized_source_length_is_refused_without_reading_out_of_bounds() {
        let src = active_alloc(16);
        assert!(!src.is_null());
        let mut len = 0usize;
        let out = unsafe { active_eval_json(src, 64 * 1024 * 1024, &mut len) };
        assert!(!out.is_null(), "an ABI error is an envelope, not a trap");
        let text = read_envelope(out, len);
        assert!(
            text.contains("ABI") && text.contains("live allocation"),
            "{text}"
        );
        active_free(out, len);

        // One byte past the allocation is refused by the hash export too.
        let hashed = unsafe { active_blake3(src, 17) };
        assert!(hashed.is_null(), "out-of-bounds hash length must be null");

        let digest = unsafe { active_blake3(src, 16) };
        assert!(!digest.is_null());
        let hex = read_envelope(digest, 64);
        assert!(
            hex.len() == 64
                && hex
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{hex}"
        );
        active_free(digest, 64);
        active_free(src, 16);
    }

    #[test]
    fn a_null_source_pointer_is_refused() {
        let mut len = 0usize;
        let out = unsafe { active_eval_json(std::ptr::null(), 0, &mut len) };
        assert!(!out.is_null());
        let text = read_envelope(out, len);
        assert!(text.contains("null pointer"), "{text}");
        active_free(out, len);
        assert!(unsafe { active_blake3(std::ptr::null(), 0) }.is_null());
    }
}

/// L-P28: evaluate a §7.2a closure manifest passed as UTF-8 bytes.
///
/// # Safety
///
/// `manifest_ptr` must point to `manifest_len` live bytes.
#[no_mangle]
pub unsafe extern "C" fn active_eval_sandboxed_json(
    manifest_ptr: *const u8,
    manifest_len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    let envelope = match catch_unwind(AssertUnwindSafe(|| {
        if manifest_ptr.is_null() {
            return envelope_err("ABI", "null pointer argument", "null");
        }
        if let Some(message) = source_bounds_error(manifest_ptr, manifest_len) {
            return envelope_err("ABI", message, "null");
        }
        let bytes = unsafe { slice::from_raw_parts(manifest_ptr, manifest_len) };
        match str::from_utf8(bytes) {
            Ok(text) => eval_manifest_inner(text),
            Err(_) => envelope_err("UTF8", "manifest is not valid UTF-8", "null"),
        }
    })) {
        Ok(envelope) => envelope,
        Err(_) => envelope_err("PANIC", "evaluation panicked; input rejected", "null"),
    };
    let bytes = envelope.into_bytes();
    let len = bytes.len();
    let (ptr, _) = retain(bytes);
    unsafe { *out_len = len };
    ptr
}
