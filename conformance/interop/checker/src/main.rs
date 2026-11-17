//! Runs the Active interop vectors through the published Infinity codec
//! (LANG-E-002).
//!
//! The vectors are owned by this workspace (`conformance/interop/`); this
//! checker is the executable half of the pairing record. For every paired
//! case it proves:
//!
//! 1. the published bridge parses the Active source this workspace ships;
//! 2. `encode` -> `validate` -> `decode` preserves the module structurally;
//! 3. re-encoding the decoded module reproduces the same bytes (canonical
//!    fixed point) and the same content hash;
//! 4. the redacted JSON projection is stable across the round-trip and
//!    equals the expectation committed here;
//! 5. the projection is semantically equal to this workspace's own export
//!    after unit normalization (so a bridge that silently reorders or
//!    rescales a duration, quantity, secret, or container fails).
//!
//! The crate pins `active-infinity` to the version recorded in
//! `docs/interop-pairing.md`; the active-core that the codec depends on is
//! the published one, exactly like a downstream consumer.
//!
//! Run from the repository root:
//!
//! ```text
//! cargo run --locked --manifest-path conformance/interop/checker/Cargo.toml
//! ```

use std::path::{Path, PathBuf};

use serde_json::Value as Json;

/// The codec version this checker is built against. Keeping it in one
/// place lets the semantic tables below and the pairing record agree.
const PINNED_CODEC: &str = "2.4.2";

/// A silently empty vector set would let this checker exit green without
/// running a single round-trip, so both tiers have a floor. Adding a vector
/// is free; deleting the corpus is not.
const MIN_PAIRED_CASES: usize = 6;
const MIN_STRUCTURAL_CASES: usize = 1;

fn main() {
    let interop = interop_root();
    let mut failures = 0usize;
    let mut checked = 0usize;

    if active_infinity::VERSION != PINNED_CODEC {
        eprintln!(
            "codec version {} does not match the pinned {PINNED_CODEC}",
            active_infinity::VERSION
        );
        std::process::exit(1);
    }
    println!("codec = active-infinity {}", active_infinity::VERSION);

    for (dir, expected_dir, semantic, floor) in [
        ("cases", "expected", true, MIN_PAIRED_CASES),
        (
            "structural",
            "expected-structural",
            false,
            MIN_STRUCTURAL_CASES,
        ),
    ] {
        let found = cases(&interop.join(dir));
        if found.len() < floor {
            failures += 1;
            println!(
                "{dir}: FAIL {} case(s) present, at least {floor} required (an empty \
                 corpus must not pass)",
                found.len()
            );
        }
        for case in found {
            checked += 1;
            let name = stem(&case);
            match run_case(&interop, &case, expected_dir, semantic) {
                Ok(()) => println!(
                    "{name}: codec ok{}",
                    if semantic { ", semantics ok" } else { "" }
                ),
                Err(message) => {
                    failures += 1;
                    println!("{name}: FAIL {message}");
                }
            }
        }
    }

    // Every paired expectation must belong to a case: an orphaned
    // expectation would silently stop testing anything.
    for orphan in orphans(&interop, "expected-infinity", &["cases", "structural"]) {
        failures += 1;
        println!("{orphan}: FAIL expectation without a case");
    }

    println!("{checked} case(s), {failures} failure(s)");
    if failures > 0 {
        std::process::exit(1);
    }
}

fn interop_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("interop directory resolves")
}

fn cases(dir: &Path) -> Vec<PathBuf> {
    files_with_extension(dir, "active")
}

fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some(extension))
        .collect();
    out.sort();
    out
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .expect("case file stem")
        .to_string()
}

fn orphans(interop: &Path, expected_dir: &str, case_dirs: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = files_with_extension(&interop.join(expected_dir), "json")
        .iter()
        .map(|path| stem(path))
        .collect();
    for dir in case_dirs {
        for case in cases(&interop.join(dir)) {
            names.retain(|name| name != &stem(&case));
        }
    }
    names
}

