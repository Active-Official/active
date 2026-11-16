//! libFuzzer target: sidecar parsing and signature verification must
//! never panic on arbitrary bytes (LANG-I-003). The input is split into
//! sidecar text and file bytes; verification runs against an empty trust
//! set (every outcome is a clean error), so the parser and the identity
//! checks see hostile TOML.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let split = data
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(data.len() / 2);
    let sidecar = String::from_utf8_lossy(&data[..split]);
    let bytes = &data[split..];
    let trusted = active_core::sign::TrustedKeys::default();
    let _ =
        active_core::sign::verify_bytes("fuzz.active", "fuzz.active", bytes, &sidecar, &trusted);
});
