//! Linter tests (C-042, spec/linter.md).
//!
//! Drives the real shipped path (`parse` → `lint_with`) for all five
//! rules: positives, negatives, spans, determinism, independent
//! disablement, and the `depends_on`/`deprecated` surface (E020/E022,
//! extraction, formatter round-trips). Messages are asserted
//! structurally; L004 additionally asserts the suspected value
//! appears nowhere.

use active_core::lint::{lint, lint_with, LintOptions, ALL_RULES, RULE_CODES};
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
            "deprecated",
            "schema-evolution",
            "unknown-capability",
            "key-style",
            "large-literal",
            "data-mode",
            "requires-discipline",
            "agent-annotations",
            "ambiguous-repair",
            "canonical-order",
            "mixed-syntax",
            "include-hygiene"
        ]
    );
    // The code table stays in the same order as the rule list, so a
    // report code always maps back to exactly one rule name.
    assert_eq!(
        RULE_CODES.iter().map(|(rule, _)| *rule).collect::<Vec<_>>(),
        ALL_RULES
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
            ..Default::default()
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
        ..Default::default()
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
    // The dual-syntax acceptance fixture deliberately declares one
    // brace table and one indentation table in the same document to
    // prove both styles parse together (spec/dual-syntax.md). L015
    // exists to flag exactly that mix, so this file is the one
    // intentional exemption from the lint-clean invariant.
    let exempt = ["22_brace_blocks.active"];
    for path in files {
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| exempt.contains(&name))
        {
            continue;
        }
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

// ------------------------------------------------------------------
// Tooling-phase rules (LINT.003-LINT.007): schema evolution, agent
// capabilities, key style, large literals, and mode discipline.
// ------------------------------------------------------------------

#[test]
fn schema_evolution_fires_only_on_partial_metadata() {
    let partial = r#"schema = { min_reader = "1.0.0", port = Int }"#;
    assert!(lint_codes(partial).contains(&"L006".to_string()));
    // A fully unversioned schema is allowed: many contracts never
    // negotiate, and the rule targets inconsistency, not absence.
    let plain = "schema = { port = Int }";
    assert!(!lint_codes(plain).contains(&"L006".to_string()));
    let versioned = r#"schema = { version = "1.2.0", min_reader = "1.0.0", port = Int }"#;
    assert!(!lint_codes(versioned).contains(&"L006".to_string()));
}

#[test]
fn unknown_capability_fires_for_unadvertised_names() {
    let bad = "active_capabilities = [\"check\", \"teleport\"]\nport = 1";
    assert!(lint_codes(bad).contains(&"L007".to_string()));
    let good = "active_capabilities = [\"check\", \"eval\"]\nport = 1";
    assert!(!lint_codes(good).contains(&"L007".to_string()));
}

#[test]
fn key_style_fires_only_when_one_table_mixes_styles() {
    let mixed = "[server]\nreadTimeout = 1\nmax_retries = 2\n";
    assert!(lint_codes(mixed).contains(&"L008".to_string()));
    let snake = "[server]\nread_timeout = 1\nmax_retries = 2\n";
    assert!(!lint_codes(snake).contains(&"L008".to_string()));
    // Two tables using different styles are each internally
    // consistent; the rule is per table, not per document.
    let split = "[a]\nxY = 1\n[b]\np_q = 2\n";
    assert!(!lint_codes(split).contains(&"L008".to_string()));
}

#[test]
fn large_literal_suggests_content_addressing() {
    let big = "x".repeat(70_000);
    let src = format!("payload = \"{big}\"\n");
    assert!(lint_codes(&src).contains(&"L009".to_string()));
    assert!(!lint_codes("payload = \"small\"\n").contains(&"L009".to_string()));
}

#[test]
fn data_mode_hint_fires_for_data_constructs_in_config_mode() {
    let blob = "id = blob(\"text/plain\", \"hi\")\n";
    assert!(lint_codes(blob).contains(&"L010".to_string()));
    let marked = "active_mode = \"data\"\nid = blob(\"text/plain\", \"hi\")\n";
    assert!(!lint_codes(marked).contains(&"L010".to_string()));
    // Capability declarations are Data-oriented too.
    let caps = "active_capabilities = [\"check\"]\nport = 1\n";
    assert!(lint_codes(caps).contains(&"L010".to_string()));
}

#[test]
fn new_rules_disable_independently() {
    let src = "active_capabilities = [\"teleport\"]\nid = blob(\"text/plain\", \"hi\")\n";
    let all = lint_codes(src);
    assert!(all.contains(&"L007".to_string()));
    assert!(all.contains(&"L010".to_string()));
    let options = LintOptions {
        disabled: ["unknown-capability", "data-mode"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        ..Default::default()
    };
    let codes: Vec<String> = lint_with(&parse_ok(src), &options)
        .iter()
        .map(|d| d.code.clone())
        .collect();
    assert!(!codes.contains(&"L007".to_string()), "{codes:?}");
    assert!(!codes.contains(&"L010".to_string()), "{codes:?}");
}

#[test]
fn machine_readable_reports_have_stable_shapes() {
    let module = parse_ok("[server]\nreadTimeout = 1\nmax_retries = 2\n");
    let diags = lint(&module);
    assert!(!diags.is_empty(), "fixture must lint");
    let items: Vec<(String, active_core::Diagnostic)> = diags
        .iter()
        .cloned()
        .map(|d| ("svc.active".to_string(), d))
        .collect();

    let json = active_core::lint::json_report(&items);
    let rows = json.as_array().expect("array");
    assert_eq!(rows.len(), diags.len());
    assert_eq!(rows[0]["file"], "svc.active");
    assert_eq!(rows[0]["code"], diags[0].code);
    assert!(rows[0]["severity"].is_string());

    let sarif = active_core::lint::sarif_report(&items, "1.4.0");
    assert_eq!(sarif["version"], "2.1.0");
    assert_eq!(sarif["runs"][0]["tool"]["driver"]["name"], "active");
    let rules = sarif["runs"][0]["tool"]["driver"]["rules"]
        .as_array()
        .expect("rules");
    assert!(!rules.is_empty());
    let results = sarif["runs"][0]["results"].as_array().expect("results");
    assert_eq!(results.len(), diags.len());
    assert_eq!(results[0]["ruleId"], diags[0].code);
    assert_eq!(
        results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "svc.active"
    );
}

#[test]
fn known_capabilities_match_the_descriptor_exactly() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../agent-kit/capability.schema.json");
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("descriptor readable"))
            .expect("descriptor parses");
    let mut advertised: Vec<String> = doc["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().expect("name").to_string())
        .collect();
    advertised.sort();
    let mut known: Vec<String> = active_core::lint::KNOWN_CAPABILITIES
        .iter()
        .map(|s| s.to_string())
        .collect();
    known.sort();
    assert_eq!(known, advertised, "L007 list drifted from the descriptor");
}

#[test]
fn l007_accepts_the_advertised_lint_capability() {
    let src = "active_capabilities = [\"check\", \"lint\"]\nport = 1\n";
    assert!(!lint_codes(src).contains(&"L007".to_string()));
}

#[test]
fn sarif_uris_are_forward_slash_and_percent_encoded() {
    let module = parse_ok("[server]\nreadTimeout = 1\nmax_retries = 2\n");
    let diags = lint(&module);
    let items: Vec<(String, active_core::Diagnostic)> = diags
        .into_iter()
        .map(|d| ("C:\\tmp\\a b.active".to_string(), d))
        .collect();
    let sarif = active_core::lint::sarif_report(&items, "1.4.0");
    let uri = sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]
        ["artifactLocation"]["uri"]
        .as_str()
        .expect("uri");
    assert_eq!(uri, "C:/tmp/a%20b.active");
    assert!(!uri.contains('\\'));
}

