//! libFuzzer target: secret handling must never panic and a redacted
//! export must never contain the raw secret material the evaluator
//! observed (LANG-I-003). Documents around `secret()`, `secret.env()`,
//! profiles, and patches are exercised through the shipped export path.

#![no_main]

use active_core::{parse, Engine};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let src = String::from_utf8_lossy(data);
    let Ok(module) = parse(&src, "fuzz-secrets") else {
        return;
    };
    let mut engine = Engine::hermetic();
    engine.budget = active_core::Budget::trusted();
    engine.budget.max_wall_ms = active_core::eval::UNLIMITED;
    let Ok((value, _, seeds)) = engine.evaluate_with_seeds(&module) else {
        return;
    };
    let profile = active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds);
    profile.begin_redaction_pass();
    let redacted = active_core::convert::redact_value(&profile, &value);
    let json = active_core::convert::value_to_pretty_json(&redacted);
    // Every observed secret seed is material: none may survive an export.
    assert!(
        seeds.redact_text(&json).is_none(),
        "raw secret material survived redaction"
    );
});
