//! Standard schema library tests (C-062, spec/schema-library.md).
//!
//! Drives the real shipped path (`parse_file` → strict/resilient
//! `Engine::evaluate`) on committed fixtures under
//! `tests/schemas_lib/`, which pull the library in via `include`
//! (the distribution mechanism itself is under test). Each schema
//! gets a good document (strict passes, values asserted) and a bad
//! document (strict fails with the pinned codes); a final test
//! renders every schema through the C-048 docgen, pinning the
//! reference pages against drift.

use active_core::convert::{collect_items, doc_to_markdown};
use active_core::{parse_file, Engine, EvalError, ParseOptions};
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .to_path_buf()
}

fn fixture(name: &str) -> PathBuf {
    workspace_root()
        .join("tests")
        .join("schemas_lib")
        .join(name)
}

fn parse_options() -> ParseOptions {
    // Fixtures reach the canonical schemas/ via `../..` (in-repo, no
    // escape): the containment check is relaxed for the test tree
    // only. Consumers vendor schemas/ next to their documents and
    // include downward with no flag (spec/schema-library.md §1).
    ParseOptions {
        allow_parent_escapes: true,
        ..ParseOptions::default()
    }
}

fn strict_engine() -> Engine {
    let mut engine = Engine::new();
    engine.resilient = false;
    engine
}

fn strict_value(name: &str) -> serde_json::Value {
    let module = parse_file(&fixture(name), parse_options())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    match strict_engine().evaluate(&module) {
        Ok((value, diags)) => {
            assert!(diags.is_empty(), "{name}: {diags:?}");
            let json = active_core::convert::value_to_pretty_json(&value);
            serde_json::from_str(&json).expect("valid json")
        }
        Err(e) => panic!("{name}: strict must pass: {e:?}"),
    }
}

fn strict_issues(name: &str) -> Vec<(String, Option<String>)> {
    let module = parse_file(&fixture(name), parse_options())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    match strict_engine().evaluate(&module) {
        Ok(_) => panic!("{name}: strict must fail"),
        Err(EvalError::ValidationFailed { diagnostics }) => diagnostics
            .iter()
            .map(|d| (d.code.clone(), d.path.clone()))
            .collect(),
        Err(e) => panic!("{name}: unexpected error: {e:?}"),
    }
}

fn resilient_issues(name: &str) -> (serde_json::Value, Vec<(String, Option<String>)>) {
    let module = parse_file(&fixture(name), parse_options())
        .unwrap_or_else(|diags| panic!("{name}: parse failed: {diags:?}"));
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: resilient must succeed: {e:?}"));
    let json = active_core::convert::value_to_pretty_json(&value);
    (
        serde_json::from_str(&json).expect("valid json"),
        diags
            .iter()
            .map(|d| (d.code.clone(), d.path.clone()))
            .collect(),
    )
}

#[test]
fn http_server_schema() {
    let v = strict_value("good_http.active");
    assert_eq!(v["port"], 9000);
    assert_eq!(v["tls"], true);
    assert_eq!(v["host"], "example.com");
    assert_eq!(
        strict_issues("bad_http.active"),
        [(String::from("T003"), Some(String::from("port")))]
    );
    // Resilient collects the same violation and keeps the tree.
    let (v, diags) = resilient_issues("bad_http.active");
    assert_eq!(v["port"], 99999);
    assert_eq!(diags, [(String::from("T003"), Some(String::from("port")))]);
}

#[test]
fn postgres_schema() {
    let v = strict_value("good_pg.active");
    assert_eq!(v["host"], "db.internal");
    assert_eq!(v["pool"], 10);
    assert_eq!(v["password"], "***SECRET***");
    assert_eq!(
        strict_issues("bad_pg.active"),
        [(String::from("T004"), Some(String::from("password")))]
    );
    let (v, diags) = resilient_issues("bad_pg.active");
    assert!(v.get("password").is_none(), "{v}");
    assert_eq!(
        diags,
        [(String::from("T004"), Some(String::from("password")))]
    );
}

#[test]
fn observability_schema() {
    let v = strict_value("good_obs.active");
    assert_eq!(v["level"], "warn");
    assert_eq!(v["metrics"], true);
    assert_eq!(
        strict_issues("bad_obs.active"),
        [(String::from("T003"), Some(String::from("level")))]
    );
    let (v, diags) = resilient_issues("bad_obs.active");
    assert_eq!(v["level"], "verbose");
    assert_eq!(diags, [(String::from("T003"), Some(String::from("level")))]);
}

#[test]
fn feature_flags_schema() {
    let v = strict_value("good_flags.active");
    assert_eq!(v["flags"][0]["rollout"], 50);
    assert_eq!(
        strict_issues("bad_flags.active"),
        [(String::from("T003"), Some(String::from("flags[0].rollout")))]
    );
    let (v, diags) = resilient_issues("bad_flags.active");
    assert_eq!(v["flags"][0]["rollout"], 150);
    assert_eq!(
        diags,
        [(String::from("T003"), Some(String::from("flags[0].rollout")))]
    );
}

#[test]
fn library_holds_no_secret_literals() {
    // Spec rule: no `secret(...)` calls inside schemas/ — secrets
    // enter through operators' own documents. Scans the raw text
    // (not the evaluated tree): even an unused literal would teach
    // bad habits to copiers. (Example *tokens* stay a review-time
    // check: no pattern distinguishes them reliably.)
    let mut checked = 0;
    let mut entries: Vec<_> = std::fs::read_dir(workspace_root().join("schemas"))
        .expect("schemas dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "active"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty(), "schemas/ must not be empty");
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).expect("readable");
        assert!(
            !text.contains("secret("),
            "{name}: secret literal in library"
        );
        checked += 1;
    }
    assert!(checked >= 5, "expected the five library schemas");
}

