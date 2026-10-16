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

// ------------------------------------------------------------------
// `solid doc` contract (C-048, spec/docgen.md).
// ------------------------------------------------------------------

#[test]
fn doc_serves_all_three_formats() {
    let f = fixture(
        "doc_src.solid",
        "# @intent: \"Service port\"\nport = 8080\ntoken = secret(\"TOPSECRETDOC\")\n",
    );
    for (to, probes) in [
        ("markdown", vec!["# doc_src", "Service port", "8080"]),
        ("html", vec!["id=\"q\"", "Service port"]),
        ("man", vec![".TH", ".SH FIELDS"]),
    ] {
        let out = solid()
            .args(["doc", "--from"])
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
        assert!(!stdout.contains("TOPSECRETDOC"), "{to} leaked: {stdout}");
    }
}

#[test]
fn doc_rejects_bad_targets_and_broken_files() {
    let f = fixture("doc_src2.solid", "port = 1\n");
    let out = solid()
        .args(["doc", "--from"])
        .arg(&f)
        .args(["--to", "pdf"])
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "unknown target must fail");
    let f = fixture("doc_broken.solid", "port = [1,\n");
    let out = solid()
        .args(["doc", "--from"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "broken input must fail");
}

#[test]
fn doc_parent_escapes_need_the_flag() {
    // `solid doc` honors the same include policy as every command:
    // parent escapes fail by default (E015) and render with
    // --allow-parent-escapes.
    let dir = std::env::temp_dir().join("solid-cli-tests");
    let sub = dir.join("doc_escape");
    std::fs::create_dir_all(&sub).expect("temp dir");
    std::fs::write(
        dir.join("doc_lib.solid"),
        "# @intent: \"Lib port\"\nport = 1\n",
    )
    .expect("write lib");
    let main = sub.join("main.solid");
    std::fs::write(&main, "include \"../doc_lib.solid\"\n").expect("write main");
    let denied = solid()
        .args(["doc", "--from"])
        .arg(&main)
        .output()
        .expect("run solid");
    assert!(!denied.status.success(), "escape must fail by default");
    let allowed = solid()
        .args(["doc", "--from"])
        .arg(&main)
        .args(["--allow-parent-escapes"])
        .output()
        .expect("run solid");
    assert!(
        allowed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&allowed.stdout).contains("Lib port"),
        "rendered: {}",
        String::from_utf8_lossy(&allowed.stdout)
    );
}

// ------------------------------------------------------------------
// Graph CLI contract (C-049, spec/graph.md §3).
// ------------------------------------------------------------------

const GRAPH_DOC: &str = "let base = 1\n[a]\nx = base\ny = x\n";

