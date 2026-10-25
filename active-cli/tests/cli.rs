//! End-to-end CLI tests for the strict/resilient exit-code policy (C-016).
//!
//! Drives the real `active` binary so the policy is verified where users
//! feel it: the process exit code.

use std::path::PathBuf;
use std::process::Command;

fn active() -> Command {
    Command::new(env!("CARGO_BIN_EXE_active"))
}

fn fixture(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("active-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    path
}

// ------------------------------------------------------------------
// `active fmt` contract (C-040/C-041, spec/formatter.md).
// ------------------------------------------------------------------

#[test]
fn fmt_prints_canonical_form_to_stdout() {
    let f = fixture("fmt_stdout.active", "port=8080\n");
    let out = active()
        .args(["fmt", "--style", "brace"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "port = 8080\n");
}

#[test]
fn fmt_write_then_check_is_clean() {
    let f = fixture("fmt_write.active", "[app]\nname=\"svc\"\n");
    let write = active()
        .args(["fmt", "--write"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        write.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&write.stderr)
    );
    let check = active()
        .args(["fmt", "--check"])
        .arg(&f)
        .output()
        .expect("run active");
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
    let f = fixture("fmt_dirty.active", "port=8080\n");
    let out = active()
        .args(["fmt", "--check"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "--check must exit non-zero");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("would reformat:"),
        "must name the file: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn fmt_check_passes_canonical_files() {
    let f = fixture("fmt_clean.active", "port = 8080\n");
    let out = active()
        .args(["fmt", "--check"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn fmt_rejects_contradictory_and_ambiguous_invocations() {
    let f = fixture("fmt_flags.active", "port = 8080\n");
    for args in [
        vec!["fmt", "--write", "--check"],
        vec!["fmt", "--style", "sideways"],
    ] {
        let out = active().args(&args).arg(&f).output().expect("run active");
        assert!(!out.status.success(), "{args:?} must fail");
    }
    // Multiple files need --write or --check.
    let g = fixture("fmt_flags2.active", "port = 8080\n");
    let out = active()
        .arg("fmt")
        .arg(&f)
        .arg(&g)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "multi-file stdout must fail");
}

#[test]
fn fmt_write_never_touches_unparseable_files() {
    let before = "port = [1,\n";
    let f = fixture("fmt_broken.active", before);
    let out = active()
        .args(["fmt", "--write"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unparseable must fail");
    assert_eq!(std::fs::read_to_string(&f).expect("read back"), before);
}

#[test]
fn fmt_indent_style_emits_headers() {
    let f = fixture("fmt_indent.active", "[app]\nname = \"svc\"\n");
    let out = active()
        .args(["fmt", "--style", "indent"])
        .arg(&f)
        .output()
        .expect("run active");
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
        let out = active().args(&args).output().expect("run active");
        assert!(!out.status.success(), "{args:?} without files must fail");
    }
}

#[test]
fn fmt_write_stops_at_first_failure_leaving_later_files_untouched() {
    // file1 is valid-but-dirty (gets rewritten), file2 is unparseable
    // (aborts the run, unchanged), file3 is dirty (never reached).
    let f1 = fixture("fmt_multi1.active", "a=1\n");
    let f2 = fixture("fmt_multi2.active", "port = [1,\n");
    let f3 = fixture("fmt_multi3.active", "b=2\n");
    let out = active()
        .args(["fmt", "--write"])
        .arg(&f1)
        .arg(&f2)
        .arg(&f3)
        .output()
        .expect("run active");
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
// `active lint` contract (C-042, spec/linter.md).
// ------------------------------------------------------------------

#[test]
fn lint_clean_file_exits_zero() {
    let f = fixture("lint_clean.active", "port = 8080\n");
    let out = active()
        .args(["lint"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lint_findings_exit_nonzero_with_codes() {
    let f = fixture("lint_dirty.active", "let idle = 1\nport = 8080\n");
    let out = active()
        .args(["lint"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "findings must fail");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("L001"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lint_disable_flag_silences_one_rule() {
    let f = fixture("lint_disable.active", "let idle = 1\nport = 8080\n");
    let out = active()
        .args(["lint", "--disable-lint", "unused-lets"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Unknown rules are an error listing the valid ones.
    let out = active()
        .args(["lint", "--disable-lint", "nope"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unused-lets"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lint_needs_a_file_and_refuses_broken_ones() {
    let out = active().arg("lint").output().expect("run active");
    assert!(!out.status.success(), "empty file list must fail");
    let f = fixture("lint_broken.active", "port = [1,\n");
    let out = active()
        .args(["lint"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unparseable must fail");
}

#[test]
fn lint_processes_every_file_despite_failures() {
    // Broken first file reports and continues: the dirty file after
    // it is still linted, and the exit stays non-zero.
    let broken = fixture("lint_multi_broken.active", "port = [1,\n");
    let dirty = fixture("lint_multi_dirty.active", "let idle = 1\nport = 8080\n");
    let out = active()
        .args(["lint"])
        .arg(&broken)
        .arg(&dirty)
        .output()
        .expect("run active");
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
        ("cfg_trailing_comma.active", "x = [1, 2,]\n"),
        ("cfg_unclosed_string.active", "x = \"abc\n"),
    ] {
        let f = fixture(name, src);
        let out = active().arg("check").arg(&f).output().expect("run active");
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
    let f = fixture("cfg_missing_comma.active", "x = [1 2]\n");

    let strict = active().arg("check").arg(&f).output().expect("run active");
    assert!(
        !strict.status.success(),
        "strict must exit non-zero on a Warning-severity repair; stdout: {}",
        String::from_utf8_lossy(&strict.stdout)
    );
    assert!(
        String::from_utf8_lossy(&strict.stderr).contains("H003"),
        "the H003 repair must be reported"
    );

    let resilient = active()
        .args(["check", "--resilient"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        resilient.status.success(),
        "resilient must exit zero; stderr: {}",
        String::from_utf8_lossy(&resilient.stderr)
    );
}

#[test]
fn declined_repair_fails_in_strict_and_heals_in_resilient() {
    // Bracket mismatch is 0.82 → resilient-only.
    let f = fixture("cfg_bracket_mismatch.active", "x = [1}\n");

    let strict = active().arg("check").arg(&f).output().expect("run active");
    assert!(
        !strict.status.success(),
        "strict must reject the declined repair"
    );
    assert!(
        String::from_utf8_lossy(&strict.stderr).contains("E001"),
        "strict must report the syntax error"
    );

    let resilient = active()
        .args(["eval", "--resilient"])
        .arg(&f)
        .output()
        .expect("run active");
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
    let f = fixture("cfg_bad_char.active", "x = 1 ` 2\n");
    for args in [vec!["check"], vec!["check", "--resilient"]] {
        let out = active().args(&args).arg(&f).output().expect("run active");
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
            "cfg_secret_leak.active",
            "x = secret(\"TOPSECRETVALUE\") + 1\n",
        ),
        (
            "cfg_secret_nested.active",
            "x = [secret(\"TOPSECRETVALUE\")] + 1\n",
        ),
        (
            "cfg_secret_table.active",
            "x = { token = secret(\"TOPSECRETVALUE\") } + 1\n",
        ),
    ] {
        let f = fixture(name, src);
        let out = active().arg("eval").arg(&f).output().expect("run active");
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
        "conv_export.active",
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
        let out = active()
            .args(["eval", "--format", format])
            .arg(&f)
            .output()
            .expect("run active");
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
    let f = fixture("conv_format.active", "port = 1\n");
    let out = active()
        .args(["eval", "--format", "xml"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unknown format must fail");
}

#[test]
fn convert_subcommand_serves_yaml_and_toml() {
    let f = fixture("conv_sub.active", "[server]\nport = 8080\n");
    let out = active()
        .args(["convert", "--to", "yaml"])
        .arg(&f)
        .output()
        .expect("run active");
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
    let out = active()
        .args(["convert", "--to", "toml"])
        .arg(&f)
        .output()
        .expect("run active");
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
    let out = active()
        .args(["convert", "--to", "xml"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unknown target must fail");
}

#[test]
fn import_jsonschema_emits_checked_schema() {
    let f = fixture(
        "conv_schema.json",
        r#"{"type": "object", "properties": {"port": {"type": "integer", "minimum": 1}}, "required": ["port"]}"#,
    );
    let out = active()
        .args(["import", "--from", "jsonschema"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let source = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(source.contains("port = Int(min = 1)"), "{source}");
    // Garbage schema fails loud.
    let g = fixture("conv_schema_bad.json", "{oops");
    let out = active()
        .args(["import", "--from", "jsonschema"])
        .arg(&g)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "garbage schema must fail");
}

#[test]
fn import_round_trips_json_through_active() {
    let f = fixture(
        "conv_import.json",
        "{\"server\": {\"port\": 8080}, \"debug\": true}",
    );
    let out = active()
        .args(["import", "--from", "json"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let source = String::from_utf8_lossy(&out.stdout);
    assert!(source.contains("server {"), "{source}");
    assert!(source.contains("port = 8080"), "{source}");
    // The emitted source evaluates back to the same tree.
    let f2 = fixture("conv_reimport.active", &source);
    let out = active()
        .args(["eval"])
        .arg(&f2)
        .output()
        .expect("run active");
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
    let out = active()
        .args(["import", "--from", "yaml"])
        .arg(&f)
        .output()
        .expect("run active");
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
    let out = active()
        .args(["import", "--from", "json"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "garbage must fail");
    // Unknown source format: hard failure.
    let f = fixture("conv_unknown.json", "{}");
    let out = active()
        .args(["import", "--from", "xml"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unknown source must fail");
}

// ------------------------------------------------------------------
// `active generate` contract (C-047, spec/converters.md §5/§7).
// ------------------------------------------------------------------

#[test]
fn generate_serves_all_five_targets() {
    let f = fixture(
        "gen_schema.active",
        "schema = { port = Int(min = 1), host = String }\nport = 8080\n",
    );
    for (to, probes) in [
        ("jsonschema", vec!["\"type\": \"object\"", "\"minimum\": 1"]),
        ("openapi", vec!["\"openapi\": \"3.1.0\"", "\"minimum\": 1"]),
        (
            "typescript",
            vec!["export interface Config", "port: number"],
        ),
        (
            "go",
            vec![
                "type Config struct",
                "`json:\"port\" validate:\"required,min=1\"`",
            ],
        ),
        ("rust", vec!["pub struct Config", "pub port: i64"]),
    ] {
        let out = active()
            .args(["generate", "--from"])
            .arg(&f)
            .args(["--to", to])
            .output()
            .expect("run active");
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
    let f = fixture("gen_noschema.active", "port = 1\n");
    let out = active()
        .args(["generate", "--from"])
        .arg(&f)
        .args(["--to", "rust"])
        .output()
        .expect("run active");
    assert!(!out.status.success(), "schemaless must fail");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no schema"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let f = fixture("gen_schema2.active", "schema = { port = Int }\nport = 1\n");
    let out = active()
        .args(["generate", "--from"])
        .arg(&f)
        .args(["--to", "cobol"])
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unknown target must fail");
}

// ------------------------------------------------------------------
// `active doc` contract (C-048, spec/docgen.md).
// ------------------------------------------------------------------

#[test]
fn doc_serves_all_three_formats() {
    let f = fixture(
        "doc_src.active",
        "# @intent: \"Service port\"\nport = 8080\ntoken = secret(\"TOPSECRETDOC\")\n",
    );
    for (to, probes) in [
        ("markdown", vec!["# doc_src", "Service port", "8080"]),
        ("html", vec!["id=\"q\"", "Service port"]),
        ("man", vec![".TH", ".SH FIELDS"]),
    ] {
        let out = active()
            .args(["doc", "--from"])
            .arg(&f)
            .args(["--to", to])
            .output()
            .expect("run active");
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
    let f = fixture("doc_src2.active", "port = 1\n");
    let out = active()
        .args(["doc", "--from"])
        .arg(&f)
        .args(["--to", "pdf"])
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unknown target must fail");
    let f = fixture("doc_broken.active", "port = [1,\n");
    let out = active()
        .args(["doc", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "broken input must fail");
}

#[test]
fn doc_parent_escapes_need_the_flag() {
    // `active doc` honors the same include policy as every command:
    // parent escapes fail by default (E015) and render with
    // --allow-parent-escapes.
    let dir = std::env::temp_dir().join("active-cli-tests");
    let sub = dir.join("doc_escape");
    std::fs::create_dir_all(&sub).expect("temp dir");
    std::fs::write(
        dir.join("doc_lib.active"),
        "# @intent: \"Lib port\"\nport = 1\n",
    )
    .expect("write lib");
    let main = sub.join("main.active");
    std::fs::write(&main, "include \"../doc_lib.active\"\n").expect("write main");
    let denied = active()
        .args(["doc", "--from"])
        .arg(&main)
        .output()
        .expect("run active");
    assert!(!denied.status.success(), "escape must fail by default");
    let allowed = active()
        .args(["doc", "--from"])
        .arg(&main)
        .args(["--allow-parent-escapes"])
        .output()
        .expect("run active");
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
    let f = fixture("graph_doc.active", GRAPH_DOC);
    let out = active()
        .args(["graph"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Bindings:"), "{text}");
    assert!(text.contains("→"), "{text}");
    let out = active()
        .args(["graph", "--format", "mermaid"])
        .arg(&f)
        .output()
        .expect("run active");
    let mermaid = String::from_utf8_lossy(&out.stdout);
    assert!(mermaid.starts_with("flowchart TD"), "{mermaid}");
    let out = active()
        .args(["graph", "--format", "dot"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("digraph active {"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let out = active()
        .args(["graph", "--format", "json"])
        .arg(&f)
        .output()
        .expect("run active");
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid json");
    assert!(
        json.get("nodes").is_some() && json.get("edges").is_some(),
        "{json}"
    );
    let out = active()
        .args(["graph", "--format", "xml"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unknown format must fail");
}

#[test]
fn impact_and_why_answer_with_paths_and_json() {
    let f = fixture("graph_impact.active", GRAPH_DOC);
    let out = active()
        .args(["impact", "base"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("a.x"), "{text}");
    assert!(text.contains("a.y"), "{text}");
    let out = active()
        .args(["impact", "base", "--format", "json"])
        .arg(&f)
        .output()
        .expect("run active");
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid json");
    assert_eq!(json["key"], "base");
    assert!(json["paths"].as_array().expect("paths").len() >= 2);
    let out = active()
        .args(["why", "a.y"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("a.x"), "{text}");
    assert!(text.contains("base"), "{text}");
    let out = active()
        .args(["impact", "nowhere"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unknown keys must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("nowhere"), "{combined}");
}

// ------------------------------------------------------------------
// `active lock` contract (C-050/C-051, spec/reproducibility.md §4).
// ------------------------------------------------------------------

#[test]
fn lock_emits_versioned_hash_and_provenance() {
    let dir = std::env::temp_dir().join("active-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("locklib.active"), "host = \"db\"\n").expect("write lib");
    let f = fixture(
        "lock_main.active",
        "include \"locklib.active\"\nport = 8080\nregion = env.ACTIVE_CLI_LOCK_REGION ?: \"r\"\n",
    );
    let out = active()
        .args(["lock"])
        .arg(&f)
        .output()
        .expect("run active");
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
        "locklib.active",
        "ACTIVE_CLI_LOCK_REGION",
    ] {
        assert!(stdout.contains(probe), "missing {probe}:\n{stdout}");
    }
    // Re-parses as TOML with the required top-level keys.
    let parsed: toml::Value = toml::from_str(&stdout).expect("valid lockfile");
    assert_eq!(parsed["version"], toml::Value::Integer(1));
    // Deterministic: a second run reproduces the document byte-for-byte.
    let again = active()
        .args(["lock"])
        .arg(&f)
        .output()
        .expect("run active");
    assert_eq!(
        String::from_utf8_lossy(&again.stdout),
        stdout,
        "lockfiles must be byte-stable"
    );
}

#[test]
fn lock_writes_output_files_and_fails_cleanly() {
    let f = fixture("lock_out.active", "port = 1\n");
    let dir = std::env::temp_dir().join("active-cli-tests");
    let target = dir.join("lock_out.active.lock");
    let _ = std::fs::remove_file(&target);
    let out = active()
        .args(["lock", "--output"])
        .arg(&target)
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read_to_string(&target).expect("lockfile written");
    assert!(written.contains("content_hash"), "{written}");
    // Broken input fails without writing.
    let f = fixture("lock_broken.active", "port = [1,\n");
    let out = active()
        .args(["lock"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "broken input must fail");
}

// ------------------------------------------------------------------
// Signing CLI contract (C-052, spec/threat-model.md §4).
// ------------------------------------------------------------------

#[test]
fn keygen_sign_verify_round_trip() {
    let dir = std::env::temp_dir().join("active-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let prefix = dir.join("roundtrip");
    let out = active()
        .args(["keygen", "--out"])
        .arg(&prefix)
        .output()
        .expect("run active");
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
    let f = fixture("sign_lib.active", "host = \"db\"\n");
    let out = active()
        .args(["sign", "--key"])
        .arg(&secret_path)
        .args(["--kid", "test-key"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sidecar = dir.join("sign_lib.active.sig");
    assert!(sidecar.exists());
    // Verify against a trust file carrying the public half.
    let trust = dir.join("trust.toml");
    std::fs::write(
        &trust,
        format!("[[keys]]\nid = \"test-key\"\npublic = \"{public_hex}\"\n"),
    )
    .expect("write trust");
    let out = active()
        .args(["verify", "--trust-keys"])
        .arg(&trust)
        .arg(&f)
        .output()
        .expect("run active");
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
    let out = active()
        .args(["verify", "--trust-keys"])
        .arg(&trust)
        .arg(&f)
        .output()
        .expect("run active");
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
    let dir = std::env::temp_dir().join("active-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    // Key setup through the real commands.
    let prefix = dir.join("gate");
    assert!(active()
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
    let _lib = fixture("gate_lib.active", "host = \"db\"\n");
    let main = fixture(
        "gate_main.active",
        "include \"gate_lib.active\"\nport = 1\n",
    );
    let out = active()
        .args(["check"])
        .arg(&main)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = active()
        .args(["check", "--trust-keys"])
        .arg(&trust)
        .arg(&main)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unsigned must fail under trust");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E032"), "{stderr}");
    // Signed include: passes under trust.
    let out = active()
        .args(["sign", "--key"])
        .arg(dir.join("gate.secret"))
        .args(["--kid", "k"])
        .arg(dir.join("gate_lib.active"))
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = active()
        .args(["check", "--trust-keys"])
        .arg(&trust)
        .arg(&main)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "signed must pass: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn signing_commands_fail_cleanly_on_bad_inputs() {
    let dir = std::env::temp_dir().join("active-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    // Malformed trust file: clean error, no key material shown.
    let bad_trust = dir.join("bad-trust.toml");
    std::fs::write(&bad_trust, "[[keys]]\n").expect("write trust");
    let f = fixture("sign_bad_trust.active", "port = 1\n");
    let out = active()
        .args(["check", "--trust-keys"])
        .arg(&bad_trust)
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "bad trust must fail");
    // Invalid kid: refused before any file is touched.
    let prefix = dir.join("badkid");
    assert!(active()
        .args(["keygen", "--out"])
        .arg(&prefix)
        .output()
        .expect("run")
        .status
        .success());
    let f = fixture("sign_bad_kid.active", "port = 1\n");
    let sidecar = dir.join("sign_bad_kid.active.sig");
    let _ = std::fs::remove_file(&sidecar);
    let out = active()
        .args(["sign", "--key"])
        .arg(dir.join("badkid.secret"))
        .args(["--kid", "bad\"kid"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "bad kid must fail");
    assert!(!sidecar.exists(), "no sidecar on refusal");
}

#[test]
#[cfg(unix)]
fn keygen_restricts_secret_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join("active-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let prefix = dir.join("perms");
    let out = active()
        .args(["keygen", "--out"])
        .arg(&prefix)
        .output()
        .expect("run active");
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

// ------------------------------------------------------------------
// Agent-native surfaces (CHECKLIST F.003/F.004/F.006).
// ------------------------------------------------------------------

#[test]
fn explain_reports_stable_guidance_per_code() {
    let f = fixture(
        "explain_t003.active",
        "schema = { contact = String(format = \"email\") }\ncontact = \"not-an-email\"\n",
    );
    let out = active()
        .args(["explain", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("T003"), "{stdout}");
    assert!(stdout.contains("format"), "{stdout}");
    assert!(!stdout.contains("not-an-email"), "value leaked: {stdout}");
}

#[test]
fn patch_suggests_rename_and_fill() {
    let f = fixture(
        "patch_typo.active",
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n",
    );
    let out = active()
        .args(["patch", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("@rename(\"Host\", \"host\")"), "{stdout}");
    assert!(stdout.contains("@patch(\"port\", 8080)"), "{stdout}");
}

#[test]
fn patch_is_empty_without_exact_mapping() {
    let f = fixture(
        "patch_t003.active",
        "schema = { contact = String(format = \"email\") }\ncontact = \"x\"\n",
    );
    let out = active()
        .args(["patch", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "T003 must not map: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn capability_handshake_accepts_major_one() {
    let out = active()
        .args(["capability", "--require-major", "1"])
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"schema-introspect\""), "{stdout}");
    assert!(stdout.contains("\"explain\""), "{stdout}");
    let bad = active()
        .args(["capability", "--require-major", "2"])
        .output()
        .expect("run active");
    assert!(!bad.status.success(), "major mismatch must fail");
}

#[test]
fn blob_materialise_flag_embeds_payload_and_default_does_not() {
    let f = fixture("blob_mat.active", "f = blob(\"text/plain\", \"hello\")\n");
    let plain = active()
        .args(["eval"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        plain.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&plain.stderr)
    );
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(
        stdout.contains("blake3:ef8a8525ffd1d32141916f461ccb49c92ee84d57279145268364fb7c1c179eea"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("payload_b64"),
        "identity only without the flag: {stdout}"
    );
    let mat = active()
        .args(["eval", "--materialise-blobs"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        mat.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&mat.stderr)
    );
    assert!(
        String::from_utf8_lossy(&mat.stdout).contains("\"payload_b64\": \"aGVsbG8=\""),
        "{}",
        String::from_utf8_lossy(&mat.stdout)
    );
}

#[test]
fn blob_materialise_refuses_tainted_with_b003() {
    let f = fixture(
        "blob_taint.active",
        "f = blob(\"text/plain\", secret(\"s3cr3t\"))\n",
    );
    let out = active()
        .args(["eval", "--materialise-blobs"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "tainted materialisation must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("B003"), "{stderr}");
    assert!(!stderr.contains("s3cr3t"), "secret leaked: {stderr}");
}

#[test]
fn unknown_budget_profile_fails_fast_with_usage() {
    // A typo'd profile must fail at argument-parse time (clap), not as
    // a runtime bail after the document is read.
    let f = fixture("budget_typo.active", "port = 8080\n");
    let out = active()
        .args(["eval"])
        .arg(&f)
        .args(["--budget-profile", "strictt"])
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unknown profile must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("strict") && stderr.contains("interactive") && stderr.contains("trusted"),
        "clap must list the valid profiles: {stderr}"
    );
}

#[test]
fn budget_profile_wins_over_explicit_flags_with_warning() {
    // A profile plus stale --max-* flags: the profile wins, and a
    // warning names the override so nothing is silently ignored.
    let f = fixture("budget_combo.active", "port = 8080\n");
    let out = active()
        .args(["eval"])
        .arg(&f)
        .args(["--budget-profile", "strict", "--max-nodes", "7"])
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "profile eval must succeed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--budget-profile overrides"),
        "must warn about the ignored flag: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn budget_profile_without_flags_stays_silent() {
    // No conflict, no warning: stderr carries only the JSON value.
    let f = fixture("budget_quiet.active", "port = 8080\n");
    let out = active()
        .args(["eval"])
        .arg(&f)
        .args(["--budget-profile", "trusted"])
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("overrides"),
        "no warning without explicit flags: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn explain_json_matches_text_field_for_field() {
    // Machine-readable output must carry the same content as text:
    // parse it, don't string-match it.
    let f = fixture(
        "explain_json.active",
        "schema = { contact = String(format = \"email\") }\ncontact = \"not-an-email\"\n",
    );
    let out = active()
        .args(["explain", "--from"])
        .arg(&f)
        .arg("--format")
        .arg("json")
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let items: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout must be a JSON array");
    let arr = items.as_array().expect("top level array");
    assert_eq!(arr.len(), 1, "{stdout}");
    let e = &arr[0];
    assert_eq!(e["code"], "T003");
    assert_eq!(e["path"], "contact");
    assert!(e["span"].as_str().is_some(), "{stdout}");
    assert!(
        e["guidance"].as_str().unwrap_or("").contains("format"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("not-an-email"),
        "value leaked into JSON: {stdout}"
    );
    // An unsupported format fails closed, like every other command.
    let bad = active()
        .args(["explain", "--from"])
        .arg(&f)
        .args(["--format", "yaml"])
        .output()
        .expect("run active");
    assert!(!bad.status.success(), "bad format must fail");
    // A clean file explains to an empty array, not empty output.
    let f2 = fixture("explain_json_empty.active", "port = 8080\n");
    let out2 = active()
        .args(["explain", "--from"])
        .arg(&f2)
        .arg("--format")
        .arg("json")
        .output()
        .expect("run active");
    assert!(out2.status.success());
    let items2: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out2.stdout)).expect("JSON array");
    assert_eq!(items2, serde_json::json!([]));
}

#[test]
fn patch_json_carries_kind_path_and_directive() {
    let f = fixture(
        "patch_json.active",
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n",
    );
    let out = active()
        .args(["patch", "--from"])
        .arg(&f)
        .arg("--format")
        .arg("json")
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let items: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout must be a JSON array");
    let arr = items.as_array().expect("top level array");
    assert!(!arr.is_empty(), "{stdout}");
    let by_kind = |k: &str| {
        arr.iter()
            .find(|s| s["kind"] == k)
            .unwrap_or_else(|| panic!("missing {k} suggestion: {stdout}"))
            .clone()
    };
    let rename = by_kind("rename");
    assert_eq!(rename["path"], "Host");
    assert_eq!(rename["directive"], "@rename(\"Host\", \"host\")");
    let fill = by_kind("fill");
    assert_eq!(fill["path"], "port");
    assert_eq!(fill["directive"], "@patch(\"port\", 8080)");
    // Empty result is an empty array, not empty output.
    let f2 = fixture(
        "patch_json_empty.active",
        "schema = { contact = String(format = \"email\") }\ncontact = \"x\"\n",
    );
    let out2 = active()
        .args(["patch", "--from"])
        .arg(&f2)
        .arg("--format")
        .arg("json")
        .output()
        .expect("run active");
    assert!(out2.status.success());
    let items2: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out2.stdout)).expect("JSON array");
    assert_eq!(items2, serde_json::json!([]));
}

#[test]
fn capability_tools_all_map_to_real_subcommands() {
    // The capability descriptor is the agents' contract: every tool it
    // advertises must resolve to a real CLI subcommand, or agents are
    // sent to commands that do not exist. The `format` tool maps to
    // `fmt`; the `graph` tool fans out to graph/impact/why.
    let schema_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../agent-kit/capability.schema.json");
    let doc: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&schema_path).expect("read capability schema"),
    )
    .expect("schema parses");
    let tools = doc["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty(), "schema must advertise tools");
    let mut mapping: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    mapping.insert("check", vec!["check"]);
    mapping.insert("eval", vec!["eval"]);
    mapping.insert("format", vec!["fmt"]);
    mapping.insert("lint", vec!["lint"]);
    mapping.insert("convert", vec!["convert"]);
    mapping.insert("graph", vec!["graph", "impact", "why"]);
    mapping.insert("generate", vec!["generate"]);
    mapping.insert("schema-introspect", vec!["inspect"]);
    mapping.insert("explain", vec!["explain"]);
    mapping.insert("patch", vec!["patch"]);
    mapping.insert("repair", vec!["repair"]);
    mapping.insert("compatibility", vec!["compatibility"]);
    mapping.insert("capability", vec!["capability"]);
    for tool in tools {
        let name = tool["name"].as_str().expect("tool name");
        let cmds = mapping
            .get(name)
            .unwrap_or_else(|| panic!("schema tool {name} has no CLI mapping — update the test"));
        for cmd in cmds {
            let out = active()
                .arg(cmd)
                .arg("--help")
                .output()
                .expect("run active");
            assert!(
                out.status.success(),
                "tool {name} maps to missing/broken subcommand {cmd}"
            );
        }
    }
}

#[test]
fn compatibility_reports_breaking_changes_and_fails_closed() {
    let old = fixture(
        "compat_old.active",
        "schema = { host = String }\nhost = \"x\"\n",
    );
    let new = fixture(
        "compat_new.active",
        "schema = { host = String, port = Int }\nhost = \"x\"\nport = 1\n",
    );
    let out = active()
        .args(["compatibility"])
        .arg(&old)
        .arg(&new)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "breaking change must fail");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("C040"), "{stdout}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E040"), "{stderr}");
}

#[test]
fn compatibility_json_reports_compatible_schemas() {
    let src = "schema = { host = String }\nhost = \"x\"\n";
    let old = fixture("compat_same_old.active", src);
    let new = fixture("compat_same_new.active", src);
    let out = active()
        .args(["compatibility", "--format", "json"])
        .arg(&old)
        .arg(&new)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let findings: serde_json::Value = serde_json::from_str(&stdout).expect("JSON array");
    assert_eq!(findings, serde_json::json!([]), "{stdout}");
}

#[test]
fn repair_reaches_clean_and_writes_output() {
    let f = fixture(
        "repair_clean.active",
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n",
    );
    let out = active()
        .args(["repair", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("repair: clean"), "{stdout}");
    assert!(stdout.contains("@patch(\"port\", 8080)"), "{stdout}");
    assert!(stdout.contains("@rename(\"Host\", \"host\")"), "{stdout}");
}

#[test]
fn repair_reports_json_plan_and_output_file() {
    let f = fixture(
        "repair_json.active",
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n",
    );
    let outpath = std::env::temp_dir().join("active-cli-tests-repaired.active");
    let out = active()
        .args(["repair", "--from"])
        .arg(&f)
        .args(["--format", "json", "--output"])
        .arg(&outpath)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON plan");
    assert_eq!(plan["clean"], true);
    assert_eq!(plan["steps"].as_array().expect("steps").len(), 2);
    let written = std::fs::read_to_string(&outpath).expect("output written");
    assert!(written.contains("@patch(\"port\", 8080)"), "{written}");
    // The written artifact re-evaluates clean: the plan is executable.
    let check = active()
        .args(["repair", "--from"])
        .arg(&outpath)
        .output()
        .expect("run active");
    assert!(
        check.status.success(),
        "repaired file must evaluate clean: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn repair_fails_closed_on_unrepairable() {
    let f = fixture(
        "repair_bad.active",
        "schema = { contact = String(format = \"email\") }\ncontact = \"not-an-email\"\n",
    );
    let out = active()
        .args(["repair", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unrepairable must fail");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("not clean"), "{stdout}");
    // H008 terminal finding rides the diagnostic channel (stderr).
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("H008"), "{stderr}");
}

#[test]
fn eval_path_selects_demanded_cone() {
    let f = fixture(
        "cone.active",
        "server = { host = \"h\", ports = [80, 443] }\n",
    );
    let out = active()
        .args(["eval", "--path", "server.ports[1]"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "443",
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn eval_path_unknown_fails_closed() {
    let f = fixture("cone_missing.active", "server = { host = \"h\" }\n");
    let out = active()
        .args(["eval", "--path", "server.missing"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unknown path must fail");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unknown path"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn materialise_applies_to_cone_not_whole_tree() {
    // A tainted blob OUTSIDE the demanded cone must not fail the
    // fetch: the demand applies to the emitted cone only.
    let f = fixture(
        "cone_taint.active",
        "keep = blob(\"text/plain\", \"hello\")\ntainted = blob(\"text/plain\", secret(\"s3cr3t\"))\n",
    );
    let out = active()
        .args(["eval", "--materialise-blobs", "--path", "keep"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "cone fetch must succeed despite tainted sibling: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("payload_b64"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn graph_query_closed_set_end_to_end() {
    let f = fixture(
        "gq.active",
        "a = 1\nb = a\nc = b\ns = secret(\"s3cr3t\")\nt = s\n",
    );
    let run = |args: &[&str]| {
        let mut cmd = active();
        cmd.arg("graph").arg(&f);
        for a in args {
            cmd.arg(a);
        }
        cmd.output().expect("run active")
    };
    let out = run(&["query", "--query", "dependents-of", "--key", "a"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains('b') && text.contains('c'), "{text}");

    let out = run(&["query", "--query", "shortest-path", "--key", "a,c"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["a", "b", "c"]);

    let out = run(&["query", "--query", "secret-tainted-cone"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains('t'), "{text}");
    assert!(!text.contains("s3cr3t"), "secret leaked: {text}");

    let out = run(&["query", "--query", "nope"]);
    assert!(!out.status.success(), "unknown query must fail");
}
#[test]
fn capability_handshake_accepts_advertised_rejects_rest() {
    let ok = fixture(
        "cap_ok.active",
        "active_capabilities = [\"eval\", \"schema-introspect\"]\nport = 1\n",
    );
    let out = active()
        .args(["capability", "--check-doc"])
        .arg(&ok)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "advertised caps must pass: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bad = fixture(
        "cap_bad.active",
        "active_capabilities = [\"eval\", \"teleport\"]\nport = 1\n",
    );
    let out = active()
        .args(["capability", "--check-doc"])
        .arg(&bad)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "unadvertised caps must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("teleport"), "{stderr}");
    let out = active()
        .args(["capability", "--profile", "strict"])
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "profiles must print: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let profile: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("profile JSON");
    assert_eq!(profile["budget"], "strict");
}

#[test]
fn sandbox_gate_applies_budget_and_refuses_excess() {
    // Strict sandbox refuses a doc declaring agent-only tools.
    let f = fixture(
        "sandbox_refuse.active",
        "active_capabilities = [\"repair\"]\nport = 1\n",
    );
    let out = active()
        .args(["eval", "--sandbox", "strict"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success(), "sandbox must refuse excess caps");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("refuses"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Same doc under agent sandbox evaluates fine.
    let out = active()
        .args(["eval", "--sandbox", "agent"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "agent sandbox must accept repair: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn audit_trail_is_valid_active_without_secrets() {
    let f = fixture(
        "audit_src.active",
        "token = secret(\"s3cr3t-audit\")\nport = 8080\n",
    );
    let audit = std::env::temp_dir().join("active-cli-tests-audit.active");
    let out = active()
        .args(["eval", "--audit-trail"])
        .arg(&audit)
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&audit).expect("audit written");
    assert!(text.contains("outcome = \"clean\""), "{text}");
    assert!(!text.contains("s3cr3t-audit"), "audit leaked: {text}");
    assert!(text.contains("sha256:"), "{text}");
    // The audit artifact is itself valid Active.
    let check = active()
        .args(["check"])
        .arg(&audit)
        .output()
        .expect("run active");
    assert!(
        check.status.success(),
        "audit file must check clean: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn lint_json_and_sarif_are_machine_readable() {
    let f = fixture(
        "lint_formats.active",
        "[server]\nreadTimeout = 1\nmax_retries = 2\n",
    );
    let json_out = active()
        .args(["lint", "--format", "json"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!json_out.status.success(), "findings exit non-zero");
    let json: serde_json::Value = serde_json::from_slice(&json_out.stdout).expect("stdout is JSON");
    let rows = json.as_array().expect("array");
    assert!(!rows.is_empty(), "{json}");
    assert_eq!(rows[0]["code"], "L008");
    assert!(rows[0]["message"].is_string());

    let sarif_out = active()
        .args(["lint", "--format", "sarif"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!sarif_out.status.success());
    let sarif: serde_json::Value =
        serde_json::from_slice(&sarif_out.stdout).expect("stdout is SARIF");
    assert_eq!(sarif["version"], "2.1.0");
    assert_eq!(sarif["runs"][0]["tool"]["driver"]["name"], "active");
    assert!(!sarif["runs"][0]["results"]
        .as_array()
        .expect("results")
        .is_empty());
}

#[test]
fn inspect_path_reports_text_and_redacts() {
    let f = fixture(
        "inspect_path.active",
        "schema = { port = Int(min = 1, max = 65535, description = \"listen port\") }\nport = 8080\ntoken = secret(\"hunter2-inspect\")\n",
    );
    let text = active()
        .args(["inspect", "--from"])
        .arg(&f)
        .args(["--path", "port", "--format", "text"])
        .output()
        .expect("run active");
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let body = String::from_utf8_lossy(&text.stdout);
    assert!(body.contains("constraint: min 1"), "{body}");
    assert!(body.contains("description: listen port"), "{body}");
    assert!(body.contains("8080"), "{body}");

    let secret = active()
        .args(["inspect", "--from"])
        .arg(&f)
        .args(["--path", "token"])
        .output()
        .expect("run active");
    assert!(secret.status.success());
    let body = String::from_utf8_lossy(&secret.stdout);
    assert!(body.contains("***SECRET***"), "{body}");
    assert!(!body.contains("hunter2"), "raw secret leaked: {body}");
}

#[test]
fn partial_repair_json_describes_the_applied_subset() {
    let f = fixture(
        "partial_json.active",
        "schema = { a = Int(default = 1), b = Int(default = 2) }\nx = 1\n",
    );
    let out = active()
        .args(["repair", "--from"])
        .arg(&f)
        .args(["--apply-step", "0", "--format", "json"])
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("repair JSON");
    assert_eq!(report["steps"][0]["status"], "applied", "{report}");
    assert!(
        report["steps"][1]["status"].get("skipped").is_some(),
        "unselected step must be skipped: {report}"
    );
    let source = report["repaired_source"].as_str().expect("source");
    assert_eq!(
        source.matches("@patch").count(),
        1,
        "only the selected directive may be reported: {source}"
    );
    assert_eq!(report["clean"], false, "{report}");
    assert!(
        report["final_codes"]
            .as_array()
            .expect("final_codes")
            .iter()
            .any(|code| code.as_str().is_some_and(|c| c.starts_with("T004@"))),
        "codes must carry their paths: {report}"
    );
}

#[test]
fn audit_hash_reproduces_under_the_profile_override() {
    let f = fixture(
        "audit_profile.active",
        "active_profile = \"a\"\nprofile \"a\" { x = 1 }\nprofile \"b\" { x = 42 }\nx = 0\n",
    );
    let audit = std::env::temp_dir().join(format!("audit-profile-{}.active", std::process::id()));
    let out = active()
        .args(["eval", "--profile", "b", "--audit-trail"])
        .arg(&audit)
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&audit).expect("audit written");
    assert!(text.contains("profile = \"b\""), "{text}");
    let hash = text
        .split("result = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no result hash: {text}"));
    let replay = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--replay", "--profile", "b", "--expect", hash])
        .output()
        .expect("run active");
    let report: serde_json::Value = serde_json::from_slice(&replay.stdout).expect("replay JSON");
    assert_eq!(report["ok"], true, "{report}");
}

#[test]
fn exports_redact_structural_material() {
    let f = fixture(
        "export_redact.active",
        "let t = env.R20SECRET\nk = secret(t)\nleak = t\ntable = { inner = t }\ncomposed = \"pre-${t}\"\n",
    );
    for (label, args) in [
        ("eval-json", vec!["eval", "--format", "json"]),
        ("convert-toml", vec!["convert", "--to", "toml"]),
        ("doc-md", vec!["doc", "--to", "markdown", "--from"]),
        ("lock", vec!["lock"]),
    ] {
        let out = active()
            .args(&args)
            .arg(&f)
            .env("R20SECRET", "ENVSECRET-export")
            .output()
            .expect("run active");
        assert!(
            out.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !text.contains("ENVSECRET"),
            "{label} leaked structural material: {text}"
        );
    }
    // The content hash is over the redacted tree: changing the material
    // value must not change it.
    let hash = |value: &str| {
        let out = active()
            .arg("lock")
            .arg(&f)
            .env("R20SECRET", value)
            .output()
            .expect("run active");
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines()
            .find(|line| line.starts_with("content_hash"))
            .unwrap_or_else(|| panic!("no hash: {text}"))
            .to_string()
    };
    assert_eq!(hash("A-MATERIAL-VALUE"), hash("B-MATERIAL-VALUE"));
}

#[test]
fn audit_hashes_the_full_redacted_tree_and_records_failures() {
    let f = fixture("audit_scope.active", "a = { b = 1 }\nc = 2\n");
    let audit = std::env::temp_dir().join(format!("audit-scope-{}.active", std::process::id()));
    let out = active()
        .args(["eval", "--path", "a.b", "--audit-trail"])
        .arg(&audit)
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&audit).expect("audit written");
    let hash = text
        .split("result = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no result hash: {text}"));
    let replay = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--replay", "--expect", hash])
        .output()
        .expect("run active");
    let report: serde_json::Value = serde_json::from_slice(&replay.stdout).expect("replay JSON");
    assert_eq!(report["ok"], true, "{report}");

    // A strict validation failure still writes an audit with no result.
    let bad = fixture("audit_bad.active", "schema = { n = Int }\nn = \"x\"\n");
    let bad_audit = std::env::temp_dir().join(format!("audit-bad-{}.active", std::process::id()));
    let out = active()
        .arg("eval")
        .arg("--audit-trail")
        .arg(&bad_audit)
        .arg(&bad)
        .output()
        .expect("run active");
    assert_eq!(out.status.code(), Some(1));
    let text = std::fs::read_to_string(&bad_audit).expect("failed audit written");
    assert!(text.contains("outcome = \"failed\""), "{text}");
    assert!(text.contains("result = null"), "{text}");
    assert!(text.contains("T002"), "{text}");
}

#[test]
fn debug_and_inspect_redact_interpolated_secret_sources() {
    let f = fixture(
        "debug_interp.active",
        "x = \"hunter2-interp\"\nmsg = \"prefix ${x}\"\nk = secret(x)\n",
    );
    let debug = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--format", "json"])
        .output()
        .expect("run active");
    assert!(
        debug.status.success(),
        "{}",
        String::from_utf8_lossy(&debug.stderr)
    );
    let body = String::from_utf8_lossy(&debug.stdout);
    assert!(
        !body.contains("hunter2"),
        "interpolated secret in debug steps: {body}"
    );
    let report: serde_json::Value = serde_json::from_slice(&debug.stdout).expect("JSON");
    let msg = report["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["path"] == "msg")
        .unwrap_or_else(|| panic!("msg step missing: {body}"));
    assert_eq!(msg["value"], "***SECRET***", "{body}");
    assert_eq!(msg["tainted"], true, "{body}");

    let inspect = active()
        .args(["inspect", "--from"])
        .arg(&f)
        .args(["--path", "msg"])
        .output()
        .expect("run active");
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&inspect.stdout).expect("JSON");
    assert_eq!(report["tainted"], true, "{report}");
    assert_eq!(report["value"], "***SECRET***", "{report}");
    assert!(!report.to_string().contains("hunter2"), "{report}");
}

#[test]
fn repair_browser_type_error_messages_are_value_free() {
    let f = fixture(
        "browser_t001.active",
        "x = \"hunter2-LOC\"\nk = secret(x)\nbad = x < 1\n",
    );
    let out = active()
        .args(["repair", "--from"])
        .arg(&f)
        .arg("--browser")
        .output()
        .expect("run active");
    let body = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !body.contains("hunter2"),
        "type error leaked a value: {body}"
    );
    assert!(body.contains("T001"), "expected a type diagnostic: {body}");
}

#[test]
fn debug_steps_break_and_replay_are_secret_safe() {
    let f = fixture(
        "debug.active",
        "schema = { port = Int(max = 10) }\nport = 999\nunrelated = 1\ntoken = secret(\"hunter2-debug\")\n",
    );
    let steps = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--format", "text", "--resilient"])
        .output()
        .expect("run active");
    assert!(
        steps.status.success(),
        "{}",
        String::from_utf8_lossy(&steps.stderr)
    );
    let body = String::from_utf8_lossy(&steps.stdout);
    assert!(body.contains("#0"), "{body}");
    assert!(body.contains("***SECRET***"), "{body}");
    assert!(!body.contains("hunter2"), "raw secret in steps: {body}");

    let break_out = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--break", "taint", "--format", "text", "--resilient"])
        .output()
        .expect("run active");
    assert!(break_out.status.success());
    assert!(String::from_utf8_lossy(&break_out.stdout).contains("break at #"));

    let replay = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--replay", "--format", "text", "--resilient"])
        .output()
        .expect("run active");
    assert!(replay.status.success());
    assert!(String::from_utf8_lossy(&replay.stdout).contains("replay: OK"));

    let reduce = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--minimize", "T003", "--format", "text"])
        .output()
        .expect("run active");
    assert!(
        reduce.status.success(),
        "{}",
        String::from_utf8_lossy(&reduce.stderr)
    );
    let body = String::from_utf8_lossy(&reduce.stdout);
    assert!(body.contains("keeping T003"), "{body}");
    assert!(
        !body.contains("hunter2"),
        "minimized body carries secret material: {body}"
    );
}

#[test]
fn viz_emitters_are_machine_readable_and_redacted() {
    let f = fixture(
        "viz.active",
        "schema = { port = Int(min = 1), token = String }\nport = 8080\ntoken = secret(\"hunter2-viz\")\n",
    );
    let graph = active()
        .args(["viz", "graph", "--from"])
        .arg(&f)
        .args(["--format", "mermaid"])
        .output()
        .expect("run active");
    assert!(graph.status.success());
    assert!(String::from_utf8_lossy(&graph.stdout).contains("flowchart TD"));

    for kind in ["taint", "budget", "schema"] {
        let out = active()
            .args(["viz", kind, "--from"])
            .arg(&f)
            .output()
            .expect("run active");
        assert!(
            out.status.success(),
            "{kind}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let body = String::from_utf8_lossy(&out.stdout);
        assert!(!body.contains("hunter2"), "{kind} leaked a secret: {body}");
        serde_json::from_str::<serde_json::Value>(&body)
            .unwrap_or_else(|e| panic!("{kind} is not JSON: {e}"));
    }

    let repair_fixture = fixture(
        "viz_repair.active",
        "schema = { port = Int(default = 8080) }\n",
    );
    let repair = active()
        .args(["viz", "repair", "--from"])
        .arg(&repair_fixture)
        .output()
        .expect("run active");
    assert!(repair.status.success());
    let plan: serde_json::Value = serde_json::from_slice(&repair.stdout).expect("repair JSON");
    assert!(!plan["steps"].as_array().expect("steps").is_empty());
}

#[test]
fn viz_unknown_kind_fails_closed() {
    let f = fixture("viz_bad.active", "port = 8080\n");
    let out = active()
        .args(["viz", "teleport", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown viz kind"));
}

#[test]
fn repair_browser_and_partial_apply() {
    let f = fixture(
        "repair_browser.active",
        "schema = { port = Int(default = 8080), host = String(default = \"x\") }\n",
    );
    let browser = active()
        .args(["repair", "--from"])
        .arg(&f)
        .arg("--browser")
        .output()
        .expect("run active");
    assert!(browser.status.success());
    let model: serde_json::Value = serde_json::from_slice(&browser.stdout).expect("browser JSON");
    assert!(model["diagnostics"].is_array());
    assert!(model["plan"]["steps"].is_array());
    assert!(model["plan"].get("repaired_source").is_none());

    let partial = active()
        .args(["repair", "--from"])
        .arg(&f)
        .args(["--apply-step", "0", "--format", "text"])
        .output()
        .expect("run active");
    assert!(
        partial.status.success(),
        "{}",
        String::from_utf8_lossy(&partial.stderr)
    );
    assert!(String::from_utf8_lossy(&partial.stdout).contains("selected: 1 directives"));

    let cherry = active()
        .args(["repair", "--from"])
        .arg(&f)
        .args(["--cherry-pick", "host", "--format", "text"])
        .output()
        .expect("run active");
    assert!(cherry.status.success());
    let body = String::from_utf8_lossy(&cherry.stdout);
    assert!(body.contains("apply @patch(\"host\""), "{body}");
}

#[test]
fn query_aliases_report_schema_mode_and_budget() {
    let f = fixture(
        "query_aliases.active",
        "schema = { port = Int(min = 1, max = 10) }\nport = 5\n",
    );
    let schema = active()
        .args(["schema", "inspect", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(schema.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&schema.stdout).expect("schema JSON");
    assert_eq!(doc["version"], 2);

    let mode = active()
        .args(["mode", "--from"])
        .arg(&f)
        .args(["--format", "json"])
        .output()
        .expect("run active");
    assert!(mode.status.success());
    let mode_doc: serde_json::Value = serde_json::from_slice(&mode.stdout).expect("mode JSON");
    assert_eq!(mode_doc["mode"], "config");
    assert_eq!(mode_doc["marker"], false);

    let budget = active()
        .args(["budget", "explain", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(budget.status.success());
    let body = String::from_utf8_lossy(&budget.stdout);
    assert!(body.contains("nodes: used"), "{body}");
    assert!(body.contains("headroom"), "{body}");
}

#[test]
fn debug_minimize_works_on_unparseable_source() {
    let f = fixture(
        "debug_broken.active",
        "ok = 1\nbad = = 2\nnoise = 3\nmore = 4\n",
    );
    let out = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--minimize", "E002", "--format", "text"])
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(body.contains("keeping E002"), "{body}");
    assert!(body.contains("bad = = 2"), "target line kept: {body}");
    assert!(!body.contains("noise = 3"), "noise removed: {body}");
}

#[test]
fn debug_text_prints_steps_before_the_verdict() {
    let f = fixture("debug_order.active", "token = secret(\"x\")\nport = 1\n");
    let out = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--break", "taint", "--format", "text"])
        .output()
        .expect("run active");
    let body = String::from_utf8_lossy(&out.stdout);
    let step = body.find("#0").expect("steps printed");
    let verdict = body.find("break at").expect("verdict printed");
    assert!(step < verdict, "steps must precede the verdict: {body}");
}

#[test]
fn mode_marker_ignores_longer_keys() {
    let f = fixture("mode_lookalike.active", "active_mode_extra = 1\nport = 1\n");
    let out = active()
        .args(["mode", "--from"])
        .arg(&f)
        .args(["--format", "json"])
        .output()
        .expect("run active");
    assert!(out.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(doc["marker"], false);
}

#[test]
fn viz_rejects_flags_that_do_not_apply() {
    let f = fixture("viz_flags.active", "port = 1\n");
    let format = active()
        .args(["viz", "taint", "--from"])
        .arg(&f)
        .args(["--format", "mermaid"])
        .output()
        .expect("run active");
    assert!(!format.status.success());
    let goal = active()
        .args(["viz", "taint", "--from"])
        .arg(&f)
        .args(["--goal", "minimize-diff"])
        .output()
        .expect("run active");
    assert!(!goal.status.success());
}

#[test]
fn viz_repair_handles_unparseable_source_and_reports_outcome() {
    let f = fixture("viz_repair_broken.active", "port = = 2\nnoise = 1\n");
    let out = active()
        .args(["viz", "repair", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&out.stdout).expect("plan JSON");
    assert_eq!(plan["unrepairable"], true);
}

#[test]
fn nested_active_mode_is_not_a_root_marker() {
    let f = fixture(
        "mode_nested.active",
        "[server]\nactive_mode = \"data\"\nport = 1\n",
    );
    let out = active()
        .args(["mode", "--from"])
        .arg(&f)
        .args(["--format", "json"])
        .output()
        .expect("run active");
    assert!(out.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(doc["marker"], false);
    assert_eq!(doc["mode"], "config");
}

#[test]
fn viz_repair_omits_source_and_secrets() {
    let f = fixture(
        "viz_repair_secret.active",
        "schema = { token = Int }\ntoken = secret(\"hunter2-viz-repair\")\n",
    );
    let out = active()
        .args(["viz", "repair", "--from"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(out.status.success());
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(
        !body.contains("hunter2"),
        "raw secret in viz repair: {body}"
    );
    assert!(!body.contains("repaired_source"), "{body}");
}

#[test]
fn graph_query_honors_outer_format_and_trust_gate() {
    let f = fixture("graph_query_flags.active", "[s]\na = 1\nb = a\n");
    let json = active()
        .args(["graph"])
        .arg(&f)
        .args([
            "--format",
            "json",
            "query",
            "--query",
            "dependents-of",
            "--key",
            "s.a",
        ])
        .output()
        .expect("run active");
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&json.stdout).expect("json output");
    assert_eq!(doc["query"], "dependents-of");

    let missing = std::env::temp_dir().join("active-missing-trust.toml");
    let trust = active()
        .args(["graph"])
        .arg(&f)
        .args(["--trust-keys"])
        .arg(&missing)
        .args(["query", "--query", "dependents-of", "--key", "s.a"])
        .output()
        .expect("run active");
    assert!(
        !trust.status.success(),
        "a missing trust set must fail, not be ignored"
    );
}

#[test]
fn repair_browser_resolves_includes_like_repair() {
    let dir = std::env::temp_dir().join(format!("active-browser-include-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("inc.active"), "host = \"x\"\n").expect("include");
    let main = dir.join("main.active");
    std::fs::write(
        &main,
        "include \"inc.active\"\nschema = { port = Int(default = 8080) }\n",
    )
    .expect("main");

    let browser = active()
        .args(["repair", "--from"])
        .arg(&main)
        .arg("--browser")
        .output()
        .expect("run active");
    assert!(
        browser.status.success(),
        "{}",
        String::from_utf8_lossy(&browser.stderr)
    );
    let model: serde_json::Value = serde_json::from_slice(&browser.stdout).expect("model");
    assert_eq!(model["plan"]["clean"], true, "{model}");
    assert!(
        !model["plan"]["steps"].as_array().expect("steps").is_empty(),
        "{model}"
    );

    let repair = active()
        .args(["repair", "--from"])
        .arg(&main)
        .output()
        .expect("run active");
    assert!(
        repair.status.success(),
        "CLI parity: {}",
        String::from_utf8_lossy(&repair.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn graph_query_is_strict_like_eval() {
    let f = fixture(
        "graph_strict.active",
        "schema = { port = Int }\nhost = \"x\"\n",
    );
    let out = active()
        .args(["graph"])
        .arg(&f)
        .args(["query", "--query", "secret-tainted-cone"])
        .output()
        .expect("run active");
    assert!(
        !out.status.success(),
        "strict validation must fail the query like eval"
    );
}

#[test]
fn inner_format_text_overrides_outer_json() {
    let f = fixture("graph_inner_format.active", "[s]\na = 1\nb = a\n");
    let out = active()
        .args(["graph"])
        .arg(&f)
        .args([
            "--format",
            "json",
            "query",
            "--query",
            "dependents-of",
            "--key",
            "s.a",
            "--format",
            "text",
        ])
        .output()
        .expect("run active");
    assert!(out.status.success());
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(!body.trim_start().starts_with('{'), "text expected: {body}");
    assert!(body.contains("s.b"), "{body}");
}

#[test]
fn viz_honors_allow_parent_escapes() {
    let dir = std::env::temp_dir().join(format!("active-viz-escape-{}", std::process::id()));
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).expect("dirs");
    std::fs::write(dir.join("inc.active"), "host = \"x\"\n").expect("include");
    let main = sub.join("main.active");
    std::fs::write(&main, "include \"../inc.active\"\n").expect("main");

    let denied = active()
        .args(["viz", "graph", "--from"])
        .arg(&main)
        .output()
        .expect("run active");
    assert!(
        !denied.status.success(),
        "escape must fail without the flag"
    );
    let allowed = active()
        .args(["viz", "graph", "--from"])
        .arg(&main)
        .arg("--allow-parent-escapes")
        .output()
        .expect("run active");
    assert!(
        allowed.status.success(),
        "{}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn viz_repair_honors_allow_parent_escapes() {
    let dir = std::env::temp_dir().join(format!("active-viz-repair-escape-{}", std::process::id()));
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).expect("dirs");
    std::fs::write(dir.join("inc.active"), "host = \"x\"\n").expect("include");
    let main = sub.join("main.active");
    std::fs::write(
        &main,
        "include \"../inc.active\"\nschema = { port = Int(default = 8080) }\n",
    )
    .expect("main");

    let denied = active()
        .args(["viz", "repair", "--from"])
        .arg(&main)
        .output()
        .expect("run active");
    assert!(denied.status.success(), "viz export still prints the plan");
    let denied_plan: serde_json::Value = serde_json::from_slice(&denied.stdout).expect("plan JSON");
    assert_eq!(denied_plan["clean"], false, "{denied_plan}");
    assert!(
        denied_plan["final_codes"]
            .as_array()
            .expect("codes")
            .iter()
            .any(|c| c.as_str().unwrap_or("").starts_with("E015")),
        "escape must surface as E015: {denied_plan}"
    );
    let allowed = active()
        .args(["viz", "repair", "--from"])
        .arg(&main)
        .arg("--allow-parent-escapes")
        .output()
        .expect("run active");
    assert!(
        allowed.status.success(),
        "{}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&allowed.stdout).expect("plan");
    assert_eq!(plan["clean"], true, "{plan}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn graph_query_threads_the_profile_override() {
    let f = fixture(
        "graph_profile.active",
        "profile \"prod\" { token = secret(\"hunter2-profile\") }\n",
    );
    let out = active()
        .args(["graph"])
        .arg(&f)
        .args([
            "--profile",
            "prod",
            "query",
            "--query",
            "secret-tainted-cone",
        ])
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(
        body.contains("token"),
        "profile value not evaluated: {body}"
    );
    assert!(
        !body.contains("hunter2"),
        "raw secret in query output: {body}"
    );
}

#[test]
fn repair_browser_keeps_per_file_diagnostics_distinct() {
    let dir = std::env::temp_dir().join(format!("active-browser-dedup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dirs");
    std::fs::write(dir.join("a.active"), "x = = 1\n").expect("a");
    std::fs::write(dir.join("b.active"), "x = = 1\n").expect("b");
    let main = dir.join("main.active");
    std::fs::write(&main, "include \"a.active\"\ninclude \"b.active\"\n").expect("main");

    let out = active()
        .args(["repair", "--from"])
        .arg(&main)
        .arg("--browser")
        .output()
        .expect("run active");
    assert!(out.status.success());
    let model: serde_json::Value = serde_json::from_slice(&out.stdout).expect("model");
    let errors = model["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter(|d| d["code"] == "E002")
        .count();
    assert_eq!(errors, 2, "each included file keeps its own E002: {model}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn debug_minimize_resolves_includes_and_rejects_trust() {
    let dir = std::env::temp_dir().join(format!("active-debug-inc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dirs");
    std::fs::write(dir.join("inc.active"), "bad = = 1\n").expect("include");
    let main = dir.join("main.active");
    std::fs::write(&main, "include \"inc.active\"\nok = 1\n").expect("main");

    let out = active()
        .args(["debug", "--from"])
        .arg(&main)
        .args(["--minimize", "E002"])
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("minimize JSON");
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(reduced.contains("include \"inc.active\""), "{reduced}");
    assert!(
        !reduced.contains("ok = 1"),
        "unrelated line must be reduced away: {reduced}"
    );
    assert!(
        report["reduced_bytes"].as_u64().unwrap() < report["original_bytes"].as_u64().unwrap(),
        "reduction made no progress: {report}"
    );

    let missing = dir.join("missing.toml");
    let trust = active()
        .args(["debug", "--from"])
        .arg(&main)
        .args(["--minimize", "E002", "--trust-keys"])
        .arg(&missing)
        .output()
        .expect("run active");
    assert!(!trust.status.success(), "signed reduction is unsupported");
    assert!(String::from_utf8_lossy(&trust.stderr).contains("not supported"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn repair_browser_fatal_errors_keep_file_and_span() {
    let dir = std::env::temp_dir().join(format!("active-browser-fatal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dirs");
    std::fs::write(dir.join("inc.active"), "a = 1 + \"x\"\n").expect("include");
    let main = dir.join("main.active");
    std::fs::write(&main, "include \"inc.active\"\nport = 8080\n").expect("main");

    let out = active()
        .args(["repair", "--from"])
        .arg(&main)
        .arg("--browser")
        .output()
        .expect("run active");
    assert!(out.status.success());
    let model: serde_json::Value = serde_json::from_slice(&out.stdout).expect("model");
    let fatal = model["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|d| d["code"] == "T001")
        .expect("type error row");
    assert!(
        fatal["file"].as_str().unwrap_or("").ends_with("inc.active"),
        "include file lost: {fatal}"
    );
    assert!(!fatal["span"].is_null(), "span lost: {fatal}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn debug_minimize_reports_an_absent_target() {
    let f = fixture("debug_clean.active", "port = 8080\n");
    let out = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--minimize", "T999"])
        .output()
        .expect("run active");
    assert_eq!(
        out.status.code(),
        Some(1),
        "documented absent-target exit: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(report["target_present"], false);
}

#[test]
fn debug_rejects_inapplicable_flags() {
    let f = fixture("debug_flags.active", "port = 8080\n");
    let expect = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--expect", "sha256:deadbeef"])
        .output()
        .expect("run active");
    assert!(!expect.status.success(), "--expect requires --replay");

    let break_replay = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--break", "taint", "--replay"])
        .output()
        .expect("run active");
    assert!(
        !break_replay.status.success(),
        "break conflicts with replay"
    );

    let break_minimize = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--break", "taint", "--minimize", "E001"])
        .output()
        .expect("run active");
    assert_eq!(
        break_minimize.status.code(),
        Some(2),
        "clap must reject the combination: {}",
        String::from_utf8_lossy(&break_minimize.stderr)
    );
    assert!(String::from_utf8_lossy(&break_minimize.stderr).contains("cannot be used with"));

    let replay_minimize = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--replay", "--minimize", "E001"])
        .output()
        .expect("run active");
    assert_eq!(replay_minimize.status.code(), Some(2));

    // --max-attempts is a reduction knob; without --minimize it would
    // silently do nothing, so clap requires the flag.
    let attempts = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--max-attempts", "5"])
        .output()
        .expect("run active");
    assert_eq!(
        attempts.status.code(),
        Some(2),
        "clap must require --minimize: {}",
        String::from_utf8_lossy(&attempts.stderr)
    );
    assert!(String::from_utf8_lossy(&attempts.stderr).contains("--minimize"));
}

#[test]
fn graph_query_accepts_inner_profile_flag() {
    // F11: the documented grammar puts `--profile` after the query
    // name, not only on the outer `graph` command.
    let f = fixture(
        "gq_profile.active",
        "active_profile = \"a\"\nprofile \"a\" { n = \"benign\" }\nprofile \"b\" { n = env.ACTIVE_GQ_TOKEN }\nn = \"default\"\nk = secret(n)\n",
    );
    let query = |profile: &str| {
        active()
            .args(["graph", "query", "--from"])
            .arg(&f)
            .args(["--query", "secret-tainted-cone", "--profile", profile])
            .env("ACTIVE_GQ_TOKEN", "s3cr3t-gq")
            .output()
            .expect("run active")
    };
    let material = query("b");
    assert!(
        material.status.success(),
        "{}",
        String::from_utf8_lossy(&material.stderr)
    );
    let text = String::from_utf8_lossy(&material.stdout);
    assert!(
        text.lines().any(|line| line == "n"),
        "override b must taint the selected locator source: {text}"
    );
    assert!(!text.contains("s3cr3t-gq"), "secret leaked: {text}");
    let benign = query("a");
    assert!(benign.status.success());
    assert!(!String::from_utf8_lossy(&benign.stdout).contains("s3cr3t-gq"));
}

#[test]
fn inspect_accepts_profile_override() {
    // F12: `inspect --path` renders through the structural profile the
    // evaluation used, and the CLI exposes the selector.
    let f = fixture(
        "inspect_profile.active",
        "active_profile = \"a\"\nprofile \"a\" { n = \"benign\" }\nprofile \"b\" { n = env.ACTIVE_INSPECT_TOKEN }\nn = \"default\"\nk = secret(n)\n",
    );
    let run = |profile: &str| {
        active()
            .args(["inspect", "--from"])
            .arg(&f)
            .args(["--path", "n", "--profile", profile])
            .env("ACTIVE_INSPECT_TOKEN", "s3cr3t-inspect")
            .output()
            .expect("run active")
    };
    let material = run("b");
    assert!(
        material.status.success(),
        "{}",
        String::from_utf8_lossy(&material.stderr)
    );
    let text = String::from_utf8_lossy(&material.stdout);
    assert!(text.contains("***SECRET***"), "{text}");
    assert!(!text.contains("s3cr3t-inspect"), "secret leaked: {text}");
    // The selected profile is the locator source under every selector,
    // so `a` is material there too; the value it names must still be
    // redacted rather than published.
    let benign = run("a");
    assert!(benign.status.success());
    let text = String::from_utf8_lossy(&benign.stdout);
    assert!(!text.contains("benign"), "selected source leaked: {text}");
}

#[test]
fn viz_taint_accepts_profile_override() {
    // F12: viz taint shares the inspect model and honors the selector.
    let f = fixture(
        "viz_profile.active",
        "active_profile = \"a\"\nprofile \"a\" { n = \"benign\" }\nprofile \"b\" { n = env.ACTIVE_VIZ_TOKEN }\nn = \"default\"\nk = secret(n)\n",
    );
    let out = active()
        .args(["viz", "taint", "--from"])
        .arg(&f)
        .args(["--profile", "b"])
        .env("ACTIVE_VIZ_TOKEN", "s3cr3t-viz")
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("\"id\": \"n\""),
        "override b must mark the selected locator source: {text}"
    );
    assert!(!text.contains("s3cr3t-viz"), "secret leaked: {text}");
}

#[test]
fn budget_explain_profile_hides_material_length() {
    // F13: the dashboard is a redacted-size view; a profile override
    // must not leak the length of the selected material.
    let f = fixture(
        "budget_profile.active",
        "active_profile = \"a\"\nprofile \"a\" { n = \"benign\" }\nprofile \"b\" { n = env.ACTIVE_BUDGET_MATERIAL }\nn = \"default\"\nk = secret(n)\n",
    );
    let size_of = |material: &str| {
        let out = active()
            .args(["budget", "explain", "--from"])
            .arg(&f)
            .args(["--profile", "b", "--format", "json"])
            .env("ACTIVE_BUDGET_MATERIAL", material)
            .output()
            .expect("run active");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let body: serde_json::Value = serde_json::from_slice(&out.stdout).expect("budget JSON");
        body["usage"]["value_size"].as_u64().expect("value_size")
    };
    assert_eq!(
        size_of("abcde"),
        size_of("0123456789012345678901234567890123456789"),
        "material length must not change the reported size"
    );
}

#[test]
fn viz_profile_is_rejected_for_kinds_without_values() {
    let f = fixture("viz_profile_reject.active", "port = 1\n");
    let out = active()
        .args(["viz", "schema", "--from"])
        .arg(&f)
        .args(["--profile", "b"])
        .output()
        .expect("run active");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--profile only applies"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn audit_source_path_with_interpolation_markers_revalidates() {
    // F11 (round 22): a source path containing `${…}` must not
    // interpolate inside the audit document. `\$` keeps it literal, so
    // the archived audit passes `active check`.
    let f = fixture("${x}.active", "port = 1\n");
    let audit = std::env::temp_dir().join(format!("audit-dollar-{}.active", std::process::id()));
    let out = active()
        .args(["eval", "--audit-trail"])
        .arg(&audit)
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = std::fs::read_to_string(&audit).expect("audit file");
    assert!(body.contains("\\${x}"), "dollar not escaped: {body}");
    let checked = active()
        .arg("check")
        .arg(&audit)
        .output()
        .expect("run active");
    assert!(
        checked.status.success(),
        "audit must re-validate: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
}

#[test]
fn materialise_blobs_fails_closed_on_a_path_material_blob() {
    // N3 (review 32): a profile places a blob at an inherited material
    // path. The demand must fail with B003, not silently drop payload.
    let f = fixture(
        "materialise_path_taint.active",
        "let s = env.ACTIVE_MAT_R32\nk = secret(s)\nn = s\nb = blob(\"text/plain\", \"public\")\nactive_profile = \"p\"\nprofile \"p\" { n = b }\n",
    );
    let out = active()
        .args(["eval", "--materialise-blobs", "--profile", "p"])
        .arg(&f)
        .env("ACTIVE_MAT_R32", "mat-secret-r32")
        .output()
        .expect("run active");
    assert!(!out.status.success(), "path-material demand must fail");
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("B003"), "{text}");
}

#[test]
fn eval_redacts_runtime_secrets_copied_outside_the_export_tree() {
    // Review 46: a let-bound runtime secret is not part of the exported
    // tree, so the export must seed the redactor from material env
    // values. Both the plain copy and the materialised payload drop.
    let f = fixture(
        "runtime_secret_let.active",
        "let s = env.ACTIVE_R46_A\nlet k = secret(s)\nlet b = env.ACTIVE_R46_B\nf = blob(\"text/plain\", b)\ncopy = b\n",
    );
    let out = active()
        .arg("eval")
        .arg(&f)
        .env("ACTIVE_R46_A", "r46-runtime-token")
        .env("ACTIVE_R46_B", "r46-runtime-token")
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("r46-runtime-token"), "{text}");
    assert!(text.contains("***SECRET***"), "{text}");
}

#[test]
fn eval_redacts_a_profile_selected_env_copy() {
    // Review 46: a profile-selected plain env read is material only by
    // path; a copy of its value elsewhere must still redact.
    let f = fixture(
        "runtime_secret_profile.active",
        "let e = env.ACTIVE_R46_LIVE\nlet p = env.ACTIVE_R46_SRC\nk = secret(p)\nn = p\nactive_profile = \"pr\"\nprofile \"pr\" { n = e }\nlet d = env.ACTIVE_R46_DUP\ncopy = d\nf = blob(\"text/plain\", d)\n",
    );
    let out = active()
        .arg("eval")
        .arg(&f)
        .env("ACTIVE_R46_LIVE", "r46-profile-token")
        .env("ACTIVE_R46_SRC", "r46-source-token")
        .env("ACTIVE_R46_DUP", "r46-profile-token")
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("r46-profile-token"), "{text}");
    assert!(text.contains("***SECRET***"), "{text}");
}

#[test]
fn audit_hash_replays_after_runtime_secret_redaction() {
    // Review 47: the audit and replay must hash the same redacted tree
    // even when the secret is bound to a `let` name and never exported.
    let f = fixture(
        "audit_replay_runtime.active",
        "let s = env.ACTIVE_R47_A\nlet k = secret(s)\ncopy = s\n",
    );
    let audit = f.with_extension("audit.active");
    let out = active()
        .arg("eval")
        .arg(&f)
        .arg("--audit-trail")
        .arg(&audit)
        .env("ACTIVE_R47_A", "r47-audit-token")
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let audit_text = std::fs::read_to_string(&audit).expect("audit file");
    let hash = audit_text
        .lines()
        .find_map(|line| line.trim().strip_prefix("result = "))
        .map(|value| value.trim_end_matches(',').trim_matches('"').to_string())
        .expect("audit result hash");
    assert!(!hash.is_empty(), "{audit_text}");
    let replay = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--replay", "--expect"])
        .arg(&hash)
        .env("ACTIVE_R47_A", "r47-audit-token")
        .output()
        .expect("run active");
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let text = String::from_utf8_lossy(&replay.stdout);
    assert!(text.contains("\"ok\": true"), "{text}");
    assert!(text.contains("\"expected_match\": true"), "{text}");
}

#[test]
fn audit_hash_replays_with_a_materialised_payload_secret() {
    // Review 48: the audit hashes a canonical materialised tree and
    // replay uses the same policy, so a payload-resident secret cannot
    // make the expected hash unreachable.
    let f = fixture(
        "audit_replay_payload.active",
        "k = secret(\"PIN-r48-payload\")\nf = blob(\"text/plain\", \"PIN-r48-payload\")\nkeep = 1\n",
    );
    let audit = f.with_extension("audit.active");
    let out = active()
        .args(["eval", "--materialise-blobs", "--path", "keep"])
        .arg(&f)
        .arg("--audit-trail")
        .arg(&audit)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let audit_text = std::fs::read_to_string(&audit).expect("audit file");
    let hash = audit_text
        .lines()
        .find_map(|line| line.trim().strip_prefix("result = "))
        .map(|value| value.trim_end_matches(',').trim_matches('"').to_string())
        .expect("audit result hash");
    let replay = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--replay", "--expect"])
        .arg(&hash)
        .output()
        .expect("run active");
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let text = String::from_utf8_lossy(&replay.stdout);
    assert!(text.contains("\"ok\": true"), "{text}");
}

#[test]
fn budget_size_describes_the_redacted_export() {
    // Review 48: a copy of a runtime secret must report marker length,
    // not the secret's length.
    let short = fixture(
        "budget_short.active",
        "let v = \"PIN-${12}\"\nlet k = secret(v)\ncopy = \"PIN-12\"\n",
    );
    let long = fixture(
        "budget_long.active",
        "let v = \"PIN-${123456789012}\"\nlet k = secret(v)\ncopy = \"PIN-123456789012\"\n",
    );
    let usage = |path: &std::path::Path| -> u64 {
        let out = active()
            .args(["budget", "explain", "--from"])
            .arg(path)
            .args(["--format", "json"])
            .output()
            .expect("run active");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("budget json");
        report["usage"]["value_size"].as_u64().expect("value_size")
    };
    assert_eq!(
        usage(&short),
        usage(&long),
        "secret length must not change the reported export size"
    );
}

#[test]
fn graph_secret_cone_includes_a_runtime_copy() {
    // Review 48: the query follows inspect/hover, which now include
    // evaluator seeds.
    let f = fixture(
        "graph_secret_cone.active",
        "let v = \"PIN-${1234}\"\nlet k = secret(v)\ncopy = \"PIN-1234\"\n",
    );
    let out = active()
        .args(["graph", "query", "--from"])
        .arg(&f)
        .args(["--query", "secret-tainted-cone", "--format", "json"])
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("copy"), "{text}");
}

#[test]
fn lock_withholds_an_env_value_matching_a_runtime_secret() {
    // Review 49: provenance withholds a value whose text matches an
    // evaluator seed, even when its name is not structurally material.
    let f = fixture(
        "lock_runtime_env.active",
        "let v = \"PIN-${12 + 34}\"\nlet k = secret(v)\nout = env.ACTIVE_R49_PROV\n",
    );
    let out = active()
        .arg("lock")
        .arg(&f)
        .env("ACTIVE_R49_PROV", "PIN-46")
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("PIN-46"), "{text}");
}

#[test]
fn audit_trail_succeeds_with_blob_aliases_near_the_budget() {
    // Review 49: the canonical audit pass uses a taint probe, so
    // per-alias payload accounting cannot fail a document that
    // exported successfully.
    let payload = "A".repeat(100);
    let src = format!(
        "d = \"{payload}\"\nb = blob(\"text/plain\", d)\nr1 = b\nr2 = b\nr3 = b\nr4 = b\nr5 = b\n"
    );
    let f = fixture("audit_blob_aliases.active", &src);
    let audit = f.with_extension("audit.active");
    let out = active()
        .arg("eval")
        .arg(&f)
        .arg("--max-total-value-size")
        .arg("2000")
        .arg("--audit-trail")
        .arg(&audit)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(audit.exists(), "audit file must be written");
}

#[test]
fn audit_and_lock_agree_on_a_payload_resident_secret() {
    // Review 49: both hashes come from the same canonicaliser.
    let f = fixture(
        "audit_lock_payload.active",
        "k = secret(\"PIN-r49-lock\")\nf = blob(\"text/plain\", \"PIN-r49-lock\")\nkeep = 1\n",
    );
    let audit = f.with_extension("audit.active");
    let out = active()
        .arg("eval")
        .arg(&f)
        .arg("--path")
        .arg("keep")
        .arg("--audit-trail")
        .arg(&audit)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let audit_text = std::fs::read_to_string(&audit).expect("audit file");
    let audit_hash = audit_text
        .lines()
        .find_map(|line| line.trim().strip_prefix("result = "))
        .map(|value| value.trim_end_matches(',').trim_matches('"').to_string())
        .expect("audit result hash");
    let lock = active().arg("lock").arg(&f).output().expect("run active");
    assert!(
        lock.status.success(),
        "{}",
        String::from_utf8_lossy(&lock.stderr)
    );
    let lock_text = String::from_utf8_lossy(&lock.stdout);
    assert!(lock_text.contains(&audit_hash), "{lock_text}");
}

#[test]
fn replay_reproduces_an_alias_heavy_audit_hash() {
    // Review 50: replay uses the same probe policy and the caller's
    // limits, so an audit written near a caller budget still replays.
    let payload = "A".repeat(150);
    let src = format!(
        "d = \"{payload}\"\nb = blob(\"text/plain\", d)\nr1 = b\nr2 = b\nr3 = b\nr4 = b\nr5 = b\n"
    );
    let f = fixture("audit_replay_aliases.active", &src);
    let audit = f.with_extension("audit.active");
    let out = active()
        .arg("eval")
        .arg(&f)
        .arg("--max-total-value-size")
        .arg("5000")
        .arg("--audit-trail")
        .arg(&audit)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let audit_text = std::fs::read_to_string(&audit).expect("audit file");
    let hash = audit_text
        .lines()
        .find_map(|line| line.trim().strip_prefix("result = "))
        .map(|value| value.trim_end_matches(',').trim_matches('"').to_string())
        .expect("audit result hash");
    let replay = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--replay", "--expect"])
        .arg(&hash)
        .args(["--max-total-value-size", "5000"])
        .output()
        .expect("run active");
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    // The budget is an input: replaying with a limit that rejects the
    // document must fail instead of silently using a different one.
    let tighter = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--replay", "--expect"])
        .arg(&hash)
        .args(["--max-nodes", "1"])
        .output()
        .expect("run active");
    assert!(
        !tighter.status.success(),
        "replay must honor the caller's limits"
    );
    let tighter_text = String::from_utf8_lossy(&tighter.stderr);
    assert!(tighter_text.contains("budget exceeded"), "{tighter_text}");
}

#[test]
fn debug_steps_honor_a_caller_budget() {
    // Review 52: the new --max-* flags bind stepping too, never a
    // silently accepted no-op.
    let f = fixture("debug_budget.active", "a = 1\nb = 2\n");
    let out = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--max-nodes", "1"])
        .output()
        .expect("run active");
    assert!(!out.status.success(), "node budget must bind stepping");
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("budget exceeded"), "{text}");
}

#[test]
fn audit_records_limits_and_revalidates() {
    // Review 53: the audit is self-describing, and a limit above i64
    // is refused instead of writing an invalid document.
    let f = fixture("audit_limits.active", "a = 1\nb = 2\n");
    let audit = f.with_extension("audit.active");
    let out = active()
        .arg("eval")
        .arg(&f)
        .args(["--max-nodes", "500", "--max-wall-ms", "50"])
        .arg("--audit-trail")
        .arg(&audit)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&audit).expect("audit file");
    assert!(text.contains("budget = {"), "{text}");
    assert!(text.contains("parse = {"), "{text}");
    assert!(text.contains("nodes = 500"), "{text}");
    let checked = active()
        .arg("check")
        .arg(&audit)
        .output()
        .expect("run active");
    assert!(
        checked.status.success(),
        "audit must re-validate: {}",
        String::from_utf8_lossy(&checked.stderr)
    );

    let huge = active()
        .arg("eval")
        .arg(&f)
        .args(["--max-nodes", "18446744073709551615"])
        .arg("--audit-trail")
        .arg(f.with_extension("huge.active"))
        .output()
        .expect("run active");
    assert!(!huge.status.success(), "i64 overflow must be refused");
    let error = String::from_utf8_lossy(&huge.stderr);
    assert!(error.contains("i64"), "{error}");
}

#[test]
fn minimize_honors_strictness_and_budget() {
    // Review 53: the reducer evaluates exactly like the run it explains.
    let strict_doc = fixture(
        "minimize_strict.active",
        "schema = { with_default = Int(default = 7) }\nother = 1\n",
    );
    let strict = active()
        .args(["debug", "--from"])
        .arg(&strict_doc)
        .args(["--minimize", "W005"])
        .output()
        .expect("run active");
    assert!(
        !strict.status.success(),
        "strict mode must not report a resilient-only target"
    );
    let resilient = active()
        .args(["debug", "--from"])
        .arg(&strict_doc)
        .args(["--minimize", "W005", "--resilient"])
        .output()
        .expect("run active");
    assert!(
        resilient.status.success(),
        "{}",
        String::from_utf8_lossy(&resilient.stderr)
    );

    let budget_doc = fixture("minimize_budget.active", "a = 1\nb = 2\n");
    let default = active()
        .args(["debug", "--from"])
        .arg(&budget_doc)
        .args(["--minimize", "X001"])
        .output()
        .expect("run active");
    assert!(!default.status.success(), "X001 is absent by default");
    let bounded = active()
        .args(["debug", "--from"])
        .arg(&budget_doc)
        .args(["--minimize", "X001", "--max-nodes", "1"])
        .output()
        .expect("run active");
    assert!(
        bounded.status.success(),
        "{}",
        String::from_utf8_lossy(&bounded.stderr)
    );
}

#[test]
fn audit_accepts_the_i64_maximum() {
    // Review 54: the boundary itself is representable and revalidates.
    let f = fixture("audit_i64_max.active", "a = 1\nb = 2\n");
    let audit = f.with_extension("i64max.active");
    let out = active()
        .arg("eval")
        .arg(&f)
        .args(["--max-nodes", "9223372036854775807"])
        .arg("--audit-trail")
        .arg(&audit)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let checked = active()
        .arg("check")
        .arg(&audit)
        .output()
        .expect("run active");
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
}

#[test]
fn minimize_redacts_duplicate_secret_literals() {
    // Review 54: a duplicate of a secret value outside the locator (a
    // blob payload here) must not survive in the reduction bundle.
    let raw = "PIN-r54-duplicate";
    let f = fixture(
        "minimize_duplicate.active",
        &format!(
            "schema = {{ f = Int }}\nf = blob(\"text/plain\", \"{raw}\")\ns = secret(\"{raw}\")\n"
        ),
    );
    let out = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--minimize", "T002", "--format", "json"])
        .output()
        .expect("run active");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains(raw), "{text}");
    assert!(text.contains("***SECRET***"), "{text}");
}

#[test]
fn minimize_parses_resiliently_when_asked() {
    // Review 54: --resilient must reach the parser, not only the
    // evaluator.
    let f = fixture("minimize_h004.active", "a = [1, 2}\n");
    let strict = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--minimize", "H004"])
        .output()
        .expect("run active");
    assert!(!strict.status.success(), "strict parse cannot heal H004");
    let resilient = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--minimize", "H004", "--resilient"])
        .output()
        .expect("run active");
    assert!(
        resilient.status.success(),
        "{}",
        String::from_utf8_lossy(&resilient.stderr)
    );
}

#[test]
fn minimize_redacts_an_env_value_duplicate() {
    // Review 55: the bundle redactor is seeded from the evaluator, so a
    // plain value equal to an env-backed secret is redacted.
    let f = fixture(
        "minimize_env_dup.active",
        "schema = { x = Int }\nx = \"hunter2-env-dup\"\ntoken = secret.env(\"PROBE_SECRET\")\n",
    );
    let out = active()
        .args(["debug", "--from"])
        .arg(&f)
        .args(["--minimize", "T002", "--resilient", "--format", "json"])
        .env("PROBE_SECRET", "hunter2-env-dup")
        .output()
        .expect("run active");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("hunter2-env-dup"), "{text}");
    assert!(text.contains("***SECRET***"), "{text}");
}