fn run_case(interop: &Path, case: &Path, expected_dir: &str, semantic: bool) -> Result<(), String> {
    let name = stem(case);
    let source = std::fs::read_to_string(case).map_err(|e| format!("read: {e}"))?;

    let module = active_infinity::interop::from_active_source(&source, &name)
        .map_err(|e| format!("bridge refused the document: {e}"))?;
    let bytes = active_infinity::encode(&module).map_err(|e| format!("encode: {e}"))?;
    let meta = active_infinity::validate(&bytes).map_err(|e| format!("validate: {e}"))?;
    let decoded = active_infinity::decode(&bytes).map_err(|e| format!("decode: {e}"))?;
    if decoded != module {
        return Err("decode changed the module structure".into());
    }
    let again = active_infinity::encode(&decoded).map_err(|e| format!("re-encode: {e}"))?;
    if again != bytes {
        return Err("re-encode is not byte-identical (canonical fixed point)".into());
    }
    let re_hash =
        active_infinity::content_hash(&again).map_err(|e| format!("content hash: {e}"))?;
    if re_hash != meta.content_hash {
        return Err("content hash changed across the round-trip".into());
    }

    let projected = active_infinity::interop::to_redacted_json(&decoded)
        .map_err(|e| format!("projection: {e}"))?;
    let projected: Json =
        serde_json::from_str(&projected).map_err(|e| format!("projection is not JSON: {e}"))?;

    let expected_infinity = read_json(
        &interop
            .join("expected-infinity")
            .join(format!("{name}.json")),
    )?;
    if projected != expected_infinity {
        return Err(format!(
            "projection differs from expected-infinity/{name}.json: {}",
            diff(&expected_infinity, &projected)
        ));
    }

    if semantic {
        let ours = read_json(&interop.join(expected_dir).join(format!("{name}.json")))?;
        compare_ours(&ours, &projected, "$").map_err(|e| format!("semantic mismatch: {e}"))?;
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Semantic normalization
// ---------------------------------------------------------------------------
//
// The two sides live at different layers: this workspace exports EVALUATED
// values (`$active.duration` in milliseconds, `$active.quantity` in the
// base unit, byte-ness erased to an integer), while the codec projects the
// PARSED module (durations and quantities as `{value, unit}` with the
// authored unit). Normalizing both to (base value, dimension) makes the
// comparison exact where the layers agree and keeps the documented
// difference visible instead of hiding it.

#[derive(Debug, PartialEq)]
enum Number {
    Int(i128),
    Float(f64),
}

impl Number {
    fn from_json(value: &Json) -> Option<Number> {
        if let Some(i) = value.as_i64() {
            Some(Number::Int(i as i128))
        } else if let Some(u) = value.as_u64() {
            Some(Number::Int(u as i128))
        } else {
            value.as_f64().map(Number::Float)
        }
    }

    fn as_f64(&self) -> f64 {
        match self {
            Number::Int(i) => *i as f64,
            Number::Float(f) => *f,
        }
    }
}

/// Our evaluated unit wrapper as `(base value, dimension)`, when the value
/// is one. Only the exporter's own wrappers qualify; a data table that
/// happens to use the marker spelling is escaped at export (`\$active...`).
fn our_unit_value(value: &Json) -> Result<Option<(f64, &'static str)>, String> {
    let Some(obj) = value.as_object() else {
        return Ok(None);
    };
    if obj.len() != 1 {
        return Ok(None);
    }
    let (key, inner) = obj.iter().next().expect("one key");
    match key.as_str() {
        "$active.duration" => {
            let ms = inner
                .get("ms")
                .and_then(Number::from_json)
                .ok_or("duration without a numeric `ms`")?;
            Ok(Some((ms.as_f64(), "time")))
        }
        "$active.quantity" => {
            let unit = inner
                .get("unit")
                .and_then(Json::as_str)
                .ok_or("quantity without a unit")?;
            let amount = inner
                .get("value")
                .and_then(Number::from_json)
                .ok_or("quantity without a value")?;
            Ok(Some((amount.as_f64() * ours_unit(unit)?, dimension(unit))))
        }
        "$active.float" => Err(
            "non-finite float marker in a paired case: the codec projection has no \
             equivalent, so prove that value structurally instead"
                .into(),
        ),
        _ => Ok(None),
    }
}

/// The codec's unit projection as `(base value, dimension)`, only for the
/// exact `{value, unit}` shape with a recognized spelling. Anything else
/// stays a plain JSON value, so an authored table cannot change shape.
fn codec_unit_value(value: &Json) -> Option<(f64, &'static str)> {
    let obj = value.as_object()?;
    if obj.len() != 2 {
        return None;
    }
    let unit = obj.get("unit")?.as_str()?;
    let factor = codec_unit(unit).ok()?;
    let amount = Number::from_json(obj.get("value")?)?;
    Some((amount.as_f64() * factor, dimension(unit)))
}

/// Base-unit factor of this workspace's quantity suffix. Unknown suffixes
/// fail: a new unit must be added to the contract rather than compared
/// loosely.
fn ours_unit(unit: &str) -> Result<f64, String> {
    let factor = match unit {
        "bps" => 1.0,
        "Hz" => 1.0,
        other => return Err(format!("unmapped Active unit suffix {other:?}")),
    };
    Ok(factor)
}

/// Base-unit factor of the codec's unit spelling (long names). Unknown
/// spellings fail loudly for the same reason.
fn codec_unit(unit: &str) -> Result<f64, String> {
    let factor = match unit {
        "milliseconds" => 1.0,
        "seconds" => 1_000.0,
        "minutes" => 60_000.0,
        "hours" => 3_600_000.0,
        "days" => 86_400_000.0,
        "bytes" => 1.0,
        "kibibytes" => 1024.0,
        "mebibytes" => 1_048_576.0,
        "gibibytes" => 1_073_741_824.0,
        "bits-per-second" => 1.0,
        "kilobits-per-second" => 1_000.0,
        "megabits-per-second" => 1_000_000.0,
        "gigabits-per-second" => 1_000_000_000.0,
        "hertz" => 1.0,
        "kilohertz" => 1_000.0,
        "megahertz" => 1_000_000.0,
        "gigahertz" => 1_000_000_000.0,
        other => return Err(format!("unmapped codec unit {other:?}")),
    };
    Ok(factor)
}

fn dimension(unit: &str) -> &'static str {
    match unit {
        "milliseconds" | "seconds" | "minutes" | "hours" | "days" => "time",
        "bytes" | "kibibytes" | "mebibytes" | "gibibytes" => "bytes",
        "bps"
        | "bits-per-second"
        | "kilobits-per-second"
        | "megabits-per-second"
        | "gigabits-per-second" => "bitrate",
        "Hz" | "hertz" | "kilohertz" | "megahertz" | "gigahertz" => "frequency",
        other => panic!("dimension asked for unmapped unit {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

fn compare_ours(ours: &Json, codec: &Json, path: &str) -> Result<(), String> {
    // A unit wrapper on our side must be a unit on the codec side.
    if let Some((base, dim)) = our_unit_value(ours).map_err(|e| format!("{path}: {e}"))? {
        let (codec_base, codec_dim) = codec_unit_value(codec).ok_or_else(|| {
            format!("{path}: our {dim} value has no codec unit projection: {codec}")
        })?;
        if dim != codec_dim {
            return Err(format!("{path}: dimension {dim} vs {codec_dim}"));
        }
        return same_float(base, codec_base, path);
    }
    // An evaluated number normally stays a number. The one documented
    // exception is byte-ness, erased at evaluation (spec/secrets.md §2),
    // so a byte literal is an integer here and a byte unit there.
    if let Some(number) = Number::from_json(ours) {
        if let Some((base, dim)) = codec_unit_value(codec) {
            if dim != "bytes" {
                return Err(format!("{path}: number vs {dim} unit"));
            }
            return same_float(number.as_f64(), base, path);
        }
        let other = Number::from_json(codec).ok_or_else(|| format!("{path}: number vs {codec}"))?;
        return if numbers_match(&number, &other) {
            Ok(())
        } else {
            Err(format!("{path}: {number:?} vs {other:?}"))
        };
    }
    if ours.is_null() {
        return if codec.is_null() {
            Ok(())
        } else {
            Err(format!("{path}: null vs {codec}"))
        };
    }
    if let Some(flag) = ours.as_bool() {
        return if codec.as_bool() == Some(flag) {
            Ok(())
        } else {
            Err(format!("{path}: {flag} vs {codec}"))
        };
    }
    if let Some(text) = ours.as_str() {
        return if codec.as_str() == Some(text) {
            Ok(())
        } else {
            Err(format!("{path}: {text:?} vs {codec}"))
        };
    }
    if let Some(items) = ours.as_array() {
        let codec_items = codec
            .as_array()
            .ok_or_else(|| format!("{path}: array vs {codec}"))?;
        if items.len() != codec_items.len() {
            return Err(format!(
                "{path}: array length {} vs {}",
                items.len(),
                codec_items.len()
            ));
        }
        for (index, (left, right)) in items.iter().zip(codec_items).enumerate() {
            compare_ours(left, right, &format!("{path}[{index}]"))?;
        }
        return Ok(());
    }
    if let Some(map) = ours.as_object() {
        if codec.get("$expr").is_some() {
            return Err(format!(
                "{path}: the codec rendered an expression node where our export has a data value"
            ));
        }
        let codec_map = codec
            .as_object()
            .ok_or_else(|| format!("{path}: object vs {codec}"))?;
        if map.len() != codec_map.len() {
            return Err(format!("{path}: {} keys vs {}", map.len(), codec_map.len()));
        }
        for (key, item) in map {
            let other = codec_map
                .get(key)
                .ok_or_else(|| format!("{path}.{key}: missing on the codec side"))?;
            compare_ours(item, other, &format!("{path}.{key}"))?;
        }
        return Ok(());
    }
    Err(format!("{path}: unsupported JSON {ours}"))
}

fn same_float(a: f64, b: f64, path: &str) -> Result<(), String> {
    if floats_match(a, b) {
        Ok(())
    } else {
        Err(format!("{path}: {a} vs {b}"))
    }
}

fn numbers_match(a: &Number, b: &Number) -> bool {
    match (a, b) {
        (Number::Int(x), Number::Int(y)) => x == y,
        _ => floats_match(a.as_f64(), b.as_f64()),
    }
}

fn floats_match(a: f64, b: f64) -> bool {
    if a.is_nan() && b.is_nan() {
        return true;
    }
    if a == b {
        return true;
    }
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() <= 1e-12 * scale
}

fn diff(expected: &Json, actual: &Json) -> String {
    let left: Vec<char> = expected.to_string().chars().collect();
    let right: Vec<char> = actual.to_string().chars().collect();
    for (index, (a, b)) in left.iter().zip(&right).enumerate() {
        if a != b {
            let start = index.saturating_sub(40);
            let end = (index + 80).min(left.len()).min(right.len());
            let before: String = left[start..index].iter().collect();
            let after: String = right[start..index].iter().collect();
            let left_tail: String = left[index..end].iter().collect();
            let right_tail: String = right[index..end].iter().collect();
            return format!(
                "first difference at char {index}: expected ...{before}[{left_tail}]... actual ...{after}[{right_tail}]..."
            );
        }
    }
    format!("length {} vs {}", left.len(), right.len())
}