#[test]
fn closed_table_typo_reports_t005() {
    // `prt` suggests `port`, but the target is taken: suggest-only,
    // never clobber. Both modes agree on the violation.
    assert_eq!(
        strict_issues("bad_closed.active"),
        [(String::from("T005"), Some(String::from("prt")))]
    );
    let (v, diags) = resilient_issues("bad_closed.active");
    assert_eq!(v["port"], 8080);
    assert_eq!(v["prt"], 1);
    assert_eq!(diags, [(String::from("T005"), Some(String::from("prt")))]);
}

#[test]
fn vendored_layout_needs_no_escape_flag() {
    // The documented consumer layout (schemas/ vendored next to
    // documents, included downward) parses under DEFAULT options:
    // no parent escapes involved. The vendored copy must stay
    // byte-identical to the canonical file (no drift).
    let canonical =
        std::fs::read_to_string(workspace_root().join("schemas").join("http-server.active"))
            .expect("readable");
    let vendored = std::fs::read_to_string(
        workspace_root().join("tests/schemas_lib/vendored/schemas/http-server.active"),
    )
    .expect("readable");
    assert_eq!(vendored, canonical);
    let module = parse_file(
        &workspace_root().join("tests/schemas_lib/vendored/app.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|diags| panic!("vendored parse failed: {diags:?}"));
    let (value, diags) = strict_engine()
        .evaluate(&module)
        .expect("vendored must pass strict");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(v["port"], 9000);
}

#[test]
fn bare_include_fills_defaults_resiliently() {
    // An include with no values: strict demands explicitness (T004
    // per defaulted field), resilient fills every default (W005)
    // with no other diagnostics.
    let diags = strict_issues("bare.active");
    assert!(!diags.is_empty(), "strict must complain");
    assert!(diags.iter().all(|(c, _)| c == "T004"), "{diags:?}");
    let (v, diags) = resilient_issues("bare.active");
    assert_eq!(v["port"], 8080);
    assert_eq!(v["host"], "localhost");
    assert!(diags.iter().all(|(c, _)| c == "W005"), "{diags:?}");
}

#[test]
fn ai_agent_schema() {
    let v = strict_value("good_ai.active");
    assert_eq!(v["model"], "m1");
    assert_eq!(v["retries"], 5);
    assert_eq!(
        strict_issues("bad_ai.active"),
        [(String::from("T003"), Some(String::from("temperature")))]
    );
    let (v, diags) = resilient_issues("bad_ai.active");
    assert_eq!(v["temperature"], 5.0);
    assert_eq!(
        diags,
        [(String::from("T003"), Some(String::from("temperature")))]
    );
}

#[test]
fn every_library_schema_renders_docs() {
    // Reference pages generate from schemas + values (the docgen
    // walks the evaluated tree against the schema): render each
    // schema over its good sample document and require exact
    // `### <path>` headings — substring matching would let renames
    // and removals slip through. The hand-written docs-site table is
    // pinned the same way, and every typed field must carry its
    // @intent description.
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "http-server",
            "good_http.active",
            &[
                "### host",
                "### port",
                "### tls",
                "### timeout",
                "### max_requests",
            ],
        ),
        (
            "postgres",
            "good_pg.active",
            &[
                "### host",
                "### port",
                "### database",
                "### user",
                "### password",
                "### pool",
            ],
        ),
        (
            "observability",
            "good_obs.active",
            &[
                "### level",
                "### metrics",
                "### sample_rate",
                "### endpoint",
            ],
        ),
        (
            "feature-flags",
            "good_flags.active",
            &[
                "### flags",
                "### flags[0].name",
                "### flags[0].enabled",
                "### flags[0].rollout",
            ],
        ),
        (
            "ai-agent",
            "good_ai.active",
            &[
                "### model",
                "### temperature",
                "### max_tokens",
                "### retries",
            ],
        ),
    ];
    let chapter =
        std::fs::read_to_string(workspace_root().join("docs-site/src/reference/schema-library.md"))
            .expect("site chapter readable");
    for (name, sample, headings) in cases {
        let module = parse_file(&fixture(sample), parse_options())
            .unwrap_or_else(|diags| panic!("{sample}: parse failed: {diags:?}"));
        let (value, diags) = Engine::new()
            .evaluate(&module)
            .unwrap_or_else(|e| panic!("{sample}: {e:?}"));
        assert!(diags.is_empty(), "{sample}: {diags:?}");
        let items = collect_items(&module, &value);
        for item in &items {
            // Every typed leaf renders a description (spec rule:
            // descriptive comments on every schema field).
            // Array-element fields are exempt: inline-table values
            // never attach annotations or comments reliably
            // (documented parser boundary — statement-level call
            // sites only), so their docs live on the array field
            // itself.
            if !item.is_table && !item.type_text.is_empty() && !item.path.contains('[') {
                assert!(
                    !item.description.is_empty(),
                    "{name}: {} has no @intent",
                    item.path
                );
            }
        }
        let md = doc_to_markdown(name, &items);
        for heading in *headings {
            assert!(
                md.lines().any(|line| line.trim() == *heading),
                "{name}: docs missing '{heading}':\n{md}"
            );
            // The docs-site table names every enforced field.
            let field = heading
                .trim_start_matches("### ")
                .rsplit('.')
                .next()
                .unwrap_or("")
                .split('[')
                .next()
                .unwrap_or("");
            assert!(
                chapter.contains(field),
                "{name}: site chapter missing '{field}'"
            );
        }
        assert!(
            !md.contains("pw-test-only"),
            "{name}: secret leaked into docs"
        );
    }
}
