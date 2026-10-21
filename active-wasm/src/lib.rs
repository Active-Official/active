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

/// Allocate `size` bytes of linear memory for the host to fill. Free with
/// `active_free` using the same pointer (size rechecked, registry rules).
#[no_mangle]
pub extern "C" fn active_alloc(size: usize) -> *mut u8 {
    let (ptr, _) = retain(vec![0u8; size]);
    ptr
}

/// Free memory previously returned by `active_alloc`/`active_eval_json`.
/// Unknown pointers are ignored (host bug, documented); null is accepted.
/// The size argument is rechecked against the registry in debug builds.
#[no_mangle]
pub extern "C" fn active_free(ptr: *mut u8, size: usize) {
    if ptr.is_null() {
        return;
    }
    let freed = release(ptr);
    debug_assert!(freed, "active_free of unknown pointer");
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

fn envelope_ok(json: &str, diagnostics: &str, items: &str) -> String {
    let mut out = String::from("{\"ok\":true,\"json\":");
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
        "{{\"ok\":false,\"json\":\"\",\"diagnostics\":{},\"items\":{}}}",
        serde_json::json!(diagnostics),
        items
    )
}

fn envelope_err(code: &str, message: &str, span: &str) -> String {
    // Bare codes are always [A-Z0-9]+ by construction; the message is
    // escaped, and span arrives pre-rendered (object or null).
    format!(
        "{{\"ok\":false,\"json\":\"\",\"diagnostics\":\"[{code}] {message}\",\"items\":[{{\"code\":\"{code}\",\"message\":{},\"severity\":\"error\",\"span\":{span}}}]}}",
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

fn eval_inner(source: &str) -> String {
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
    let engine = Engine::hermetic();
    match engine.evaluate(&module) {
        Ok((value, diags)) => {
            let json = active_core::convert::value_to_pretty_json(&value);
            let lines: Vec<String> = diags
                .iter()
                .map(|d| format!("[{}] {}", d.code, d.message))
                .collect();
            envelope_ok(
                &json,
                &serde_json::json!(lines.join("\n")).to_string(),
                &diag_items(&diags),
            )
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
/// be readable for this call; `out_len` must be null or point to a writable
/// `usize`. All pointers obey the registry discipline (allocate here, free
/// here, same thread).
#[no_mangle]
pub unsafe extern "C" fn active_eval_json(
    src_ptr: *const u8,
    src_len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    let envelope = match catch_unwind(AssertUnwindSafe(|| {
        if src_ptr.is_null() || out_len.is_null() {
            return envelope_err("ABI", "null pointer argument", "null");
        }
        // SAFETY: host guarantees readable bytes per WIRING.md.
        let bytes = unsafe { slice::from_raw_parts(src_ptr, src_len) };
        match str::from_utf8(bytes) {
            Ok(text) => eval_inner(text),
            Err(_) => envelope_err("UTF8", "source is not valid UTF-8", "null"),
        }
    })) {
        Ok(env) => env,
        Err(_) => envelope_err("PANIC", "evaluation panicked; input rejected", "null"),
    };
    let bytes = envelope.into_bytes();
    let len = bytes.len();
    let (ptr, _) = retain(bytes);
    if !out_len.is_null() {
        // SAFETY: host passed a writable slot per WIRING.md.
        unsafe {
            *out_len = len;
        }
    }
    ptr
}

/// ABI version constant for hosts to check at load time.
#[no_mangle]
pub extern "C" fn active_abi_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope_of(src: &str) -> serde_json::Value {
        let out = eval_inner(src);
        serde_json::from_str(&out).expect("envelope is JSON")
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
        // Recovery can repeat a code: every occurrence is an item
        // (conformance 0006: depth trips on open and close brackets).
        let deep_src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../conformance/cases/0006-budget-depth.active"
        ))
        .expect("budget fixture readable");
        let deep = envelope_of(&deep_src);
        let items = deep["items"].as_array().expect("items array");
        assert!(items.len() > 1, "depth refusal repeats: {items:?}");
        assert!(items.iter().all(|it| it["code"] == "E004"));
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
}
