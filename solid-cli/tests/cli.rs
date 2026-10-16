//! End-to-end CLI tests for the strict/resilient exit-code policy (C-016).
//!
//! Drives the real `solid` binary so the policy is verified where users
//! feel it: the process exit code.

use std::path::PathBuf;
use std::process::Command;

fn solid() -> Command {
    Command::new(env!("CARGO_BIN_EXE_solid"))
}

fn fixture(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("solid-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    path
}

// ------------------------------------------------------------------
// `solid fmt` contract (C-040/C-041, spec/formatter.md).
// ------------------------------------------------------------------

#[test]
fn fmt_prints_canonical_form_to_stdout() {
    let f = fixture("fmt_stdout.solid", "port=8080\n");
    let out = solid()
        .args(["fmt", "--style", "brace"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "port = 8080\n");
}

#[test]
fn fmt_write_then_check_is_clean() {
    let f = fixture("fmt_write.solid", "[app]\nname=\"svc\"\n");
    let write = solid()
        .args(["fmt", "--write"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        write.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&write.stderr)
    );
    let check = solid()
        .args(["fmt", "--check"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        check.status.success(),
        "written file must check clean; stderr: {}",
        String::from_utf8_lossy(&check.stderr)
    );
    // A second write is a no-op: canonical files are left untouched.
    let again = std::fs::read_to_string(&f).expect("read back");
    assert!(again.contains("name = \"svc\""), "{again}");
}

#[test]
fn fmt_check_flags_unformatted_files() {
    let f = fixture("fmt_dirty.solid", "port=8080\n");
    let out = solid()
        .args(["fmt", "--check"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "--check must exit non-zero");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("would reformat:"),
        "must name the file: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn fmt_check_passes_canonical_files() {
    let f = fixture("fmt_clean.solid", "port = 8080\n");
    let out = solid()
        .args(["fmt", "--check"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn fmt_rejects_contradictory_and_ambiguous_invocations() {
    let f = fixture("fmt_flags.solid", "port = 8080\n");
    for args in [
        vec!["fmt", "--write", "--check"],
        vec!["fmt", "--style", "sideways"],
    ] {
        let out = solid().args(&args).arg(&f).output().expect("run solid");
        assert!(!out.status.success(), "{args:?} must fail");
    }
    // Multiple files need --write or --check.
    let g = fixture("fmt_flags2.solid", "port = 8080\n");
    let out = solid()
        .arg("fmt")
        .arg(&f)
        .arg(&g)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "multi-file stdout must fail");
}

#[test]
fn fmt_write_never_touches_unparseable_files() {
    let before = "port = [1,\n";
    let f = fixture("fmt_broken.solid", before);
    let out = solid()
        .args(["fmt", "--write"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "unparseable must fail");
    assert_eq!(std::fs::read_to_string(&f).expect("read back"), before);
}

#[test]
fn fmt_indent_style_emits_headers() {
    let f = fixture("fmt_indent.solid", "[app]\nname = \"svc\"\n");
    let out = solid()
        .args(["fmt", "--style", "indent"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("app:"), "indent header expected:\n{text}");
}

#[test]
fn fmt_requires_at_least_one_file() {
    for args in [vec!["fmt", "--check"], vec!["fmt", "--write"]] {
        let out = solid().args(&args).output().expect("run solid");
        assert!(!out.status.success(), "{args:?} without files must fail");
    }
}

#[test]
fn fmt_write_stops_at_first_failure_leaving_later_files_untouched() {
    // file1 is valid-but-dirty (gets rewritten), file2 is unparseable
    // (aborts the run, unchanged), file3 is dirty (never reached).
    let f1 = fixture("fmt_multi1.solid", "a=1\n");
    let f2 = fixture("fmt_multi2.solid", "port = [1,\n");
    let f3 = fixture("fmt_multi3.solid", "b=2\n");
    let out = solid()
        .args(["fmt", "--write"])
        .arg(&f1)
        .arg(&f2)
        .arg(&f3)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "run must fail on the broken file");
    assert_eq!(
        std::fs::read_to_string(&f1).expect("read back"),
        "a = 1\n",
        "earlier writes stay done"
    );
    assert_eq!(
        std::fs::read_to_string(&f2).expect("read back"),
        "port = [1,\n",
        "failing file unchanged"
    );
    assert_eq!(
        std::fs::read_to_string(&f3).expect("read back"),
        "b=2\n",
        "later files untouched"
    );
}

#[test]
fn safe_repairs_do_not_fail_strict_mode() {
    // Trailing comma and unclosed string are safe (≥ 0.95) repairs.
    for (name, src) in [
        ("cfg_trailing_comma.solid", "x = [1, 2,]\n"),
        ("cfg_unclosed_string.solid", "x = \"abc\n"),
    ] {
        let f = fixture(name, src);
        let out = solid().arg("check").arg(&f).output().expect("run solid");
        assert!(
            out.status.success(),
            "{name}: safe repairs must pass strict mode; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn medium_repair_passes_resilient_and_fails_strict() {
    // Missing comma is 0.90 → Warning: applied in both modes, but strict
    // exits non-zero because a Warning is present.
    let f = fixture("cfg_missing_comma.solid", "x = [1 2]\n");

    let strict = solid().arg("check").arg(&f).output().expect("run solid");
    assert!(
        !strict.status.success(),
        "strict must exit non-zero on a Warning-severity repair; stdout: {}",
        String::from_utf8_lossy(&strict.stdout)
    );
    assert!(
        String::from_utf8_lossy(&strict.stderr).contains("H003"),
        "the H003 repair must be reported"
    );

    let resilient = solid()
        .args(["check", "--resilient"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        resilient.status.success(),
        "resilient must exit zero; stderr: {}",
        String::from_utf8_lossy(&resilient.stderr)
    );
}

#[test]
fn declined_repair_fails_in_strict_and_heals_in_resilient() {
    // Bracket mismatch is 0.82 → resilient-only.
    let f = fixture("cfg_bracket_mismatch.solid", "x = [1}\n");

    let strict = solid().arg("check").arg(&f).output().expect("run solid");
    assert!(
        !strict.status.success(),
        "strict must reject the declined repair"
    );
    assert!(
        String::from_utf8_lossy(&strict.stderr).contains("E001"),
        "strict must report the syntax error"
    );

    let resilient = solid()
        .args(["eval", "--resilient"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        resilient.status.success(),
        "resilient must heal; stderr: {}",
        String::from_utf8_lossy(&resilient.stderr)
    );
    assert!(
        String::from_utf8_lossy(&resilient.stderr).contains("H004"),
        "the H004 repair must be reported"
    );
}

#[test]
fn unexpected_character_is_rejected_in_both_modes() {
    // Backtick is not part of the language in any position (note: `@`
    // became a real token with semantic blocks in C-024, so it no longer
    // qualifies as an unexpected character).
    let f = fixture("cfg_bad_char.solid", "x = 1 ` 2\n");
    for args in [vec!["check"], vec!["check", "--resilient"]] {
        let out = solid().args(&args).arg(&f).output().expect("run solid");
        assert!(
            !out.status.success(),
            "{args:?} must reject an unexpected character"
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("E005"));
    }
}

#[test]
fn secrets_never_reach_stdout_or_stderr() {
    // A secret-bearing document that also fails must not leak the value —
    // including nested inside collections.
    for (name, src) in [
        (
            "cfg_secret_leak.solid",
            "x = secret(\"TOPSECRETVALUE\") + 1\n",
        ),
        (
            "cfg_secret_nested.solid",
            "x = [secret(\"TOPSECRETVALUE\")] + 1\n",
        ),
        (
            "cfg_secret_table.solid",
            "x = { token = secret(\"TOPSECRETVALUE\") } + 1\n",
        ),
    ] {
        let f = fixture(name, src);
        let out = solid().arg("eval").arg(&f).output().expect("run solid");
        assert!(!out.status.success(), "{name}: expected failure");
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !combined.contains("TOPSECRETVALUE"),
            "{name}: secret leaked to the console: {combined}"
        );
    }
}
