//! `active-fmt` binary tests (LANG-D-001, D-004, D-005, D-901).
//!
//! Drives the shipped binary end to end: strict refusal of documents the
//! parser had to repair, the `--style` surface (including `preserve`),
//! byte-exact `--check`, and `--write` skipping canonical files.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_active-fmt"))
}

fn temp(name: &str, contents: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join("active-fmt-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    path
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_document_that_needs_repair_is_refused_not_rewritten() {
    // LANG-D-001: the parser records auto-applied healing instead of
    // failing, so the formatter must refuse here - a silent repair baked
    // into the output defeats `fmt` as a CI gate.
    for (name, src) in [
        ("heal-array.active", "a = [1 2]\n"),
        ("heal-table.active", "a = { x = 1 y = 2 }\n"),
        ("heal-string.active", "x = \"unclosed\n"),
        ("heal-dup.active", "x = 1\nx = 2\n"),
    ] {
        let path = temp(name, src.as_bytes());
        let out = bin().arg(&path).output().expect("run active-fmt");
        assert!(!out.status.success(), "{name} must fail");
        assert!(out.stdout.is_empty(), "{name}: no output on refusal");
        let stderr = stderr_of(&out);
        assert!(
            stderr.contains("cannot format a file that needs repair"),
            "{name}: {stderr}"
        );
        assert!(
            stderr.contains('H') || stderr.contains("W011"),
            "{name}: the refusal names the repair: {stderr}"
        );
    }

    // Negative control: a malformed closer this formatter cannot repair
    // still fails (it always did).
    let path = temp("bad-closer.active", b"a = [1, 2}\n");
    let out = bin().arg(&path).output().expect("run active-fmt");
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("cannot format a file with parse errors"));
}

#[test]
fn the_style_flag_covers_brace_indent_and_preserve() {
    let src = "[t]\na = 1\n";
    let path = temp("style.active", src.as_bytes());
    let brace = bin()
        .args(["--style", "brace"])
        .arg(&path)
        .output()
        .expect("run");
    assert!(brace.status.success(), "{}", stderr_of(&brace));
    let brace_text = String::from_utf8_lossy(&brace.stdout);
    assert!(brace_text.contains("[t]"), "{brace_text}");

    let indent = bin()
        .args(["--style", "indent"])
        .arg(&path)
        .output()
        .expect("run");
    let indent_text = String::from_utf8_lossy(&indent.stdout);
    assert!(indent_text.contains("t:"), "{indent_text}");

    // Preserve re-emits in the source's predominant style: a brace source
    // stays brace, an indented source stays indented.
    let preserved_brace = bin()
        .args(["--style", "preserve"])
        .arg(&path)
        .output()
        .expect("run");
    assert_eq!(
        String::from_utf8_lossy(&preserved_brace.stdout),
        brace_text,
        "preserve keeps the brace source in brace style"
    );
    let indented_src = temp("preserve-indent.active", b"t:\n  a = 1\n");
    let preserved_indent = bin()
        .args(["--style", "preserve"])
        .arg(&indented_src)
        .output()
        .expect("run");
    assert!(
        String::from_utf8_lossy(&preserved_indent.stdout).contains("t:"),
        "preserve keeps the indented source in indentation style"
    );

    // The legacy aliases still work.
    let alias = bin().arg("--bracketed").arg(&path).output().expect("run");
    assert_eq!(String::from_utf8_lossy(&alias.stdout), brace_text);
}

#[test]
fn help_advertises_the_style_flag_and_aliases() {
    // LANG-D-004: the help snapshot pins the documented surface.
    let out = bin().arg("--help").output().expect("run --help");
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("--style"), "{help}");
    assert!(help.contains("bracketed"), "{help}");
    assert!(help.contains("indented"), "{help}");
    assert!(help.contains("--check"), "{help}");
}

#[test]
fn check_compares_bytes_and_reports_non_utf8_cleanly() {
    // LANG-D-005: a non-UTF-8 file fails with a coded parse error, not an
    // encoding panic or a spurious "would reformat".
    let path = temp("not-utf8.active", &[0xff, 0xfe, 0x00, 0x41]);
    let out = bin().arg("--check").arg(&path).output().expect("run");
    assert!(!out.status.success());
    let stderr = stderr_of(&out);
    assert!(stderr.contains("E0") || stderr.contains("E017"), "{stderr}");
    assert!(!stderr.contains("would reformat"), "{stderr}");

    // A canonical file is clean under --check.
    let canonical = temp("canonical.active", b"a = 1\n");
    let out = bin().arg("--check").arg(&canonical).output().expect("run");
    assert!(out.status.success(), "{}", stderr_of(&out));
}

#[test]
fn write_skips_canonical_files_and_keeps_their_mtime() {
    // LANG-D-901: "files already canonical are left untouched (mtime
    // preserved)" - and no false "formatted" report.
    let path = temp("already-canonical.active", b"a = 1\n");
    let before = std::fs::metadata(&path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    std::thread::sleep(std::time::Duration::from_millis(20));
    let out = bin().arg("--write").arg(&path).output().expect("run");
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(out.stdout.is_empty(), "no 'formatted' report on a no-op");
    let after = std::fs::metadata(&path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    assert_eq!(before, after, "canonical file mtime must not change");

    // A dirty file is written and reported.
    let dirty = temp("dirty.active", b"a   =   1\n");
    let out = bin().arg("--write").arg(&dirty).output().expect("run");
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("formatted"),
        "a real rewrite is reported"
    );
    assert_eq!(std::fs::read_to_string(&dirty).expect("read"), "a = 1\n");
}
