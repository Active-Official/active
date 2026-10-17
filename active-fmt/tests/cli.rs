//! End-to-end tests for the standalone `active-fmt` binary (C-041).
//!
//! Proves `--check` parity with `active fmt --check`: names differing
//! files, writes nothing, exits non-zero on differences.

use std::path::PathBuf;
use std::process::Command;

fn active_fmt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_active-fmt"))
}

fn fixture(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("active-fmt-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    path
}

#[test]
fn check_flags_dirty_files_without_writing() {
    let before = "port=8080\n";
    let f = fixture("dirty.active", before);
    let out = active_fmt()
        .arg("--check")
        .arg(&f)
        .output()
        .expect("run active-fmt");
    assert!(!out.status.success(), "--check must exit non-zero");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("would reformat:"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(std::fs::read_to_string(&f).expect("read back"), before);
}

#[test]
fn check_passes_clean_files() {
    let f = fixture("clean.active", "port = 8080\n");
    let out = active_fmt()
        .arg("--check")
        .arg(&f)
        .output()
        .expect("run active-fmt");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
