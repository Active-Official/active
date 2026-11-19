//! Root-schema `check` invariants (L-P19, spec/validation.md section 11).

use active_core::convert::value_to_pretty_json;
use active_core::{parse, Engine, Style};

const PASS: &str = r#"
schema {
  port = Int
  stage = String
  check {
    staging_replicas = stage != "staging" || replicas == 10
  }
}
port = 8080
stage = "staging"
replicas = 10
"#;

fn t009s(source: &str) -> Vec<String> {
    let module = parse(source, "checks").expect("parse");
    let (_, diagnostics) = Engine::new().evaluate(&module).expect("resilient");
    diagnostics
        .into_iter()
        .filter(|d| d.code == "T009")
        .map(|d| d.message)
        .collect()
}

#[test]
fn a_passing_check_evaluates_cleanly_in_strict_mode() {
    let module = parse(PASS, "checks").expect("parse");
    let (value, diagnostics) = Engine::strict().evaluate(&module).expect("strict");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(value_to_pretty_json(&value).contains("\"replicas\": 10"));
}

#[test]
fn a_failing_check_is_t009_in_strict_and_resilient() {
    let source = PASS.replace("replicas = 10", "replicas = 2");
    let module = parse(&source, "checks").expect("parse");
    let error = Engine::strict()
        .evaluate(&module)
        .expect_err("strict refuses");
    let codes: Vec<&str> = match &error {
        active_core::EvalError::ValidationFailed { diagnostics } => {
            diagnostics.iter().map(|d| d.code.as_str()).collect()
        }
        other => panic!("unexpected error: {other:?}"),
    };
    assert!(codes.contains(&"T009"), "{codes:?}");
    let messages = t009s(&source);
    assert!(messages.iter().any(|m| m.contains("staging_replicas")));
}

#[test]
fn checks_see_the_profile_merged_tree() {
    let source = r#"
schema {
  check {
    ok = "${stage}-x" == expected
  }
}
stage = "prod"
expected = "prod-x"
active_profile = "p"
profile "p" {
  stage = "dev"
}
"#;
    // The final value is "dev-x", so the stale "prod-x" expectation
    // must fail rather than pass on the pre-profile node.
    assert!(t009s(source).iter().any(|m| m.contains("'ok' failed")));
}

#[test]
fn a_removed_key_or_replaced_table_is_t009_not_stale_data() {
    let removed = r#"
schema { check { r = "${name}" == "gone" } }
name = "gone"
@remove("name")
"#;
    assert!(t009s(removed)
        .iter()
        .any(|m| m.contains("could not be evaluated")));
    let replaced = r#"
schema { check { safe = admin.enabled == false } }
admin = { enabled = false }
@patch("admin", 5)
"#;
    assert!(t009s(replaced)
        .iter()
        .any(|m| m.contains("could not be evaluated")));
}

#[test]
fn a_resilient_default_fill_is_visible_to_checks() {
    let source = r#"
schema { port = Int(default = 8080), check { ok = port > 0 } }
"#;
    let module = parse(source, "checks").expect("parse");
    let (_, diagnostics) = Engine::new().evaluate(&module).expect("resilient");
    assert!(diagnostics.iter().any(|d| d.code == "W005"));
    assert!(
        !diagnostics.iter().any(|d| d.code == "T009"),
        "{diagnostics:?}"
    );
}

#[test]
fn material_and_composite_equality_is_incomparable() {
    let secret = r#"
schema { check { same = secret.env("LP19_X") == "" } }
"#;
    assert!(t009s(secret)
        .iter()
        .any(|m| m.contains("could not be evaluated")));
    let table = r#"
schema { check { diff = a != b } }
a = { x = 1 }
b = { x = 1 }
"#;
    assert!(t009s(table)
        .iter()
        .any(|m| m.contains("could not be evaluated")));
}

#[test]
fn predicate_errors_are_t009_and_do_not_hide_earlier_checks() {
    let source = r#"
schema { check { first = false, second = 1 / 0 == 0 } }
"#;
    let messages = t009s(source);
    assert!(
        messages.iter().any(|m| m.contains("'first' failed")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("'second' could not be evaluated")),
        "{messages:?}"
    );
}

#[test]
fn checks_are_root_schema_only() {
    let nested = r#"
server {
  schema { port = Int, check { ok = port > 0 } }
  port = 5
}
"#;
    let diagnostics = parse(nested, "checks").expect_err("nested check refused");
    assert!(diagnostics
        .iter()
        .any(|d| d.code == "E020" && d.message.contains("root schema")));
}

