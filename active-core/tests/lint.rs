//! Linter tests (C-042, spec/linter.md).
//!
//! Drives the real shipped path (`parse` → `lint_with`) for all five
//! rules: positives, negatives, spans, determinism, independent
//! disablement, and the `depends_on`/`deprecated` surface (E020/E022,
//! extraction, formatter round-trips). Messages are asserted
//! structurally; L004 additionally asserts the suspected value
//! appears nowhere.

use active_core::lint::{lint, lint_with, LintOptions, ALL_RULES};
use active_core::{parse, Engine};
use std::collections::HashSet;
use std::path::Path;

fn parse_ok(src: &str) -> active_core::Module {
    parse(src, "lint").unwrap_or_else(|d| panic!("parse failed: {d:?}"))
}

fn lint_codes(src: &str) -> Vec<String> {
    lint(&parse_ok(src))
        .iter()
        .map(|d| d.code.clone())
        .collect()
}

fn lint_messages(src: &str) -> Vec<String> {
    lint(&parse_ok(src))
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn rule_ids_are_stable_and_complete() {
    assert_eq!(
        ALL_RULES,
        &[
            "unused-lets",
            "depends-on",
            "complexity",
            "hardcoded-secrets",
            "deprecated"
        ]
    );
}

#[test]
fn unused_lets_fire_only_for_unreferenced_bindings() {
    let diags = lint(&parse_ok("let used = 1\nlet idle = 2\nport = used\n"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "L001");
    assert!(diags[0].message.contains("idle"), "{:?}", diags[0]);
    assert_eq!(diags[0].severity, active_core::Severity::Warning);
    assert!(diags[0].span.is_some(), "bindings carry spans");
}

#[test]
fn uses_through_patches_constraints_and_schemas_count() {
    // `limit` is used by a patch value, `cap` by a schema bound, `n`
    // by a constraint: none may report L001.
    let src = "let limit = 1\nlet cap = 5\nlet n = 3\n\
         # @constraint: value <= n\nport = 1\n\
         schema = { port = Int(max = cap) }\n\
         @patch(\"port\", limit)\n";
    assert!(
        lint(&parse_ok(src)).iter().all(|d| d.code != "L001"),
        "{src}"
    );
}

#[test]
fn depends_on_completeness_dangling_and_self_scope() {
    // Complete declaration: silent.
    let diags = lint(&parse_ok(
        "let max = 4\n[server]\ndepends_on = [\"max\"]\nport = max\n",
    ));
    assert!(diags.iter().all(|d| d.code != "L002"), "{diags:?}");
    // Missing entry.
    let diags = lint(&parse_ok(
        "[server]\ndepends_on = []\nport = max\nlet max = 4\n",
    ));
    // `max` resolves as a let binding: missing, not dangling.
    assert!(
        diags
            .iter()
            .any(|d| d.code == "L002" && d.message.contains("missing")),
        "{diags:?}"
    );
    // Dangling path.
    let diags = lint(&parse_ok(
        "[server]\ndepends_on = [\"nowhere\"]\nport = 1\n",
    ));
    assert!(
        diags
            .iter()
            .any(|d| d.code == "L002" && d.message.contains("dangling")),
        "{diags:?}"
    );
    // Self-scope entry.
    let diags = lint(&parse_ok(
        "[server]\ndepends_on = [\"server.port\"]\nport = 1\n",
    ));
    assert!(
        diags
            .iter()
            .any(|d| d.code == "L002" && d.message.contains("unnecessary")),
        "{diags:?}"
    );
}

#[test]
fn depends_on_never_exports_and_cycles_fail() {
    let module = parse_ok("[server]\ndepends_on = [\"db\"]\nport = 1\n[db]\nhost = \"h\"\n");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(
        !json.contains("depends_on"),
        "metadata must not export: {json}"
    );
    // Declared cycle surfaces as S001 like every cycle.
    let module = parse_ok("[a]\ndepends_on = [\"b\"]\nx = 1\n[b]\ndepends_on = [\"a\"]\ny = 2\n");
    let err = Engine::new()
        .evaluate(&module)
        .expect_err("cycle must fail");
    assert_eq!(err.code(), "S001");
}

#[test]
fn depends_on_malformed_forms_are_e022() {
    for src in [
        "[server]\ndepends_on = [1, 2]\nport = 1\n",
        "servers = [{ depends_on = [\"a\"] }]\n",
    ] {
        let diags = parse(src, "lint").expect_err("must fail");
        assert!(diags.iter().any(|d| d.code == "E022"), "{diags:?}");
    }
    // Non-array values stay ordinary data, even in profiles.
    let module = parse_ok("profile \"p\" {\n  depends_on = \"x\"\n}\n");
    assert!(module.depends_on.is_empty());
    // Array metadata inside profiles is E022 and dropped.
    let diags =
        parse("profile \"p\" {\n  depends_on = [\"a\"]\n}\n", "lint").expect_err("must fail");
    assert!(diags.iter().any(|d| d.code == "E022"), "{diags:?}");
}

#[test]
fn complexity_counts_operators_not_data() {
    // 150-term addition: 149 Binary nodes, must fire.
    let mut terms = String::from("x = 1");
    for _ in 0..149 {
        terms.push_str(" + 1");
    }
    terms.push('\n');
    let diags = lint(&parse_ok(&terms));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "L003");
    assert!(diags[0].message.contains("149"), "{:?}", diags[0]);
    // A 200-element data array holds no operators: silent.
    let mut arr = String::from("xs = [");
    for i in 0..200 {
        arr.push_str(&format!("{i},"));
    }
    arr.push_str("]\n");
    assert!(lint(&parse_ok(&arr)).is_empty());
}

#[test]
fn hardcoded_secrets_name_and_entropy_signals() {
    // Credential-named key with a literal password.
    let diags = lint(&parse_ok("[db]\npassword = \"s3cr3t-hunter2\"\n"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "L004");
    assert!(
        !diags[0].message.contains("s3cr3t"),
        "value must not leak: {:?}",
        diags[0]
    );
    // Placeholders stay quiet.
    for value in ["changeme", "test", "***SECRET***", ""] {
        let src = format!("[db]\npassword = \"{value}\"\n");
        assert!(lint(&parse_ok(&src)).is_empty(), "{src}");
    }
    // Proper secret plumbing stays quiet.
    assert!(lint(&parse_ok("[db]\npassword = secret(\"k\")\n")).is_empty());
    assert!(lint(&parse_ok("let pw = \"x\"\n[db]\npassword = pw\n")).is_empty());
    // High-entropy literal under an innocent name still fires — and
    // never quotes the value. (24 unique chars: log2(24) ≈ 4.58.)
    let token = "aB3xY9qW2eR7tU5iO1pL4zQ8v";
    assert!(token.len() >= 20);
    let src = format!("note = \"{token}\"\n");
    let diags = lint(&parse_ok(&src));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(!diags[0].message.contains(&token[..8]), "{:?}", diags[0]);
    // Prose stays quiet.
    assert!(lint(&parse_ok("note = \"Primary database cluster\"\n")).is_empty());
}

#[test]
fn hardcoded_secrets_cover_arrays() {
    // Name signal through array elements, path indexed to the owner.
    let diags = lint(&parse_ok("passwords = [\"s3cr3t-hunter2\"]\n"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "L004");
    assert_eq!(diags[0].path.as_deref(), Some("passwords[0]"));
    assert!(!diags[0].message.contains("s3cr3t"), "{:?}", diags[0]);
    // Entropy signal through nested arrays.
    let token = "zQ8vA1bC3dE5fG7hJ9kL2mN4";
    let src = format!("xs = [[\"{token}\"]]\n");
    let diags = lint(&parse_ok(&src));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].path.as_deref(), Some("xs[0][0]"));
}

#[test]
fn complexity_counts_env_defaults_and_secret_locators() {
    // A 101-term default hidden in env() must trip like inline math.
    let mut terms = String::from("x = env.FLAG ?: (1");
    for _ in 0..100 {
        terms.push_str(" + 1");
    }
    terms.push_str(")\n");
    let diags = lint(&parse_ok(&terms));
    assert!(diags.iter().any(|d| d.code == "L003"), "{diags:?}");
    // Same through a secret locator.
    let mut locator = String::from("x = secret((1");
    for _ in 0..100 {
        locator.push_str(" + 1");
    }
    locator.push_str("))\n");
    let diags = lint(&parse_ok(&locator));
    assert!(diags.iter().any(|d| d.code == "L003"), "{diags:?}");
}

#[test]
fn deprecated_false_is_e020_in_all_forms() {
    for src in [
        "schema = { x = Int(deprecated = false) }\n",
        "schema = { x = Int(deprecated = false), y = String }\n",
        "schema = { mode = Enum(\"a\", deprecated = false) }\n",
    ] {
        let diags = parse(src, "lint").expect_err("must fail");
        assert!(diags.iter().any(|d| d.code == "E020"), "{diags:?}");
    }
}

#[test]
fn entropy_boundary_is_deterministic() {
    // Near-threshold input must decide identically every run.
    let src = "note = \"aB3xY9qW2eR7tU5iO1pL4zQ8v\"\n";
    let run = || lint_messages(src);
    assert_eq!(run(), run());
}

#[test]
fn deprecated_fields_warn_with_migration_message() {
    let src =
        "schema = { old_port = Int(deprecated = \"use port\"), gone = Bool(deprecated = true) }\n\
         old_port = 1\ngone = true\nfresh = 2\n";
    let diags = lint(&parse_ok(src));
    assert_eq!(diags.len(), 2, "{diags:?}");
    assert!(diags.iter().all(|d| d.code == "L005"));
    assert!(diags[0].message.contains("use port"), "{:?}", diags[0]);
    assert!(!diags[1].message.contains(':'), "{:?}", diags[1]);
    // Absent deprecated keys stay silent.
    let src = "schema = { old_port = Int(deprecated = true) }\nport = 1\n";
    assert!(lint(&parse_ok(src)).is_empty());
    // Bad deprecated payloads are E020.
    for src in [
        "schema = { x = Int(deprecated = 42) }\n",
        "schema = { x = Int(deprecated = some_let) }\nlet some_let = 1\n",
    ] {
        let diags = parse(src, "lint").expect_err("must fail");
        assert!(diags.iter().any(|d| d.code == "E020"), "{diags:?}");
    }
    // Deprecated Enums round-trip through the formatter.
    let src = "schema = { mode = Enum(\"a\", \"b\", deprecated = \"gone\") }\nmode = \"a\"\n";
    let module = parse_ok(src);
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    assert!(out.contains("deprecated = \"gone\""), "{out}");
    let diags = lint(&parse(&out, "lint").expect("reparse"));
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, "L005");
}

