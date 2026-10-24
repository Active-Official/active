//! Redaction sweep: every core export path over one secret-bearing
//! document must show `***SECRET***` and never the raw value.
//!
//! The formatter is source-faithful by contract (it reprints the
//! `secret(...)` locator the user wrote, like any literal) while the
//! EVALUATED value redacts — both halves pinned here so neither drifts.

use active_core::convert::{
    collect_items, doc_to_html, doc_to_man, doc_to_markdown, value_to_pretty_json,
    value_to_toml_string, value_to_yaml_string,
};
use active_core::fmt::format;
use active_core::viz;
use active_core::{parse, Budget, Engine, Style};

const RAW: &str = "s3cr3t-sweep-9z";
const DOC: &str = "token = secret(\"s3cr3t-sweep-9z\")\nport = 8080\n[server]\nhost = \"h\"\n";

fn evaluated() -> active_core::Value {
    let module = parse(DOC, "sweep").expect("must parse");
    Engine::new().evaluate(&module).expect("must evaluate").0
}

fn items() -> Vec<active_core::convert::DocItem> {
    let module = parse(DOC, "sweep").expect("must parse");
    let value = evaluated();
    collect_items(&module, &value)
}

#[test]
fn json_yaml_toml_exports_redact() {
    let value = evaluated();
    for (label, text) in [
        ("json", value_to_pretty_json(&value)),
        ("yaml", value_to_yaml_string(&value).expect("yaml")),
        ("toml", value_to_toml_string(&value).expect("toml").0),
    ] {
        assert!(!text.contains(RAW), "{label} leaked raw secret:\n{text}");
        assert!(
            text.contains("***SECRET***"),
            "{label} lost marker:\n{text}"
        );
    }
}

#[test]
fn docgen_all_formats_redact() {
    let items = items();
    for (label, text) in [
        ("markdown", doc_to_markdown("Svc", &items)),
        ("html", doc_to_html("Svc", &items)),
        ("man", doc_to_man("Svc", &items)),
    ] {
        assert!(!text.contains(RAW), "{label} leaked raw secret:\n{text}");
        assert!(
            text.contains("***SECRET***"),
            "{label} lost marker:\n{text}"
        );
    }
}

#[test]
fn tooling_surfaces_redact_derived_material() {
    // A plain value used as a secret locator (and an interpolation over
    // it) is material without a `Value::Secret` marker. Sweep every
    // tooling emitter over one such document so a new surface cannot
    // regress the invariant silently.
    let src = "x = \"s3cr3t-derived\"\nmsg = \"pre-${x}\"\nk = secret(x)\nt = { a = x }\n";
    let module = parse(src, "sweep-derived").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module)
        .expect("must step");
    let taint = viz::taint_data(&module, &value);
    let budget = viz::budget_data(&module, &value, &Budget::default(), &diags);
    let graph = active_core::graph::secret_tainted_paths_with(&module, &value, None);
    let inspect_x = active_core::convert::inspect_path(&module, &value, "x").expect("inspect x");
    let inspect_msg =
        active_core::convert::inspect_path(&module, &value, "msg").expect("inspect msg");
    assert_eq!(inspect_x["tainted"], true, "{inspect_x}");
    assert_eq!(inspect_msg["tainted"], true, "{inspect_msg}");
    for (label, text) in [
        ("steps", serde_json::to_string(&steps).expect("step json")),
        ("taint", taint.to_string()),
        ("budget", budget.to_string()),
        ("graph", graph.join(",")),
        ("inspect-x", inspect_x.to_string()),
        ("inspect-msg", inspect_msg.to_string()),
    ] {
        assert!(
            !text.contains("s3cr3t-derived"),
            "{label} leaked derived material: {text}"
        );
    }
}

#[test]
fn structural_material_never_reaches_exports_or_the_content_hash() {
    use std::collections::HashMap;
    // A plain env read used as a secret locator, its aliases, a
    // container, and an interpolation are all material. Exports must
    // mark them, and the content hash must be stable across the
    // material value itself.
    let src = "let t = env.R20SECRET\nk = secret(t)\nleak = t\ntable = { inner = t }\ncomposed = \"pre-${t}\"\n";
    let module = parse(src, "export-sweep").expect("must parse");
    let evaluate = |value: &str| {
        let mut env = HashMap::new();
        env.insert("R20SECRET".to_string(), value.to_string());
        Engine::hermetic()
            .with_env(env)
            .evaluate(&module)
            .expect("must evaluate")
            .0
    };
    let first = evaluate("ENVSECRET-export");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    let redacted = active_core::convert::redact_value(&profile, &first);
    let json = active_core::convert::value_to_json(&redacted);
    let yaml = active_core::convert::value_to_yaml_string(&redacted).expect("yaml");
    let toml = active_core::convert::value_to_toml_string(&redacted)
        .expect("toml")
        .0;
    for (label, text) in [("json", json.to_string()), ("yaml", yaml), ("toml", toml)] {
        assert!(
            !text.contains("ENVSECRET"),
            "{label} leaked structural material: {text}"
        );
        assert!(
            text.contains("***SECRET***"),
            "{label} lost markers: {text}"
        );
    }
    let first_hash = active_core::provenance::content_hash(&redacted);
    let second = evaluate("OTHER-SECRET-999");
    let second_redacted = active_core::convert::redact_value(&profile, &second);
    assert_eq!(
        first_hash,
        active_core::provenance::content_hash(&second_redacted),
        "the content hash must not depend on material values"
    );
}

#[test]
fn formatter_keeps_source_locator_but_value_redacts() {
    // Source fidelity: the formatter reprints what the user wrote,
    // including the secret locator (formatters echo source; `cat`
    // does the same). The evaluated tree still redacts.
    let module = parse(DOC, "sweep").expect("must parse");
    for style in [Style::Brace, Style::Indent, Style::Preserve] {
        let out = format(&module, style);
        assert!(out.contains("secret("), "locator dropped:\n{out}");
    }
    let json = value_to_pretty_json(&evaluated());
    assert!(!json.contains(RAW), "{json}");
    assert!(json.contains("***SECRET***"), "{json}");
}