#[test]
fn a_duplicate_schema_drops_the_discarded_schemas_checks() {
    let source = r#"
schema { check { stale = false } }
schema { port = Int }
port = 1
"#;
    let module = parse(source, "checks").expect("parse");
    let (_, diagnostics) = Engine::strict()
        .evaluate(&module)
        .expect("effective schema wins");
    assert!(
        !diagnostics.iter().any(|d| d.code == "T009"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_field_named_check_is_still_a_field() {
    let source = r#"
schema { check = String, inbox = String }
check = "hello"
inbox = "x"
"#;
    let module = parse(source, "checks").expect("parse");
    assert!(module.schema_checks.is_empty());
    let (value, _) = Engine::strict().evaluate(&module).expect("valid");
    assert!(value_to_pretty_json(&value).contains("\"check\": \"hello\""));
}

#[test]
fn strict_t009_messages_are_redacted_like_resilient_ones() {
    let source = r#"
schema { check { leakname = false } }
token = secret("leakname")
"#;
    let module = parse(source, "checks").expect("parse");
    let error = Engine::strict().evaluate(&module).expect_err("refused");
    let message = match error {
        active_core::EvalError::ValidationFailed { diagnostics } => diagnostics
            .into_iter()
            .find(|d| d.code == "T009")
            .map(|d| d.message)
            .expect("T009"),
        other => panic!("{other:?}"),
    };
    assert!(message.contains("***SECRET***"), "{message}");
    assert!(!message.contains("leakname"), "{message}");
}

#[test]
fn the_formatter_preserves_dotted_check_references_in_both_styles() {
    let source = r#"
schema {
  check {
    same = a.port == b.port
  }
}
a = { port = 1 }
b = { port = 2 }
"#;
    for style in [Style::Brace, Style::Indent] {
        let module = parse(source, "checks").expect("parse");
        let formatted = active_core::fmt::format(&module, style);
        assert!(formatted.contains("a.port == b.port"), "{formatted}");
        let reparsed = parse(&formatted, "checks").expect("formatted document parses");
        let (_, diagnostics) = Engine::new().evaluate(&reparsed).expect("evaluates");
        assert!(
            diagnostics.iter().any(|d| d.code == "T009"),
            "the failing gate survived formatting: {formatted}"
        );
        // Idempotent.
        assert_eq!(formatted, active_core::fmt::format(&reparsed, style));
    }
}

#[test]
fn inspection_and_compatibility_report_checks() {
    let old = parse(
        "schema { port = Int, check { floor = port > 0 } }\nport = 1\n",
        "checks",
    )
    .expect("parse");
    let changed = parse(
        "schema { port = Int, check { floor = port > 10 } }\nport = 1\n",
        "checks",
    )
    .expect("parse");
    let inspect = active_core::convert::schema_inspect(&old).expect("inspect");
    assert_eq!(inspect["checks"][0]["name"], "floor");
    assert!(inspect["checks"][0]["source"]
        .as_str()
        .is_some_and(|source| source.contains("port > 0")));
    let findings = active_core::convert::compare_schemas(&old, &changed).expect("compare");
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == "C045" && finding.message.contains("changed")),
        "{findings:?}"
    );
    let added = parse("schema { port = Int }\nport = 1\n", "checks").expect("parse");
    let findings = active_core::convert::compare_schemas(&added, &old).expect("compare");
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == "C045" && finding.message.contains("added")),
        "{findings:?}"
    );
}

#[test]
fn checks_resolve_root_keys_only() {
    // Identifiers are document keys; a let-only name is not resolvable.
    let binding = "schema { check { ok = limit > 0 } }\nlet limit = 5\nport = 1\n";
    let module = parse(binding, "checks").expect("parse");
    let (_, diagnostics) = Engine::new().evaluate(&module).expect("resilient");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "T009" && d.message.contains("could not be evaluated")),
        "{diagnostics:?}"
    );
    // A let that shadows a final root key never wins.
    let shadow = r#"
schema { check { ok = limit == 10 } }
let limit = 5
active_profile = "p"
profile "p" { limit = 10 }
"#;
    let module = parse(shadow, "checks").expect("parse");
    let (_, diagnostics) = Engine::strict()
        .evaluate(&module)
        .expect("the final tree value satisfies the check");
    assert!(
        !diagnostics.iter().any(|d| d.code == "T009"),
        "{diagnostics:?}"
    );
}

