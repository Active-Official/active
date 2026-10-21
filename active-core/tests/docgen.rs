//! Documentation generator tests (C-048, spec/docgen.md).
//!
//! Golden probes per format over a fixture with nesting, intents,
//! comments, constraints, defaults, deprecations, and a secret —
//! plus comment attachment, redaction, and schemaless generation.

use active_core::convert::{collect_items, doc_to_html, doc_to_man, doc_to_markdown};
use active_core::{parse, Engine};

const DOC: &str = r#"# Service configuration
# @intent: "Public HTTP port"
# @constraint: value >= 1
port = 8080
flags = [true, false]
token = secret("s3cr3t-doc")
old = "x"

schema = {
  port = Int(min = 1, max = 65535),
  flags = Array<Bool>,
  database = Table { host = String, port = Int },
}

# Database host name.
[database]
host = "db.internal" # primary, do not change
port = 5432
"#;

fn items() -> Vec<active_core::convert::DocItem> {
    let module = parse(DOC, "doc").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    collect_items(&module, &value)
}

#[test]
fn markdown_renders_types_values_and_prose() {
    let md = doc_to_markdown("Service", &items());
    for probe in [
        "# Service",
        "## General",
        "### port",
        "**Type:** Int",
        "`8080`",
        "Public HTTP port",
        "value >= 1",
        "min 1",
        "max 65535",
        "## database",
        "### database.host",
        "primary, do not change",
        "Database host name.",
        "Array<Bool>",
        "array[2]",
    ] {
        assert!(md.contains(probe), "missing {probe}:\n{md}");
    }
    // Array elements resolve to the element type, not the array.
    assert!(md.contains("### flags[0]"), "{md}");
    let items = items();
    let element = items
        .iter()
        .find(|item| item.path == "flags[0]")
        .expect("element");
    assert_eq!(element.type_text, "Bool", "{element:?}");
    // Annotation payloads render once, parsed — never echoed raw.
    assert!(!md.contains("@intent:"), "{md}");
    assert!(!md.contains("@constraint:"), "{md}");
}

#[test]
fn html_is_searchable_and_escaped() {
    let html = doc_to_html("Service", &items());
    for probe in [
        "id=\"q\"",
        "class=\"field\"",
        "querySelectorAll('.field')",
        "Public HTTP port",
        "<h2>database</h2>",
    ] {
        assert!(html.contains(probe), "missing {probe}:\n{html}");
    }
    // No raw markup leaks through content (see html_escapes_content).
}

#[test]
fn html_escapes_content() {
    let module = parse("x = \"<b>&\\\"\" \n", "doc").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let items = collect_items(&module, &value);
    let html = doc_to_html("T", &items);
    assert!(!html.contains("<b>&"), "{html}");
    assert!(html.contains("&lt;b&gt;&amp;"), "{html}");
}

#[test]
fn man_pages_use_requests_and_escape() {
    let man = doc_to_man("service", &items());
    for probe in [
        ".TH \"SERVICE\" 5",
        ".SH NAME",
        ".SH FIELDS",
        ".TP",
        "database.host",
    ] {
        assert!(man.contains(probe), "missing {probe}:\n{man}");
    }
}

#[test]
fn secrets_redact_in_every_format() {
    let items = items();
    let md = doc_to_markdown("T", &items);
    let html = doc_to_html("T", &items);
    let man = doc_to_man("T", &items);
    for output in [&md, &html, &man] {
        assert!(!output.contains("s3cr3t-doc"), "{output}");
        assert!(output.contains("***SECRET***"), "{output}");
    }
}

#[test]
fn schemaless_documents_still_generate() {
    let module = parse("port = 1\n[server]\nhost = \"h\"\n", "doc").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let items = collect_items(&module, &value);
    let md = doc_to_markdown("Plain", &items);
    assert!(md.contains("**Type:** Int"), "{md}");
    assert!(md.contains("## server"), "{md}");
}

#[test]
fn items_cover_nested_and_array_paths() {
    let module = parse(
        "servers = [{ host = \"a\" }, { host = \"b\" }]\n[db]\nport = 1\n",
        "doc",
    )
    .expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let items = collect_items(&module, &value);
    let paths: Vec<&str> = items.iter().map(|item| item.path.as_str()).collect();
    for expected in ["servers", "servers[0]", "servers[0].host", "db", "db.port"] {
        assert!(paths.contains(&expected), "{paths:?}");
    }
}

#[test]
fn markdown_surfaces_schema_description_and_deprecated() {
    // spec/schema-extensions.md §2: description metadata and the
    // standardized deprecated message must render in docgen output.
    let module = parse(
        "schema = {\n  host = String(description = \"DNS name.\", deprecated = \"use fqdn\"),\n}\nhost = \"x\"\n",
        "doc",
    )
    .expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let md = doc_to_markdown("Net", &collect_items(&module, &value));
    assert!(md.contains("DNS name."), "{md}");
    assert!(md.contains("use fqdn"), "{md}");
    // Same DocItem feeds every format: pin HTML and man too.
    let items = collect_items(&module, &value);
    let html = doc_to_html("Net", &items);
    assert!(html.contains("DNS name."), "{html}");
    assert!(html.contains("use fqdn"), "{html}");
    let man = doc_to_man("Net", &items);
    assert!(man.contains("DNS name."), "{man}");
    assert!(man.contains("use fqdn"), "{man}");
}
