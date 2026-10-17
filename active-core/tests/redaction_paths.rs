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
use active_core::{parse, Engine, Style};

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