#[test]
fn depends_on_blocks_round_trip_through_the_formatter() {
    let src = "[server]\ndepends_on = [\"db\"]\nport = 1\n[db]\nhost = \"h\"\n";
    for style in [active_core::Style::Brace, active_core::Style::Indent] {
        let out = active_core::fmt::format(&parse_ok(src), style);
        assert!(out.contains("depends_on = [\"db\"]"), "kept:\n{out}");
        let module = parse(&out, "lint").expect("reparse");
        assert!(module.depends_on.values().any(|d| d.paths == ["db"]));
    }
}

#[test]
fn rules_disable_independently() {
    let src = "let idle = 1\n[db]\npassword = \"s3cr3t-hunter2\"\n";
    assert_eq!(lint_codes(src).len(), 2);
    for rule in ["unused-lets", "hardcoded-secrets"] {
        let options = LintOptions {
            disabled: [rule.to_string()].into_iter().collect::<HashSet<_>>(),
        };
        let codes = lint_with(&parse_ok(src), &options)
            .iter()
            .map(|d| d.code.clone())
            .collect::<Vec<_>>();
        assert_eq!(codes.len(), 1, "{rule}: {codes:?}");
    }
    let options = LintOptions {
        disabled: ["unused-lets".to_string(), "hardcoded-secrets".to_string()]
            .into_iter()
            .collect::<HashSet<_>>(),
    };
    assert!(lint_with(&parse_ok(src), &options).is_empty());
    // Unknown rules are an error listing the valid ones.
    let err = LintOptions::with_disabled(&["nope".to_string()]).expect_err("must fail");
    assert!(err.contains("unused-lets"), "{err}");
}

