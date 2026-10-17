//! libFuzzer target: interpolation-heavy inputs must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;
use active_core::{parse, Engine};

fuzz_target!(|data: &[u8]| {
    let src = String::from_utf8_lossy(data);
    let with_interp = format!("let x = \"${{{src}}}\"\ny = x\n");
    if let Ok(module) = parse(&with_interp, "fuzz-interp") {
        let _ = Engine::new().evaluate(&module);
    }
});