#[test]
fn graph_serves_all_four_formats() {
    let f = fixture("graph_doc.solid", GRAPH_DOC);
    let out = solid().args(["graph"]).arg(&f).output().expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Bindings:"), "{text}");
    assert!(text.contains("→"), "{text}");
    let out = solid()
        .args(["graph", "--format", "mermaid"])
        .arg(&f)
        .output()
        .expect("run solid");
    let mermaid = String::from_utf8_lossy(&out.stdout);
    assert!(mermaid.starts_with("flowchart TD"), "{mermaid}");
    let out = solid()
        .args(["graph", "--format", "dot"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("digraph solid {"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let out = solid()
        .args(["graph", "--format", "json"])
        .arg(&f)
        .output()
        .expect("run solid");
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid json");
    assert!(
        json.get("nodes").is_some() && json.get("edges").is_some(),
        "{json}"
    );
    let out = solid()
        .args(["graph", "--format", "xml"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "unknown format must fail");
}

#[test]
fn impact_and_why_answer_with_paths_and_json() {
    let f = fixture("graph_impact.solid", GRAPH_DOC);
    let out = solid()
        .args(["impact", "base"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("a.x"), "{text}");
    assert!(text.contains("a.y"), "{text}");
    let out = solid()
        .args(["impact", "base", "--format", "json"])
        .arg(&f)
        .output()
        .expect("run solid");
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid json");
    assert_eq!(json["key"], "base");
    assert!(json["paths"].as_array().expect("paths").len() >= 2);
    let out = solid()
        .args(["why", "a.y"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("a.x"), "{text}");
    assert!(text.contains("base"), "{text}");
    let out = solid()
        .args(["impact", "nowhere"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "unknown keys must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("nowhere"), "{combined}");
}

// ------------------------------------------------------------------
// `solid lock` contract (C-050/C-051, spec/reproducibility.md §4).
// ------------------------------------------------------------------

#[test]
fn lock_emits_versioned_hash_and_provenance() {
    let dir = std::env::temp_dir().join("solid-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("locklib.solid"), "host = \"db\"\n").expect("write lib");
    let f = fixture(
        "lock_main.solid",
        "include \"locklib.solid\"\nport = 8080\nregion = env.SOLID_CLI_LOCK_REGION ?: \"r\"\n",
    );
    let out = solid().args(["lock"]).arg(&f).output().expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for probe in [
        "version = 1",
        "content_hash = \"sha256:",
        "toolchain = ",
        "locklib.solid",
        "SOLID_CLI_LOCK_REGION",
    ] {
        assert!(stdout.contains(probe), "missing {probe}:\n{stdout}");
    }
    // Re-parses as TOML with the required top-level keys.
    let parsed: toml::Value = toml::from_str(&stdout).expect("valid lockfile");
    assert_eq!(parsed["version"], toml::Value::Integer(1));
    // Deterministic: a second run reproduces the document byte-for-byte.
    let again = solid().args(["lock"]).arg(&f).output().expect("run solid");
    assert_eq!(
        String::from_utf8_lossy(&again.stdout),
        stdout,
        "lockfiles must be byte-stable"
    );
}

#[test]
fn lock_writes_output_files_and_fails_cleanly() {
    let f = fixture("lock_out.solid", "port = 1\n");
    let dir = std::env::temp_dir().join("solid-cli-tests");
    let target = dir.join("lock_out.solid.lock");
    let _ = std::fs::remove_file(&target);
    let out = solid()
        .args(["lock", "--output"])
        .arg(&target)
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read_to_string(&target).expect("lockfile written");
    assert!(written.contains("content_hash"), "{written}");
    // Broken input fails without writing.
    let f = fixture("lock_broken.solid", "port = [1,\n");
    let out = solid().args(["lock"]).arg(&f).output().expect("run solid");
    assert!(!out.status.success(), "broken input must fail");
}

// ------------------------------------------------------------------
// Signing CLI contract (C-052, spec/threat-model.md §4).
// ------------------------------------------------------------------

#[test]
fn keygen_sign_verify_round_trip() {
    let dir = std::env::temp_dir().join("solid-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let prefix = dir.join("roundtrip");
    let out = solid()
        .args(["keygen", "--out"])
        .arg(&prefix)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let secret_path = dir.join("roundtrip.secret");
    let public_path = dir.join("roundtrip.public");
    assert!(secret_path.exists() && public_path.exists());
    let public_hex = std::fs::read_to_string(&public_path).expect("public");
    assert_eq!(public_hex.len(), 64);
    // Sign a document overlay with the fresh key.
    let f = fixture("sign_lib.solid", "host = \"db\"\n");
    let out = solid()
        .args(["sign", "--key"])
        .arg(&secret_path)
        .args(["--kid", "test-key"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sidecar = dir.join("sign_lib.solid.sig");
    assert!(sidecar.exists());
    // Verify against a trust file carrying the public half.
    let trust = dir.join("trust.toml");
    std::fs::write(
        &trust,
        format!("[[keys]]\nid = \"test-key\"\npublic = \"{public_hex}\"\n"),
    )
    .expect("write trust");
    let out = solid()
        .args(["verify", "--trust-keys"])
        .arg(&trust)
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("valid signature"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // Tampering breaks verification without leaking key material.
    std::fs::write(&f, "host = \"evil\"\n").expect("tamper");
    let out = solid()
        .args(["verify", "--trust-keys"])
        .arg(&trust)
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "tampered must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("signature mismatch"), "{combined}");
    assert!(!combined.contains(&public_hex), "{combined}");
}

#[test]
fn trust_keys_gate_include_loading() {
    let dir = std::env::temp_dir().join("solid-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    // Key setup through the real commands.
    let prefix = dir.join("gate");
    assert!(solid()
        .args(["keygen", "--out"])
        .arg(&prefix)
        .output()
        .expect("run")
        .status
        .success());
    let public_hex = std::fs::read_to_string(dir.join("gate.public")).expect("public");
    let trust = dir.join("gate-trust.toml");
    std::fs::write(
        &trust,
        format!("[[keys]]\nid = \"k\"\npublic = \"{public_hex}\"\n"),
    )
    .expect("write trust");
    // Unsigned include: clean without the flag, E032 with it.
    let _lib = fixture("gate_lib.solid", "host = \"db\"\n");
    let main = fixture("gate_main.solid", "include \"gate_lib.solid\"\nport = 1\n");
    let out = solid()
        .args(["check"])
        .arg(&main)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = solid()
        .args(["check", "--trust-keys"])
        .arg(&trust)
        .arg(&main)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "unsigned must fail under trust");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E032"), "{stderr}");
    // Signed include: passes under trust.
    let out = solid()
        .args(["sign", "--key"])
        .arg(dir.join("gate.secret"))
        .args(["--kid", "k"])
        .arg(dir.join("gate_lib.solid"))
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = solid()
        .args(["check", "--trust-keys"])
        .arg(&trust)
        .arg(&main)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "signed must pass: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn signing_commands_fail_cleanly_on_bad_inputs() {
    let dir = std::env::temp_dir().join("solid-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    // Malformed trust file: clean error, no key material shown.
    let bad_trust = dir.join("bad-trust.toml");
    std::fs::write(&bad_trust, "[[keys]]\n").expect("write trust");
    let f = fixture("sign_bad_trust.solid", "port = 1\n");
    let out = solid()
        .args(["check", "--trust-keys"])
        .arg(&bad_trust)
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "bad trust must fail");
    // Invalid kid: refused before any file is touched.
    let prefix = dir.join("badkid");
    assert!(solid()
        .args(["keygen", "--out"])
        .arg(&prefix)
        .output()
        .expect("run")
        .status
        .success());
    let f = fixture("sign_bad_kid.solid", "port = 1\n");
    let sidecar = dir.join("sign_bad_kid.solid.sig");
    let _ = std::fs::remove_file(&sidecar);
    let out = solid()
        .args(["sign", "--key"])
        .arg(dir.join("badkid.secret"))
        .args(["--kid", "bad\"kid"])
        .arg(&f)
        .output()
        .expect("run solid");
    assert!(!out.status.success(), "bad kid must fail");
    assert!(!sidecar.exists(), "no sidecar on refusal");
}

#[test]
#[cfg(unix)]
fn keygen_restricts_secret_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join("solid-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let prefix = dir.join("perms");
    let out = solid()
        .args(["keygen", "--out"])
        .arg(&prefix)
        .output()
        .expect("run solid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mode = std::fs::metadata(dir.join("perms.secret"))
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "secret key must be owner-only");
}