#[test]
fn nested_inline_and_let_schemas_refuse_checks() {
    let inline = "server = { schema = { port = Int, check { bad = false } }, port = 1 }\n";
    let diagnostics = parse(inline, "checks").expect_err("nested inline schema check refused");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "E020" && d.message.contains("root schema")),
        "{diagnostics:?}"
    );
    let let_table = "let s = { schema = { check { bad = false } } }\nx = 1\n";
    let diagnostics = parse(let_table, "checks").expect_err("let table schema check refused");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "E020" && d.message.contains("root schema")),
        "{diagnostics:?}"
    );
}

#[test]
fn material_equality_is_incomparable_in_every_form() {
    fn t009_with_env(source: &str, env: &[(&str, &str)]) -> Vec<String> {
        let module = parse(source, "checks").expect("parse");
        let mut engine = Engine::new();
        for (key, value) in env {
            engine.env.insert((*key).to_string(), (*value).to_string());
        }
        let (_, diagnostics) = engine.evaluate(&module).expect("resilient");
        diagnostics
            .into_iter()
            .filter(|d| d.code == "T009")
            .map(|d| d.message)
            .collect()
    }
    // Direct env equality with the CORRECT guess must still be incomparable.
    let direct = r#"
schema { check { same = env.LP19_TOKEN == "abc" } }
hidden = secret(env.LP19_TOKEN)
"#;
    assert!(
        t009_with_env(direct, &[("LP19_TOKEN", "abc")])
            .iter()
            .any(|m| m.contains("could not be evaluated")),
        "a correct guess must not pass"
    );
    // Path form.
    let path = r#"
schema { check { same = token == "abc" } }
token = env.LP19_TOKEN
hidden = secret(token)
"#;
    assert!(t009_with_env(path, &[("LP19_TOKEN", "abc")])
        .iter()
        .any(|m| m.contains("could not be evaluated")));
    // Plain copy of an authored secret literal.
    let copy = r#"
schema { check { same = token == "abc" } }
token = "abc"
hidden = secret("abc")
"#;
    assert!(t009s(copy)
        .iter()
        .any(|m| m.contains("could not be evaluated")));
}

#[test]
fn a_bom_on_the_first_line_does_not_corrupt_the_check_source() {
    let source = "\u{feff}schema { check { ok = flag == false } }\nflag = true\nfals = true\n";
    let module = parse(source, "checks").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let formatted = active_core::fmt::format(&module, style);
        assert!(formatted.contains("flag == false"), "{formatted}");
        let reparsed = parse(&formatted, "checks").expect("formatted parses");
        let (_, diagnostics) = Engine::new().evaluate(&reparsed).expect("evaluates");
        assert!(
            diagnostics.iter().any(|d| d.code == "T009"),
            "the gate survived formatting: {formatted}"
        );
    }
}

#[test]
fn c045_redacts_a_material_rule_name_in_path_and_message() {
    let old = parse(
        "schema { check { leakname = false } }\ntoken = secret(\"leakname\")\n",
        "checks",
    )
    .expect("parse");
    let new = parse(
        "schema { check { leakname = true } }\ntoken = secret(\"leakname\")\n",
        "checks",
    )
    .expect("parse");
    let findings = active_core::convert::compare_schemas(&old, &new).expect("compare");
    let finding = findings
        .iter()
        .find(|finding| finding.code == "C045")
        .expect("C045");
    assert!(!finding.path.contains("leakname"), "{}", finding.path);
    assert!(!finding.message.contains("leakname"), "{}", finding.message);
    assert!(finding.path.contains("***SECRET***") || finding.message.contains("***SECRET***"));
}

#[test]
fn check_diagnostics_are_capped_with_w019() {
    let mut source = String::from("schema { check {\n");
    for index in 0..1025 {
        source.push_str(&format!("r{index} = false\n"));
    }
    source.push_str("} }\n");
    let module = parse(&source, "checks").expect("parse");
    let (_, diagnostics) = Engine::new().evaluate(&module).expect("resilient");
    let t009 = diagnostics.iter().filter(|d| d.code == "T009").count();
    assert_eq!(t009, 1024, "capped T009 count");
    assert!(
        diagnostics.iter().any(|d| d.code == "W019"),
        "truncation notice"
    );
}