#[test]
fn lint_output_is_deterministic() {
    let src = "let b = 1\nlet a = 2\n[db]\npassword = \"s3cr3t-hunter2\"\n";
    let run = || lint_messages(src);
    assert_eq!(run(), run());
}

#[test]
fn clean_corpus_and_examples_raise_nothing() {
    // The shipped corpus and both examples must be lint-clean at the
    // default complexity threshold: rules fire on real risks, not on
    // idiomatic documents.
    let mut files = Vec::new();
    let dirs = ["tests/positive", "examples"];
    for dir in dirs {
        let entries = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(dir))
            .unwrap_or_else(|e| panic!("{dir}: {e}"));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "active") {
                files.push(path);
            }
        }
    }
    assert!(!files.is_empty(), "corpus must exist");
    for path in files {
        let module = active_core::parse_file(&path, active_core::ParseOptions::default())
            .unwrap_or_else(|d| panic!("{}: {d:?}", path.display()));
        let diags = lint(&module);
        assert!(diags.is_empty(), "{}: {diags:?}", path.display());
    }
}

#[test]
fn chained_and_nested_uses_count_as_used() {
    // Guard for the unused_lets flat scan: uses through chains, nested
    // tables/arrays, and interpolation must mark bindings used.
    let src = "let a = 1\nlet b = a\nlet idle = 2\n[tbl]\ndeep = [{ v = b }]\nmsg = \"${a}\"\n";
    let diags = lint(&parse_ok(src));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "L001");
    assert!(diags[0].message.contains("idle"), "{:?}", diags[0]);
}
