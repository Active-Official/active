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
