//! Ported external corpora: toml-test, JSONTestSuite, yaml-test-suite.
//!
//! Each `tests/ported/<suite>/` directory holds verbatim source documents
//! plus one expectation sidecar per case: `<stem>.expected.json` with the
//! Active canonical value tree, or `<stem>.expected-diag.txt` pinning the
//! exact pipeline stage that rejects the document (`IMPORT_HARD_ERROR`,
//! `PARSE_ERROR`, `EVAL_ERROR`). Every case drives the SHIPPED importer
//! (`toml_text_to_active`, `json_text_to_active`, `yaml_text_to_active`)
//! followed by the real parser and evaluator — no re-implemented logic.
//! Origins, licenses and per-case delta classes: `tests/ported/README.md`
//! plus each suite's `manifest.json`.

use active_core::convert::{
    json_text_to_active, toml_text_to_active, value_to_pretty_json, yaml_text_to_active,
};
use active_core::fmt::format;
use active_core::{parse, Diagnostic, Engine, Style};
use std::path::{Path, PathBuf};

type ImportFn = fn(&str) -> Result<(String, Vec<Diagnostic>), String>;

fn suite_root(suite: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("ported")
        .join(suite)
}

/// (case input files, importer) enumerated in stable order.
/// Expectation sidecars (`*.expected.json`, `*.expected-diag.txt`,
/// `manifest.json`) are never treated as inputs.
fn case_files(suite: &str, ext: &str) -> Vec<PathBuf> {
    let root = suite_root(suite);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", root.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == ext)
                && !p.to_string_lossy().contains(".expected.")
                && p.file_name().is_some_and(|n| n != "manifest.json")
        })
        .collect();
    files.sort();
    files
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn eval_json(source: &str, name: &str) -> serde_json::Value {
    let module =
        parse(source, name).unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: evaluation failed: {e}"));
    assert!(diags.is_empty(), "{name}: eval diagnostics: {diags:?}");
    serde_json::from_str(&value_to_pretty_json(&value)).expect("canonical json")
}

fn check_suite(suite: &str, ext: &str, import: ImportFn, minimum: usize) {
    let files = case_files(suite, ext);
    assert!(
        files.len() >= minimum,
        "{suite}: expected at least {minimum} cases, found {}",
        files.len()
    );
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        // Sidecar stem: `example` for `example.toml`, `y_x.json` for
        // `y_x.json` (the `.json` source extension is part of the stem).
        let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
        let stem = if ext == "toml" || ext == "yaml" {
            stem
        } else {
            format!("{stem}.{ext}")
        };
        let dir = f.parent().unwrap();
        let expected = dir.join(format!("{stem}.expected.json"));
        let diag = dir.join(format!("{stem}.expected-diag.txt"));
        let text = read(f);
        if expected.exists() {
            let (source, notes) =
                import(&text).unwrap_or_else(|e| panic!("{suite}/{name}: import failed: {e}"));
            let _ = notes;
            let got = eval_json(&source, &name);
            let want: serde_json::Value = serde_json::from_str(&read(&expected))
                .unwrap_or_else(|e| panic!("{suite}/{name}: bad expectation: {e}"));
            assert_eq!(got, want, "{suite}/{name}: value mismatch");
        } else if diag.exists() {
            let recorded = read(&diag);
            let stage = recorded
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(':');
            match import(&text) {
                Err(_) => assert_eq!(
                    stage, "IMPORT_HARD_ERROR",
                    "{suite}/{name}: import failed, expected stage {stage}"
                ),
                Ok((source, notes)) => {
                    let codes: Vec<&str> = notes.iter().map(|d| d.code.as_str()).collect();
                    match parse(&source, &name) {
                        Err(_) => assert_eq!(
                            stage, "PARSE_ERROR",
                            "{suite}/{name}: parse failed, expected stage {stage}"
                        ),
                        Ok(module) => {
                            let failed = Engine::new().evaluate(&module).is_err();
                            assert!(
                                failed,
                                "{suite}/{name}: pipeline succeeded, expected {stage} (import notes: {codes:?})"
                            );
                            assert_eq!(
                                stage, "EVAL_ERROR",
                                "{suite}/{name}: eval failed, expected stage {stage}"
                            );
                        }
                    }
                }
            }
        } else {
            panic!("{suite}/{name}: no expectation sidecar (.expected.json or .expected-diag.txt)");
        }
    }
    // Every expectation sidecar must belong to a case file (no orphans).
    for entry in std::fs::read_dir(suite_root(suite)).expect("readable") {
        let p = entry.expect("entry").path();
        let n = p.file_name().unwrap().to_string_lossy().into_owned();
        if n == "manifest.json" || n == "README.md" {
            continue;
        }
        if n.contains(".expected.") {
            let base = n.split(".expected.").next().unwrap();
            let case_name = if ext == "toml" || ext == "yaml" {
                format!("{base}.{ext}")
            } else {
                base.to_string()
            };
            assert!(
                p.with_file_name(&case_name).exists(),
                "{suite}/{n}: orphan expectation without {case_name}"
            );
        }
    }
    // The manifest documents every case; fail loudly if it drifts.
    // Skip-only entries (e.g. non-UTF8 bytes, never committed as
    // fixtures) have no file on disk and are counted separately.
    let manifest: serde_json::Value =
        serde_json::from_str(&read(&suite_root(suite).join("manifest.json")))
            .expect("manifest parses");
    let manifest_map = manifest.as_object().expect("manifest object");
    let skips = manifest_map
        .values()
        .filter(|v| {
            v.get("delta")
                .and_then(|d| d.as_str())
                .is_some_and(|d| d.starts_with("skip-"))
        })
        .count();
    assert_eq!(
        manifest_map.len() - skips,
        files.len(),
        "{suite}: manifest case count drifted from fixture count"
    );
}

