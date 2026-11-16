//! libFuzzer target: secret handling must never panic and a redacted
//! export must not leak the runtime secret material the evaluator
//! observed (LANG-I-003). Documents around `secret()`, `secret.env()`,
//! profiles, and patches are exercised through the shipped export path.
//!
//! The check is structural, over exported *values*: raw text scanning
//! would flag unrelated key names and marker text. Material substrings
//! shorter than [`MIN_SUBSTRING`] are exact-match only by contract
//! (spec/secrets.md §3), so those are checked by identity, which is
//! exactly what the shipped redactor does.

#![no_main]

use active_core::secret::SecretRedactor;
use active_core::{parse, Engine, Value};
use libfuzzer_sys::fuzz_target;

/// Shortest value the redactor treats as a substring (spec/secrets.md §3).
const MIN_SUBSTRING: usize = 4;

fn assert_no_material(value: &Value, seeds: &SecretRedactor) {
    match value {
        // A redacted material value is an empty marker; a payload here
        // means the export path failed to drop the bytes.
        Value::Secret(text) => {
            assert!(text.is_empty(), "redaction kept secret payload bytes");
        }
        Value::String(text) => {
            assert!(
                !seeds.contains_exact(text),
                "an exported string is exactly a runtime secret"
            );
            assert!(
                !seeds.contains_substring(text, MIN_SUBSTRING),
                "an exported string embeds a runtime secret of at least {MIN_SUBSTRING} bytes"
            );
        }
        Value::Array(items) => items
            .iter()
            .for_each(|item| assert_no_material(item, seeds)),
        Value::Table(map) => map
            .values()
            .for_each(|item| assert_no_material(item, seeds)),
        // A retained payload is one the shipped materiality check
        // cleared; scanning it here asks the same question of the
        // bytes that reach `payload_b64`.
        Value::Blob {
            payload: Some(bytes),
            ..
        } => assert!(
            !seeds.contains_substring(&String::from_utf8_lossy(bytes), MIN_SUBSTRING),
            "an exported blob payload embeds a runtime secret"
        ),
        _ => {}
    }
}

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
    let redacted = active_core::convert::redact_value(&profile, &value);
    assert_no_material(&redacted, &seeds);
    // Serialisation is part of the shipped path: it must not panic.
    let _ = active_core::convert::value_to_pretty_json(&redacted);
});