#[test]
fn unicode_literals_do_not_panic_the_url_check() {
    // The L004 URL heuristic must never slice a multi-byte character.
    let diags = lint(&parse_ok("token = \"éé\"\n"));
    assert!(diags.iter().all(|d| d.code == "L004"), "{diags:?}");
    let _ = lint(&parse_ok("token = \"éé://x\"\n"));
}

// ------------------------------------------------------------------
// Tooling-phase rules L011-L016 and project configuration.
// ------------------------------------------------------------------

#[test]
fn requires_discipline_flags_dangling_self_and_empty_targets() {
    let dangling = r#"schema = { tls = Bool, cert = String(requires = "tlsz") }"#;
    let codes = lint_codes(dangling);
    assert!(codes.contains(&"L011".to_string()), "{codes:?}");
    let selfref = r#"schema = { a = String(requires = "a") }"#;
    assert!(lint_codes(selfref).contains(&"L011".to_string()));
    let empty = r#"schema = { tls = Bool, cert = String(requires = "  ") }"#;
    assert!(lint_codes(empty).contains(&"L011".to_string()));
    // A real schema path is the supported form and stays silent.
    let good = r#"schema = { tls = Bool, cert = String(requires = "tls") }"#;
    assert!(!lint_codes(good).contains(&"L011".to_string()));
    // Messages name the field, never the target text.
    let messages = lint_messages(dangling);
    assert!(messages.iter().any(|m| m.contains("cert")), "{messages:?}");
    assert!(messages.iter().all(|m| !m.contains("tlsz")), "{messages:?}");
}

