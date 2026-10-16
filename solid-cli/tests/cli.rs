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

// ------------------------------------------------------------------
// `solid lint` contract (C-042, spec/linter.md).
// ------------------------------------------------------------------

#[test]
fn lint_clean_file_exits_zero() {
    let f = fixture("lint_clean.solid", "port = 8080\n");
    let out = solid().args(["lint"]).arg(&f).output().expect("run solid");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lint_findings_exit_nonzero_with_codes() {
    let f = fixture("lint_dirty.solid", "let idle = 1\nport = 8080\n");
    let out = solid().args(["lint"]).arg(&f).output().expect("run solid");
    assert!(!out.status.success(), "findings must fail");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("L001"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lint_disable_flag_silences_one_rule() {
    let f = fixture("lint_disable.solid", "let idle = 1\nport = 8080\n");
    let out = solid()
        .args(["lint", "--disable-lint", "unused-lets"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Unknown rules are an error listing the valid ones.
    let out = solid()
        .args(["lint", "--disable-lint", "nope"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unused-lets"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lint_needs_a_file_and_refuses_broken_ones() {
    let out = solid().arg("lint").output().expect("run solid");
    assert!(!out.status.success(), "empty file list must fail");
    let f = fixture("lint_broken.solid", "port = [1,\n");
    let out = solid().args(["lint"]).arg(&f).output().expect("run solid");
    assert!(!out.status.success(), "unparseable must fail");
}

#[test]
fn lint_processes_every_file_despite_failures() {
    // Broken first file reports and continues: the dirty file after
    // it is still linted, and the exit stays non-zero.
    let broken = fixture("lint_multi_broken.solid", "port = [1,\n");
    let dirty = fixture("lint_multi_dirty.solid", "let idle = 1\nport = 8080\n");
    let out = solid()
        .args(["lint"])
        .arg(&broken)
        .arg(&dirty)
        .output()
        .expect("run solid");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("L001"),
        "later file must be linted: {stderr}"
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

// ------------------------------------------------------------------
// Converter CLI contract (C-046, spec/converters.md §5).
// ------------------------------------------------------------------

#[test]
fn export_serves_all_three_formats_with_redaction() {
    let f = fixture(
        "conv_export.solid",
        "port = 8080\nwait = 5s\ntoken = secret(\"TOPSECRETEXPORT\")\n[server]\nhost = \"h\"\n",
    );
    for (format, probes) in [
        (
            "json",
            vec!["\"port\": 8080", "\"ms\": 5000", "***SECRET***"],
        ),
        ("yaml", vec!["port: 8080", "ms: 5000", "***SECRET***"]),
        (
            "toml",
            vec!["port = 8080", "\"5000ms\"", "[server]", "***SECRET***"],
        ),
    ] {
        let out = solid()
            .args(["eval", "--format", format])
            .arg(&f)
            .output()
            .expect("run solid");
        assert!(
            out.status.success(),
            "{format}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        for probe in probes {
            assert!(stdout.contains(probe), "{format} missing {probe}: {stdout}");
        }
        assert!(
            !stdout.contains("TOPSECRETEXPORT"),
            "{format} leaked: {stdout}"
        );
    }
}

#[test]
fn export_rejects_unknown_formats() {
    let f = fixture("conv_format.solid", "port = 1\n");
    let out = solid()
        .args(["eval", "--format", "xml"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "unknown format must fail");
}

#[test]
fn convert_subcommand_serves_yaml_and_toml() {
    let f = fixture("conv_sub.solid", "[server]\nport = 8080\n");
    let out = solid()
        .args(["convert", "--to", "yaml"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("port: 8080"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let out = solid()
        .args(["convert", "--to", "toml"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("[server]"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let out = solid()
        .args(["convert", "--to", "xml"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "unknown target must fail");
}

#[test]
fn import_round_trips_json_through_solid() {
    let f = fixture(
        "conv_import.json",
        "{\"server\": {\"port\": 8080}, \"debug\": true}",
    );
    let out = solid()
        .args(["import", "--from", "json"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let source = String::from_utf8_lossy(&out.stdout);
    assert!(source.contains("server {"), "{source}");
    assert!(source.contains("port = 8080"), "{source}");
    // The emitted source evaluates back to the same tree.
    let f2 = fixture("conv_reimport.solid", &source);
    let out = solid().args(["eval"]).arg(&f2).output().expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json = String::from_utf8_lossy(&out.stdout);
    assert!(json.contains("\"port\": 8080"), "{json}");
}

#[test]
fn import_reports_lossy_corners_and_rejects_garbage() {
    // YAML non-string keys: diagnosed, exit 0.
    let f = fixture("conv_yaml_keys.yaml", "123: a\n");
    let out = solid()
        .args(["import", "--from", "yaml"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("W007"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Garbage in: hard failure.
    let f = fixture("conv_garbage.json", "{bad");
    let out = solid()
        .args(["import", "--from", "json"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "garbage must fail");
    // Unknown source format: hard failure.
    let f = fixture("conv_unknown.json", "{}");
    let out = solid()
        .args(["import", "--from", "xml"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "unknown source must fail");
}

// ------------------------------------------------------------------
// `solid generate` contract (C-047, spec/converters.md §5/§7).
// ------------------------------------------------------------------

#[test]
fn generate_serves_all_five_targets() {
    let f = fixture(
        "gen_schema.solid",
        "schema = { port = Int(min = 1), host = String }\nport = 8080\n",
    );
    for (to, probes) in [
        ("jsonschema", vec!["\"type\": \"object\"", "\"minimum\": 1"]),
        ("openapi", vec!["\"openapi\": \"3.1.0\"", "\"minimum\": 1"]),
        (
            "typescript",
            vec!["export interface Config", "port: number"],
        ),
        ("go", vec!["type Config struct", "`json:\"port\"`"]),
        ("rust", vec!["pub struct Config", "pub port: i64"]),
    ] {
        let out = solid()
            .args(["generate", "--from"])
            .arg(&f)
            .args(["--to", to])
            .output()
            .expect("run solid");
        assert!(
            out.status.success(),
            "{to}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        for probe in probes {
            assert!(stdout.contains(probe), "{to} missing {probe}: {stdout}");
        }
    }
}

#[test]
fn generate_refuses_schemaless_documents_and_bad_targets() {
    let f = fixture("gen_noschema.solid", "port = 1\n");
    let out = solid()
        .args(["generate", "--from"])
        .arg(&f)
        .args(["--to", "rust"])
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "schemaless must fail");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no schema"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let f = fixture("gen_schema2.solid", "schema = { port = Int }\nport = 1\n");
    let out = solid()
        .args(["generate", "--from"])
        .arg(&f)
        .args(["--to", "cobol"])
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "unknown target must fail");
}
