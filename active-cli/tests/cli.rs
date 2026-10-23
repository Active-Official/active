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
    mapping.insert("convert", vec!["convert"]);
    mapping.insert("graph", vec!["graph", "impact", "why"]);
    mapping.insert("generate", vec!["generate"]);
    mapping.insert("schema-introspect", vec!["inspect"]);
    mapping.insert("explain", vec!["explain"]);
    mapping.insert("patch", vec!["patch"]);
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