#[test]
fn agent_annotations_require_intent_alongside_constraints() {
    let bare = "# @constraint: value > 0\nport = 1\n";
    let diags = lint(&parse_ok(bare));
    assert!(diags.iter().any(|d| d.code == "L012"), "{diags:?}");
    assert!(diags[0].message.contains("port"), "{:?}", diags[0]);
    let documented = "# @intent: \"public port\"\n# @constraint: value > 0\nport = 1\n";
    assert!(!lint_codes(documented).contains(&"L012".to_string()));
}

#[test]
fn ambiguous_repair_flags_normalized_sibling_collisions() {
    let collide = "[db]\nuser_name = \"a\"\nusername = \"b\"\n";
    let codes = lint_codes(collide);
    assert!(codes.contains(&"L013".to_string()), "{codes:?}");
    let distinct = "[db]\nuser_name = \"a\"\nremote = \"b\"\n";
    assert!(!lint_codes(distinct).contains(&"L013".to_string()));
}

#[test]
fn canonical_order_flags_depends_on_after_keys() {
    let late = "[server]\nport = 1\ndepends_on = [\"db\"]\n[db]\nurl = \"x\"\n";
    let codes = lint_codes(late);
    assert!(codes.contains(&"L014".to_string()), "{codes:?}");
    let early = "[server]\ndepends_on = [\"db\"]\nport = 1\n[db]\nurl = \"x\"\n";
    assert!(!lint_codes(early).contains(&"L014".to_string()));
}

#[test]
fn mixed_syntax_flags_only_when_both_block_styles_appear() {
    let mixed = "a {\n  x = 1\n}\n\nb:\n  y = 2\n";
    assert!(lint_codes(mixed).contains(&"L015".to_string()));
    let braces = "a {\n  x = 1\n}\n";
    assert!(!lint_codes(braces).contains(&"L015".to_string()));
    let indent = "a:\n  x = 1\n";
    assert!(!lint_codes(indent).contains(&"L015".to_string()));
}

#[test]
fn include_hygiene_flags_escapes_and_non_active_targets() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint-hygiene");
    let root = fixtures.join("sub/root.active");
    let options = active_core::ParseOptions {
        allow_parent_escapes: true,
        ..Default::default()
    };
    let module = active_core::parse_file(&root, options).expect("fixture parses");
    let diags = lint(&module);
    let codes: Vec<&str> = diags.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(
        codes.iter().filter(|c| **c == "L016").count(),
        3,
        "{diags:?}"
    );
    assert!(codes.iter().all(|c| *c == "L016"), "{diags:?}");
    let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
    assert!(
        messages.iter().any(|m| m.contains("escapes its directory")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("does not end in .active")),
        "{messages:?}"
    );
}

