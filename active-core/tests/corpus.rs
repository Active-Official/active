//! Corpus runner (C-010; spec corpus promoted to the official
//! conformance suite in C-063, spec/conformance.md).
//!
//! - tests/positive/*.active — must parse and evaluate with zero diagnostics.
//! - tests/negative/*.active — must fail at parse or evaluation.
//! - tests/spec/<name>.active with sidecar <name>.expected.json — the
//!   evaluated value tree must match the expected tree exactly (the
//!   official conformance suite; see also tests/conformance.rs for
//!   suite-metadata pins).
//!
//! Env vars are only read with defaults in the positive corpus, so results
//! are environment-independent.

use active_core::{parse, Engine};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn corpus_files(dir: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join(dir);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", root.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "active"))
        .collect();
    files.sort();
    files
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn positive_corpus_parses_and_evaluates_cleanly() {
    let files = corpus_files("positive");
    assert!(
        files.len() >= 20,
        "expected at least 20 positive corpus files, found {}",
        files.len()
    );
    let engine = Engine::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy();
        let src = read(f);
        let module =
            parse(&src, &name).unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
        let (_, diags) = engine
            .evaluate(&module)
            .unwrap_or_else(|e| panic!("{name}: evaluation failed: {e}"));
        assert!(diags.is_empty(), "{name}: produced diagnostics: {diags:?}");
    }
}

#[test]
fn negative_corpus_fails() {
    let files = corpus_files("negative");
    assert!(
        files.len() >= 10,
        "expected at least 10 negative corpus files, found {}",
        files.len()
    );
    let engine = Engine::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy();
        let src = read(f);
        let failed = match parse(&src, &name) {
            Err(_) => true,
            Ok(module) => engine.evaluate(&module).is_err(),
        };
        assert!(
            failed,
            "{name}: expected a parse or evaluation failure, but it succeeded"
        );
    }
}

#[test]
fn spec_corpus_matches_expected_values() {
    let files = corpus_files("spec");
    assert!(
        !files.is_empty(),
        "expected at least one spec corpus file with a sidecar expected.json"
    );
    // Hermetic by construction: an empty env snapshot, so ambient
    // variables cannot leak into conformance results on any machine.
    let engine = Engine::new().with_env(HashMap::new());
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy();
        let src = read(f);
        let module =
            parse(&src, &name).unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
        let (value, diags) = engine
            .evaluate(&module)
            .unwrap_or_else(|e| panic!("{name}: evaluation failed: {e}"));
        assert!(diags.is_empty(), "{name}: produced diagnostics: {diags:?}");

        let expected_path = f.with_extension("expected.json");
        let expected = read(&expected_path);
        let actual = active_core::convert::value_to_pretty_json(&value);
        // Normalize line endings: Windows checkouts use CRLF, the JSON
        // output uses LF. Compare on a common footing.
        let normalize = |s: &str| s.replace("\r\n", "\n").trim().to_string();
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "{name}: evaluated value does not match {expected_path:?}"
        );
    }
}