#[test]
fn ported_toml_valid_and_documented_rejections() {
    check_suite("toml", "toml", toml_text_to_active, 200);
}

#[test]
fn ported_json_accept_reject_and_impl_defined() {
    check_suite("json", "json", json_text_to_active, 250);
}

#[test]
fn ported_yaml_documents_and_rejections() {
    check_suite("yaml", "yaml", yaml_text_to_active, 300);
}

/// Guards the suite contract itself: no fixture may be an empty document
/// (an empty case would pass vacuously).
#[test]
fn ported_fixtures_are_nonempty() {
    for (suite, ext) in [("toml", "toml"), ("json", "json"), ("yaml", "yaml")] {
        for f in case_files(suite, ext) {
            // Zero-byte inputs are meaningful only with a rejection
            // expectation (e.g. JSON's empty-file case); without one
            // they would pass vacuously.
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
            let stem = if ext == "toml" || ext == "yaml" {
                stem
            } else {
                format!("{stem}.{ext}")
            };
            let has_diag = f
                .with_file_name(format!("{stem}.expected-diag.txt"))
                .exists();
            assert!(
                !read(&f).is_empty() || has_diag,
                "{suite}/{name} is zero bytes with no rejection expectation"
            );
        }
    }
}

/// Property over the whole ported accept set: format(parse(x)) re-parses
/// and evaluates to the identical tree (formatter fixed-point), in both
/// styles. Catches formatter data-loss on real-world shapes.
#[test]
fn ported_accept_cases_survive_format_round_trip() {
    for (suite, ext, import) in [
        ("toml", "toml", toml_text_to_active as ImportFn),
        ("json", "json", json_text_to_active as ImportFn),
        ("yaml", "yaml", yaml_text_to_active as ImportFn),
    ] {
        for f in case_files(suite, ext) {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
            let stem = if ext == "toml" || ext == "yaml" {
                stem
            } else {
                format!("{stem}.{ext}")
            };
            if !f.with_file_name(format!("{stem}.expected.json")).exists() {
                continue; // rejection cases have no tree to preserve
            }
            let text = read(&f);
            let (source, _) =
                import(&text).unwrap_or_else(|e| panic!("{suite}/{name}: import failed: {e}"));
            let first = eval_json(&source, &name);
            for style in [Style::Brace, Style::Indent] {
                let module = parse(&source, &name).unwrap_or_else(|d| panic!("{name}: {d:?}"));
                let reformatted = format(&module, style);
                let second = eval_json(&reformatted, &name);
                assert_eq!(
                    first, second,
                    "{suite}/{name}: format round-trip changed the tree ({style:?})"
                );
            }
        }
    }
}

/// Property over the whole ported accept set: double evaluation is
/// byte-identical (determinism over 500+ real-world documents).
#[test]
fn ported_accept_cases_evaluate_deterministically() {
    for (suite, ext, import) in [
        ("toml", "toml", toml_text_to_active as ImportFn),
        ("json", "json", json_text_to_active as ImportFn),
        ("yaml", "yaml", yaml_text_to_active as ImportFn),
    ] {
        for f in case_files(suite, ext) {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
            let stem = if ext == "toml" || ext == "yaml" {
                stem
            } else {
                format!("{stem}.{ext}")
            };
            if !f.with_file_name(format!("{stem}.expected.json")).exists() {
                continue;
            }
            let text = read(&f);
            let (source, _) =
                import(&text).unwrap_or_else(|e| panic!("{suite}/{name}: import failed: {e}"));
            let a = eval_json(&source, &name);
            let b = eval_json(&source, &name);
            assert_eq!(a, b, "{suite}/{name}: non-deterministic evaluation");
        }
    }
}