#[test]
fn checks_support_env_defaults_and_scalar_interpolation() {
    let env_default =
        "schema { check { ok = (env.LP19_MISSING_VALUE ?: \"x\") == \"x\" } }\nport = 1\n";
    assert!(t009s(env_default).is_empty(), "{:?}", t009s(env_default));
    let float = "schema { check { ok = \"${ratio}\" == \"1.5\" } }\nratio = 1.5\n";
    assert!(t009s(float).is_empty(), "{:?}", t009s(float));
    let duration = "schema { check { ok = 5ms < 10ms } }\nport = 1\n";
    assert!(t009s(duration).is_empty(), "{:?}", t009s(duration));
}

#[test]
fn generators_emit_a_check_notice_without_rule_material() {
    let module = parse(
        "schema { port = Int, check { leakname = port > 0 } }\nport = 1\n",
        "checks",
    )
    .expect("parse");
    let json = active_core::convert::schema_to_json_schema(&module)
        .expect("json schema")
        .to_string();
    let ts = active_core::convert::schema_to_typescript(&module, "Config").expect("ts");
    let go = active_core::convert::schema_to_go(&module, "Config").expect("go");
    let rust = active_core::convert::schema_to_rust(&module, "Config").expect("rust");
    let python = active_core::convert::schema_to_python(&module, "Config").expect("python");
    for artifact in [&json, &ts, &go, &rust, &python] {
        assert!(
            artifact.contains("cross-field check rule"),
            "missing notice: {artifact}"
        );
        assert!(
            !artifact.contains("leakname"),
            "a rule name leaked: {artifact}"
        );
    }
}

#[test]
fn a_rejected_nested_check_expression_never_reaches_the_walk() {
    let source = "server { schema { check { bad = 1 / 0 } } }\n";
    let diagnostics = parse(source, "checks").expect_err("E020");
    assert!(diagnostics.iter().any(|d| d.code == "E020"));
    assert!(
        !diagnostics.iter().any(|d| d.code == "T001"),
        "the rejected predicate surfaced a fatal expression error: {diagnostics:?}"
    );
    // Recovering hosts keep the module; the predicate nodes stay out of
    // the value graph.
    let (module, _) = active_core::parse_recovering(source, "checks", Default::default());
    if let Ok((_, diags)) = Engine::new().evaluate(&module) {
        assert!(!diags.iter().any(|d| d.code == "T001"), "{diags:?}");
    }
}

#[test]
fn indentation_and_header_nested_schemas_refuse_checks() {
    let indentation = "server:\n  schema:\n    port = Int\n    check:\n      bad = false\n";
    let diagnostics =
        parse(indentation, "checks").expect_err("nested indentation schema check refused");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "E020" && d.message.contains("root schema")),
        "{diagnostics:?}"
    );
    let header = "[server.schema]\nport = Int\ncheck { bad = false }\n";
    let diagnostics = parse(header, "checks").expect_err("nested header schema check refused");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "E020" && d.message.contains("root schema")),
        "{diagnostics:?}"
    );
}

#[test]
fn checks_use_the_engine_env_fallback_and_null_semantics() {
    let source = r#"
schema { check { ok = env.LP19_LOOKUP == "dev" } }
tier = env.LP19_LOOKUP ?: "prod"
"#;
    let module = parse(source, "checks").expect("parse");
    let mut engine = Engine::new();
    engine.env.clear();
    let lookup: active_core::eval::EnvLookup =
        std::sync::Arc::new(|name: &str| (name == "LP19_LOOKUP").then(|| "dev".to_string()));
    engine = engine.with_env_fallback(lookup);
    let (_, diagnostics) = engine.evaluate(&module).expect("evaluates");
    assert!(
        !diagnostics.iter().any(|d| d.code == "T009"),
        "the check must see the host fallback: {diagnostics:?}"
    );

    // A missing env evaluates to null, exactly like the walk.
    let missing = "schema { check { absent = env.LP19_UNSET_XYZ == null } }\nport = 1\n";
    let module = parse(missing, "checks").expect("parse");
    let (_, diagnostics) = Engine::new().evaluate(&module).expect("evaluates");
    assert!(
        !diagnostics.iter().any(|d| d.code == "T009"),
        "{diagnostics:?}"
    );
}

#[test]
fn duplicate_check_names_are_refused() {
    let source = "schema { check { a = true, a = port > 0 } }\nport = 1\n";
    let diagnostics = parse(source, "checks").expect_err("duplicate refused");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "E020" && d.message.contains("duplicate check name")),
        "{diagnostics:?}"
    );
}
