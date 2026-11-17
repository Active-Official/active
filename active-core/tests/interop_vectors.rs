//! Owned interop vectors (LANG-E-002).
//!
//! `conformance/interop/` is the contract this workspace owns for the
//! binary bridge: every document exercises a value kind, and every case
//! has the exact redacted JSON this workspace ships. The executable half
//! of the pairing record lives in `conformance/interop/checker/`, which
//! runs the same paired documents through the pinned published codec
//! (`active-infinity 2.4.2`) and compares both projections.
//!
//! This test drives the shipped path on the text side (`parse` -> `Engine`
//! -> profile-redacted export), so the vectors cannot drift from the
//! language before the checker sees them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use active_core::convert;
use active_core::secret::SecretProfile;
use active_core::{parse, Engine};

type Json = serde_json::Value;

fn interop() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/interop")
}

fn cases(dir: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(interop().join(dir))
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("active"))
        .collect();
    out.sort();
    out
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .expect("stem")
        .to_string()
}

fn read_json(path: &Path) -> Json {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Run the shipped path on one case and return its redacted export.
fn export(case: &Path) -> (Json, String) {
    let source = std::fs::read_to_string(case).expect("case readable");
    let module = parse(&source, case.to_str().expect("path"))
        .unwrap_or_else(|diags| panic!("{}: parse failed: {diags:?}", case.display()));
    let engine = Engine::hermetic();
    let (value, diagnostics, seeds) = engine
        .evaluate_with_seeds(&module)
        .unwrap_or_else(|e| panic!("{}: evaluation failed: {e}", case.display()));
    assert!(
        diagnostics.is_empty(),
        "{}: a vector must evaluate cleanly: {diagnostics:?}",
        case.display()
    );
    let profile = SecretProfile::for_engine(&module, &engine, &seeds);
    (convert::value_to_json_with(&profile, &value), source)
}

fn expected_path(dir: &str, case: &Path) -> PathBuf {
    interop().join(dir).join(format!("{}.json", stem(case)))
}

fn assert_case_set(case_dir: &str, expected_dir: &str) {
    let expected: BTreeSet<String> = std::fs::read_dir(interop().join(expected_dir))
        .unwrap_or_else(|e| panic!("{expected_dir}: {e}"))
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .map(|path| stem(&path))
        .collect();
    let actual: BTreeSet<String> = cases(case_dir).iter().map(|path| stem(path)).collect();
    assert_eq!(
        actual, expected,
        "{case_dir} and {expected_dir} must describe the same case set"
    );
}

/// Every path rendered as the secret marker, so both projections can be
/// checked for the same taint map.
fn marker_paths(value: &Json, path: &str, out: &mut BTreeSet<String>) {
    match value {
        Json::String(text) if text == "***SECRET***" => {
            out.insert(path.to_string());
        }
        Json::Object(map) => {
            if let Some(blob) = map.get("$active.blob") {
                if blob.get("tainted").and_then(Json::as_bool) == Some(true) {
                    out.insert(format!("{path}.$active.blob"));
                }
            }
            for (key, item) in map {
                marker_paths(item, &format!("{path}.{key}"), out);
            }
        }
        Json::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                marker_paths(item, &format!("{path}[{index}]"), out);
            }
        }
        _ => {}
    }
}

/// Literal arguments of `secret("...")` calls, unescaped enough for the
/// vector corpus. Used to prove no expectation echoes one.
fn secret_literals(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(index) = rest.find("secret(\"") {
        let start = index + "secret(\"".len();
        let tail = &rest[start..];
        let mut literal = String::new();
        let mut escaped = false;
        for ch in tail.chars() {
            if escaped {
                literal.push(ch);
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => break,
                other => literal.push(other),
            }
        }
        if literal.len() >= 4 {
            out.push(literal);
        }
        rest = &tail[tail.find('"').map(|i| i + 1).unwrap_or(tail.len())..];
    }
    out
}

fn run_tier(case_dir: &str, expected_dir: &str, has_codec_expectation: bool, floor: usize) {
    assert_case_set(case_dir, expected_dir);
    let all = cases(case_dir);
    assert!(
        all.len() >= floor,
        "{case_dir}: {} case(s) present, at least {floor} required (an emptied corpus \
         must not pass)",
        all.len()
    );
    for case in all {
        let name = stem(&case);
        let (json, source) = export(&case);
        let expected = read_json(&expected_path(expected_dir, &case));
        assert_eq!(json, expected, "{name}: export differs from {expected_dir}");

        // Determinism: the same source, engine, and profile produce the
        // same bytes (spec/reproducibility.md).
        let (again, _) = export(&case);
        assert_eq!(json, again, "{name}: export is not deterministic");

        // No expectation may echo a secret literal authored in the case.
        let rendered = format!("{expected}");
        for literal in secret_literals(&source) {
            assert!(
                !rendered.contains(&literal),
                "{name}: {expected_dir} echoes the secret literal `{literal}`"
            );
        }

        let infinity_path = interop()
            .join("expected-infinity")
            .join(format!("{name}.json"));
        if has_codec_expectation {
            assert!(
                infinity_path.is_file(),
                "{name}: paired case needs a codec expectation"
            );
            let infinity = read_json(&infinity_path);
            let mut ours_markers = BTreeSet::new();
            marker_paths(&expected, "", &mut ours_markers);
            let mut codec_markers = BTreeSet::new();
            marker_paths(&infinity, "", &mut codec_markers);
            assert_eq!(
                ours_markers, codec_markers,
                "{name}: the codec projection must redact the same paths"
            );
            for literal in secret_literals(&source) {
                assert!(
                    !format!("{infinity}").contains(&literal),
                    "{name}: the codec expectation echoes `{literal}`"
                );
            }
        } else {
            assert!(
                !infinity_path.exists(),
                "{name}: current-core cases cannot have a codec expectation until the \
                 published codec depends on a core that parses them"
            );
        }
    }
}

#[test]
fn paired_vectors_match_the_shipped_export() {
    // Documents the published core (and therefore the pinned codec)
    // accepts today: scalars, containers, units, secrets, multiline text.
    run_tier("cases", "expected", true, 6);
}

#[test]
fn structural_vectors_match_the_shipped_export() {
    // Expression graphs: the codec preserves them structurally, its JSON
    // projection renders expression nodes as markers, so only the
    // round-trip half applies to the paired expectation.
    run_tier("structural", "expected-structural", true, 1);
}

#[test]
fn current_core_vectors_match_the_shipped_export() {
    // Blobs, tainted blobs, profiles, and patches need a core newer than
    // the published 1.4.0 the pinned codec depends on. They are owned
    // here now so the next re-pair has an executable contract.
    run_tier("current", "expected-current", false, 4);
}

#[test]
fn no_codec_expectation_is_orphaned() {
    let expected: BTreeSet<String> = std::fs::read_dir(interop().join("expected-infinity"))
        .expect("expected-infinity readable")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .map(|path| stem(&path))
        .collect();
    let mut cases_here: BTreeSet<String> = cases("cases").iter().map(|path| stem(path)).collect();
    for case in cases("structural") {
        cases_here.insert(stem(&case));
    }
    assert_eq!(
        expected, cases_here,
        "expected-infinity must mirror the paired cases exactly"
    );
}
