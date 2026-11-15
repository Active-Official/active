//! `requires` resolution agreement across surfaces (LANG-B-001, B-002).
//!
//! One rule: a `requires` target is sibling-relative (its containing
//! table), falling back to root-absolute. Validation enforces it, lint
//! does not flag it, and codegen emits `dependentRequired` in the right
//! table.

use active_core::{lint, parse, Engine};

const SRC: &str = "schema = {\n  tls = Bool,\n  server = Table { cert = String(requires = \"tls\"), tls = Bool },\n}\n";

#[test]
fn lint_accepts_a_sibling_requires_target() {
    let module = parse(SRC, "requires").expect("parse");
    let findings = lint::lint_with(&module, &lint::LintOptions::default());
    assert!(
        !findings.iter().any(|d| d.code == "L011"),
        "nested requires must not be flagged: {findings:?}"
    );
}

#[test]
fn lint_still_flags_a_dangling_target() {
    let src = "schema = { a = String(requires = \"ghost\") }\n";
    let module = parse(src, "requires").expect("parse");
    let findings = lint::lint_with(&module, &lint::LintOptions::default());
    assert!(findings.iter().any(|d| d.code == "L011"), "{findings:?}");
}

#[test]
fn codegen_emits_dependent_required_in_the_right_table() {
    let module = parse(SRC, "requires").expect("parse");
    let json = active_core::convert::gen::schema_to_json_schema(&module).expect("generate");
    let dep = &json["properties"]["server"]["dependentRequired"];
    assert_eq!(dep["tls"][0], "cert", "{json}");
    assert!(
        json["dependentRequired"]["tls"].is_null(),
        "the dependency belongs to the nested table, not the root: {json}"
    );
}

#[test]
fn validation_enforces_the_sibling_target() {
    // cert present + nested tls true -> an enabled trigger requires cert's
    // peer? Here `cert requires tls`: cert present, tls absent -> T004.
    let src = "schema = {\n  server = Table { cert = String(requires = \"tls\"), tls = Bool },\n}\nserver = { cert = \"c\" }\n";
    let module = parse(src, "requires").expect("parse");
    let (_value, diags) = Engine::new().evaluate(&module).expect("evaluate");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "T004" && d.message.contains("'server.tls'")),
        "nested sibling target must be enforced: {diags:?}"
    );
}

#[test]
fn a_let_bound_target_names_the_literal() {
    // `let t = "mode"` + `requires = t` must name the target, not fall
    // back to the generic wording (LANG-B-025).
    let src = "let t = \"mode\"\nschema = { mode = String, replicas = Int(requires = t) }\nmode = \"prod\"\n";
    let module = parse(src, "requires").expect("parse");
    let (_v, diags) = Engine::new().evaluate(&module).expect("evaluate");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "T004" && d.message.contains("'mode'")),
        "{diags:?}"
    );
}
