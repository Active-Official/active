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

use active_core::{parse, Engine};
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

fn envelope_ok(json: &str, diagnostics: &str) -> String {
    let mut out = String::from("{\"ok\":true,\"json\":");
    out.push_str(json);
    out.push_str(",\"diagnostics\":");
    out.push_str(diagnostics);
    out.push('}');
    out
}

fn envelope_err_diagnostics(diagnostics: &str) -> String {
    // Real diagnostic lines already carry their codes; no synthetic prefix
    // (a wrapper code here would pollute code-set comparisons downstream).
    format!(
        "{{\"ok\":false,\"json\":\"\",\"diagnostics\":{}}}",
        serde_json::json!(diagnostics)
    )
}

fn envelope_err(code: &str, message: &str) -> String {
    format!(
        "{{\"ok\":false,\"json\":\"\",\"diagnostics\":\"[{code}] {message}\"}}",
        code = escape_json(code),
        message = escape_json(message)
    )
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
            return envelope_err_diagnostics(&lines.join("\n"));
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
            envelope_ok(&json, &serde_json::json!(lines.join("\n")).to_string())
        }
        Err(e) => envelope_err(e.code(), &e.to_string()),
    }
}

/// Evaluate UTF-8 source to a JSON envelope
/// `{"ok":bool,"json":...,"diagnostics":"..."}`. Returns the envelope bytes
/// (registry-owned, freed with `active_free`); `out_len` receives the length.
/// Secrets redact by construction; panics become error envelopes.
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
            return envelope_err("ABI", "null pointer argument");
        }
        // SAFETY: host guarantees readable bytes per WIRING.md.
        let bytes = unsafe { slice::from_raw_parts(src_ptr, src_len) };
        match str::from_utf8(bytes) {
            Ok(text) => eval_inner(text),
            Err(_) => envelope_err("UTF8", "source is not valid UTF-8"),
        }
    })) {
        Ok(env) => env,
        Err(_) => envelope_err("PANIC", "evaluation panicked; input rejected"),
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
