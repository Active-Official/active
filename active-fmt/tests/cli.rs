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

#[test]
fn write_reformats_file_in_place() {
    let f = fixture("write.active", "port=8080\n");
    let out = active_fmt()
        .arg("--write")
        .arg(&f)
        .output()
        .expect("run active-fmt");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&f).expect("read back"),
        "port = 8080\n"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("formatted"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn style_flags_convert_between_syntaxes_on_stdout() {
    let brace = fixture("to_indent.active", "[app]\nname = \"x\"\n");
    let out = active_fmt()
        .arg("--indented")
        .arg(&brace)
        .output()
        .expect("run active-fmt");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("app:"), "{text}");

    let indent = fixture("to_brace.active", "app:\n  name = \"x\"\n");
    let out = active_fmt()
        .arg("--bracketed")
        .arg(&indent)
        .output()
        .expect("run active-fmt");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("[app]"), "{text}");
    // Style flags only select output style: the file itself is untouched.
    assert_eq!(
        std::fs::read_to_string(&indent).expect("read back"),
        "app:\n  name = \"x\"\n"
    );
}

#[test]
fn broken_files_fail_loudly_and_untouched() {
    let before = "port = \n";
    let f = fixture("broken.active", before);
    let out = active_fmt()
        .arg("--write")
        .arg(&f)
        .output()
        .expect("run active-fmt");
    assert!(!out.status.success(), "broken input must fail");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cannot format"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read_to_string(&f).expect("read back"), before);
}

#[test]
fn conflicting_flags_are_rejected() {
    let f = fixture("conflict.active", "port = 8080\n");
    let out = active_fmt()
        .arg("--check")
        .arg("--write")
        .arg(&f)
        .output()
        .expect("run active-fmt");
    assert!(!out.status.success(), "conflicting flags must fail");
}
