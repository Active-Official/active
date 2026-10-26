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

#[test]
fn doc_redacts_secret_text_quoted_in_an_intent() {
    // Review 64: annotation and comment prose renders through docgen,
    // so a quoted locator must redact there too.
    let src =
        "token = secret(\"s3cr3t-doc\")\n# @intent: \"the token is s3cr3t-doc\"\nname = \"x\"\n";
    let module = parse(src, "doc").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let items = collect_items(&module, &value);
    let md = doc_to_markdown("Service", &items);
    assert!(!md.contains("s3cr3t-doc"), "{md}");
}

#[test]
fn doc_redacts_schema_prose_that_quotes_a_secret() {
    // Review 65: deprecated reasons, patterns, and defaults are
    // authored text too.
    let src = "token = secret(\"s3cr3t-doc\")\nschema = { host = String(deprecated = \"use s3cr3t-doc instead\") }\nhost = \"x\"\n";
    let module = parse(src, "doc").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let raw = active_core::convert::value_to_pretty_json(&value);
    let items = collect_items(&module, &value);
    let md = doc_to_markdown("Service", &items);
    assert!(!md.contains("s3cr3t-doc"), "{md} / {raw}");
}

#[test]
fn docgen_stays_linear_with_many_secrets_and_fields() {
    // Regression for the quadratic docgen path: `redact_every_occurrence`
    // rebuilt a full redactor (and its automata) per prose field. The old
    // implementation took ~22 s on a 800-secret/310 KB probe; this
    // 200-secret/3000-field probe must stay well under 10 s.
    let mut src = String::new();
    for i in 0..300 {
        src.push_str(&format!("let s{i} = secret(\"secret-value-{i:04}\")\n"));
    }
    src.push_str("schema = {\n");
    for i in 0..3000 {
        src.push_str(&format!(
            "  f{i} = String(description = \"prose field {i}\") \n"
        ));
    }
    src.push_str("}\n");
    for i in 0..3000 {
        src.push_str(&format!("f{i} = \"value {i}\"\n"));
    }
    let module = parse(&src, "doc-perf").expect("perf document parses");
    let engine = Engine::new();
    let (value, _) = engine.evaluate(&module).expect("perf document evaluates");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    let started = std::time::Instant::now();
    let items = active_core::convert::collect_items_with_profile(&module, &value, &profile);
    let elapsed = started.elapsed();
    assert!(items.len() >= 3000, "items: {}", items.len());
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "docgen is not linear: {elapsed:?} for 300 secrets / 3000 fields"
    );
}
