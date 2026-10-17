//! Conformance suite metadata (C-063, spec/conformance.md).
//!
//! The value-tree assertions live in `corpus.rs`
//! (`spec_corpus_matches_expected_values`); this target pins the
//! suite itself: every case has a parseable sidecar in both
//! directions (no orphans), the reference-implementation
//! declaration is present, and every case evaluates
//! byte-identically twice (determinism is part of conformance).

use active_core::{parse, Engine};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn spec_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("spec")
}

fn spec_pairs() -> Vec<(PathBuf, PathBuf)> {
    let mut actives: Vec<PathBuf> = std::fs::read_dir(spec_root())
        .expect("tests/spec readable")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "active"))
        .collect();
    actives.sort();
    assert_eq!(
        actives.len(),
        13,
        "conformance suite currently holds 13 cases; spec/conformance.md §2 names them all"
    );
    actives
        .into_iter()
        .map(|active| {
            let sidecar = active.with_extension("expected.json");
            assert!(
                sidecar.is_file(),
                "{:?} has no <name>.expected.json sidecar",
                active.file_name().unwrap()
            );
            let text = std::fs::read_to_string(&sidecar).expect("sidecar readable");
            serde_json::from_str::<serde_json::Value>(&text)
                .unwrap_or_else(|e| panic!("{sidecar:?} is not valid JSON: {e}"));
            (active, sidecar)
        })
        .collect()
}

fn eval_json(src: &str, name: &str) -> String {
    let module = parse(src, name).unwrap_or_else(|diags| panic!("{name}: {diags:?}"));
    // Same hermetic snapshot as the corpus runner: ambient values
    // must not leak into conformance results on any machine.
    let (value, diags) = Engine::new()
        .with_env(HashMap::new())
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert!(diags.is_empty(), "{name}: {diags:?}");
    active_core::convert::value_to_pretty_json(&value)
}

#[test]
fn sidecars_are_complete_and_valid() {
    // Also the reverse direction: no orphan .expected.json without
    // its .active case (matching sidecar suffix exactly, not any
    // *.json that happens to sit in the directory).
    let pairs = spec_pairs();
    let expected_count = std::fs::read_dir(spec_root())
        .expect("readable")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".expected.json"))
        .count();
    assert_eq!(
        expected_count,
        pairs.len(),
        "orphan .expected.json without a .active case"
    );
}

#[test]
fn reference_implementation_is_declared() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("spec")
        .join("conformance.md");
    let text = std::fs::read_to_string(&path).expect("spec/conformance.md readable");
    assert!(
        text.contains("reference implementation"),
        "declaration missing"
    );
    assert!(
        text.contains("RatioArtificiosa/solid-lang"),
        "declaration must name the repository"
    );
}

#[test]
fn every_case_evaluates_deterministically() {
    for (active, _) in spec_pairs() {
        let name = active.file_name().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read_to_string(&active).expect("readable");
        let first = eval_json(&src, &name);
        let second = eval_json(&src, &name);
        assert_eq!(first, second, "{name}: non-deterministic evaluation");
    }
}
