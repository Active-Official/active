//! Machine-enforced pairing record (LANG-E-003).
//!
//! `docs/interop-pairing.md` records the verified triple. A written rule
//! alone cannot stop version skew, so this test fails when the shipped
//! version, the codec the interop vectors pin, and the recorded triple
//! disagree. Bumping `active-core` without re-verifying and updating the
//! record turns the suite red; changing the checker's pin without updating
//! the record does the same.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(relative: &str) -> String {
    let path = repo().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Version cell of a table row whose first cell names `component`.
fn row_version(doc: &str, component: &str) -> String {
    let prefix = format!("| `{component}`");
    for line in doc.lines() {
        if line.trim_start().starts_with(&prefix) {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            // cells[0] is empty (leading pipe), cells[1] is the name.
            let version = cells.get(2).copied().unwrap_or_default();
            return version
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches('`')
                .to_string();
        }
    }
    panic!("docs/interop-pairing.md has no `{component}` row");
}

fn is_concrete(version: &str) -> bool {
    let mut parts = version.split('.');
    let ok = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    });
    ok && parts.next().is_none()
}

#[test]
fn the_record_and_the_shipped_version_agree() {
    let doc = read("docs/interop-pairing.md");
    let recorded = row_version(&doc, "active-lang");
    assert_eq!(
        recorded,
        env!("CARGO_PKG_VERSION"),
        "docs/interop-pairing.md records active-lang {recorded} but this crate ships {}: \
         re-verify the bridge and update the triple before releasing",
        env!("CARGO_PKG_VERSION")
    );
    assert!(is_concrete(&recorded), "not a concrete version: {recorded}");
}

#[test]
fn the_record_pins_a_concrete_standalone_triple() {
    let doc = read("docs/interop-pairing.md");
    for component in ["active-infinity", "active-rpc"] {
        let version = row_version(&doc, component);
        assert!(
            is_concrete(&version),
            "docs/interop-pairing.md records {component} as {version}; the record must name \
             the version that was verified, never a range"
        );
    }
    assert!(
        doc.contains("re-verifies the bridge"),
        "the record must state the re-verification rule"
    );
    assert!(
        doc.contains("conformance/interop"),
        "the record must name the owned vector set it re-verifies"
    );
}

#[test]
fn the_interop_checker_tests_the_recorded_codec() {
    let doc = read("docs/interop-pairing.md");
    let recorded = row_version(&doc, "active-infinity");
    let manifest = read("conformance/interop/checker/Cargo.toml");
    let pinned = manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("active-infinity = \"="))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| {
            panic!("conformance/interop/checker/Cargo.toml must pin active-infinity with `=x.y.z`")
        });
    assert_eq!(
        pinned, recorded,
        "the checker pins active-infinity {pinned} but the record says {recorded}"
    );

    // The lockfile must resolve the pinned registry crate, not a local
    // path or a newer compatible version: the checker's whole purpose is
    // to exercise the version the record names.
    let lock = read("conformance/interop/checker/Cargo.lock");
    let mut lines = lock.lines();
    let mut found = false;
    while let Some(line) = lines.next() {
        if line.trim() == "name = \"active-infinity\"" {
            let version = lines
                .next()
                .unwrap_or_default()
                .trim()
                .trim_start_matches("version = ")
                .trim_matches('"');
            assert_eq!(
                version, recorded,
                "conformance/interop/checker/Cargo.lock resolves active-infinity {version}, \
                 the record says {recorded}"
            );
            found = true;
            break;
        }
    }
    assert!(found, "the checker lockfile has no active-infinity entry");
}

#[test]
fn the_release_checklist_points_at_the_record() {
    let checklist = read("docs/release-checklist.md");
    assert!(
        checklist.contains("docs/interop-pairing.md"),
        "the release checklist must point a release operator at the pairing record"
    );
}
