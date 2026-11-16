//! libFuzzer target: formatting is meaning-preserving (LANG-I-003).
//! Formatting a parsed document, re-parsing it, and evaluating both must
//! yield identical JSON; neither pass may panic.

#![no_main]

use active_core::{fmt, parse, Engine, Style};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let src = String::from_utf8_lossy(data);
    let Ok(module) = parse(&src, "fuzz-fmt-eval") else {
        return;
    };
    let formatted = fmt::format(&module, Style::Indent);
    let Ok(reparsed) = parse(&formatted, "fuzz-fmt-eval") else {
        panic!("formatted output must re-parse:\n{formatted}");
    };
    let mut engine = Engine::hermetic();
    engine.budget = active_core::Budget::trusted();
    engine.budget.max_wall_ms = active_core::eval::UNLIMITED;
    let before = engine.evaluate(&module);
    let after = engine.evaluate(&reparsed);
    if let (Ok((before, _)), Ok((after, _))) = (before, after) {
        assert_eq!(
            active_core::convert::value_to_pretty_json(&before),
            active_core::convert::value_to_pretty_json(&after),
            "formatting changed the evaluated value"
        );
    }
});