#[test]
fn include_hygiene_flags_duplicates_and_absolute_paths() {
    // Absolute and duplicate declarations need the metadata layer:
    // the parser refuses an absolute path unless the caller opts in,
    // and a duplicate include re-merges the same keys.
    let mut module = active_core::Module::new("synthetic");
    for (path, alias, line) in [
        ("/etc/active.conf", None, 1u32),
        ("shared.active", Some("a"), 2),
        ("shared.active", Some("a"), 3),
    ] {
        module.includes.push(active_core::ir::IncludeStmt {
            path: path.to_string(),
            alias: alias.map(str::to_string),
            private: false,
            scope_path: Vec::new(),
            line,
        });
    }
    let diags = lint(&module);
    let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
    assert!(
        messages.iter().any(|m| m.contains("absolute")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("more than once")),
        "{messages:?}"
    );
}

#[test]
fn project_config_disables_rules_and_overrides_severity() {
    use active_core::lint::LintConfig;

    let config = LintConfig::parse("disable = [\"unused-lets\"]\n").expect("valid config");
    let options = config.options(&[]).expect("options");
    assert!(lint_with(&parse_ok("let idle = 1\nport = 2\n"), &options).is_empty());

    let config = LintConfig::parse("[severity]\nunused-lets = \"error\"\n").expect("valid config");
    let options = config.options(&[]).expect("options");
    let diags = lint_with(&parse_ok("let idle = 1\nport = 2\n"), &options);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].severity, active_core::Severity::Error);

    let config = LintConfig::parse("[severity]\nunused-lets = \"off\"\n").expect("valid config");
    let options = config.options(&[]).expect("options");
    assert!(lint_with(&parse_ok("let idle = 1\nport = 2\n"), &options).is_empty());

    // Typos fail loudly instead of silently doing nothing.
    assert!(LintConfig::parse("disable = [\"nope\"]\n").is_err());
    assert!(LintConfig::parse("unknown = 1\n").is_err());
    assert!(LintConfig::parse("[severity]\nunused-lets = \"loud\"\n").is_err());
}

#[test]
fn project_config_is_discovered_from_parent_directories() {
    use active_core::lint::LintConfig;

    let doc =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint-config/sub/doc.active");
    let (path, config) = LintConfig::discover(&doc)
        .expect("discovery is infallible here")
        .expect("config discovered");
    assert!(path.ends_with(".active-lint.toml"));
    assert_eq!(config.disabled, vec!["unused-lets".to_string()]);
    let options = config.options(&[]).expect("options");
    let module = active_core::parse_file(&doc, active_core::ParseOptions::default())
        .expect("fixture parses");
    assert!(lint_with(&module, &options).is_empty());
}

#[test]
fn evolution_rules_see_nested_schema_tables() {
    // Nested table metadata is parsed, formatted, and emitted by
    // inspect; the rules must not stop at the root table.
    let nested = r#"schema = { db = Table { min_reader = "1.0.0" } }"#;
    assert!(
        lint_codes(nested).contains(&"L006".to_string()),
        "{:?}",
        lint_codes(nested)
    );
    let nested_removed = r#"schema = { db = Table { old = Int(removed_in = "2.0.0") } }"#;
    assert!(
        lint_codes(nested_removed).contains(&"L006".to_string()),
        "{:?}",
        lint_codes(nested_removed)
    );
    let nested_version = "schema = { db = Table { version = \"1.0.0\" } }";
    assert!(!lint_codes(nested_version).contains(&"L006".to_string()));
    assert!(
        lint_codes(nested_version).contains(&"L010".to_string()),
        "nested evolution metadata in Config Mode is a data construct: {:?}",
        lint_codes(nested_version)
    );
}
