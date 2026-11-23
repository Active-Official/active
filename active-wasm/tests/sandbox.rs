//! L-P28 smoke: the WASM sandbox export consumes the §7.2a manifest.

use std::ffi::CString;

#[test]
fn wasm_sandboxed_export_uses_the_closure() {
    let manifest = CString::new(
        r#"{"version":1,"document_b64":"cCA9IGVudi5QID86ICJkIgo=","source_name":"main.active","includes":[],"env":[{"name":"P","set":true,"value":"closed"}]}"#,
    )
    .unwrap();
    let bytes = manifest.as_bytes();
    let mut out_len = 0usize;
    let input = active_wasm::active_alloc(bytes.len());
    assert!(!input.is_null());
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), input, bytes.len()) };
    let ptr = unsafe { active_wasm::active_eval_sandboxed_json(input, bytes.len(), &mut out_len) };
    assert!(!ptr.is_null());
    let envelope = unsafe { std::slice::from_raw_parts(ptr, out_len) };
    let text = String::from_utf8_lossy(envelope);
    assert!(text.contains("\"ok\":true"), "{text}");
    active_wasm::active_free(ptr, out_len);

    let undeclared = CString::new(
        r#"{"version":1,"document_b64":"cCA9IGVudi5QID86ICJkIgo=","source_name":"main.active","includes":[],"env":[]}"#,
    )
    .unwrap();
    let bytes = undeclared.as_bytes();
    let input = active_wasm::active_alloc(bytes.len());
    assert!(!input.is_null());
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), input, bytes.len()) };
    let ptr = unsafe { active_wasm::active_eval_sandboxed_json(input, bytes.len(), &mut out_len) };
    assert!(!ptr.is_null());
    let envelope = unsafe { std::slice::from_raw_parts(ptr, out_len) };
    let text = String::from_utf8_lossy(envelope);
    assert!(text.contains("E048"), "{text}");
    active_wasm::active_free(ptr, out_len);
}

#[test]
fn wasm_sandboxed_export_trips_a_count_budget_with_an_x_code() {
    // 120_000 bindings exceed the default 100_000-node ceiling.
    let mut document = String::with_capacity(1_500_000);
    for index in 0..120_000 {
        document.push_str(&format!("k{index} = {index}\n"));
    }
    let encoded = base64(document.as_bytes());
    let manifest = format!(
        r#"{{"version":1,"document_b64":"{encoded}","source_name":"budget.active","includes":[],"env":[]}}"#
    );
    let bytes = manifest.as_bytes();
    let input = active_wasm::active_alloc(bytes.len());
    assert!(!input.is_null());
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), input, bytes.len()) };
    let mut out_len = 0usize;
    let ptr = unsafe { active_wasm::active_eval_sandboxed_json(input, bytes.len(), &mut out_len) };
    assert!(!ptr.is_null());
    let envelope = unsafe { std::slice::from_raw_parts(ptr, out_len) };
    let text = String::from_utf8_lossy(envelope);
    assert!(text.contains("X001"), "{text}");
    active_wasm::active_free(ptr, out_len);
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (a << 16) | (b << 8) | c;
        out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
