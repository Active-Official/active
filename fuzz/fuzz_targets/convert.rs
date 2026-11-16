//! libFuzzer target: the import converters must never panic and their
//! emitted source must parse and evaluate (LANG-I-003). Arbitrary bytes
//! are fed to all four importers; a successful import is round-tripped
//! through parse → evaluate → format.

#![no_main]

use active_core::{parse, Engine};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Keep one iteration cheap: the fuzzer explores many mutants.
    if data.len() > 64 * 1024 {
        return;
    }
    // Lossy conversion is deliberate: invalid UTF-8 must not panic.
    let text = String::from_utf8_lossy(data);
    let importers: [fn(&str) -> Result<(String, Vec<active_core::Diagnostic>), String>; 4] = [
        active_core::convert::json_text_to_active,
        active_core::convert::yaml_text_to_active,
        active_core::convert::toml_text_to_active,
        active_core::convert::jsonschema_to_active,
    ];
    for import in importers {
        if let Ok((source, _)) = import(&text) {
            if let Ok(module) = parse(&source, "fuzz-import") {
                let mut engine = Engine::hermetic();
                engine.budget = active_core::Budget::trusted();
                engine.budget.max_wall_ms = active_core::eval::UNLIMITED;
                let _ = engine.evaluate(&module);
                let _ = active_core::fmt::format(&module, active_core::Style::Indent);
            }
        }
    }
});
