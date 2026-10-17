//! libFuzzer target: the parser (and evaluator) must never panic and
//! always terminate on arbitrary byte input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use active_core::{parse, Engine};

fuzz_target!(|data: &[u8]| {
    // Lossy conversion is deliberate: invalid UTF-8 must not panic.
    let src = String::from_utf8_lossy(data);
    if let Ok(module) = parse(&src, "fuzz") {
        let _ = Engine::new().evaluate(&module);
    }
});
