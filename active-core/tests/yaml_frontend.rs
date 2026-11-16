//! YAML frontend baseline (LANG-I-004).
//!
//! `serde_yaml 0.9.34` is versioned `+deprecated` over `unsafe-libyaml`,
//! an unmaintained C transpile with no patch path (RUSTSEC-2024-0449).
//! A migration to a maintained frontend needs evidence, so this file pins
//! the CURRENT frontend's output byte-for-byte for the shapes the
//! converters promise: a replacement must reproduce these strings, the
//! W007 codes, and the export bytes before it can ship. Any deliberate
//! change lands with updated fixtures and conformance vectors in the same
//! change, and the accepted-risk record in `.cargo/audit.toml` names the
//! owner and the re-evaluation trigger.

use active_core::convert::{value_to_yaml_string, yaml_text_to_active};
use active_core::{parse, Engine};

fn evaluated(source: &str) -> String {
    let module = parse(source, "yaml-frontend").expect("emitted source parses");
    let (value, _) = Engine::hermetic().evaluate(&module).expect("must evaluate");
    active_core::convert::value_to_pretty_json(&value)
}

#[test]
fn import_output_is_pinned_for_the_documented_shapes() {
    let cases: &[(&str, &str)] = &[
        // Nested mapping → a table block (brace-style import output).
        (
            "server:\n  host: h\n  port: 8080\n",
            "server {\n  host = \"h\"\n  port = 8080\n}\n",
        ),
        // Sequence and scalar.
        (
            "flags: [true, false]\ncount: 3\n",
            "flags = [true, false]\ncount = 3\n",
        ),
        // Non-string keys stringify and are renamed (W007 pair).
        ("123: a\ntrue: b\n", "_123 = \"a\"\ntrue_ = \"b\"\n"),
        // Empty mapping/sequence and an explicit null.
        (
            "empty: {}\nlist: []\nnullish: null\n",
            "empty {\n}\nlist = []\nnullish = null\n",
        ),
    ];
    for (yaml, want) in cases {
        let (source, diags) = yaml_text_to_active(yaml).expect("import");
        assert_eq!(source, *want);
        if yaml.contains("123:") {
            let codes: Vec<&str> = diags.iter().map(|d| d.code.as_str()).collect();
            assert!(codes.contains(&"W007"), "{codes:?}");
        }
        // The emitted source evaluates identically on a second pass: the
        // frontend's output is a stable input to the compiler.
        let first = evaluated(&source);
        assert_eq!(first, evaluated(&source));
    }
}

#[test]
fn export_output_is_pinned() {
    // The other direction shares the same stack (`serde_yaml`): a new
    // frontend must keep these bytes.
    let module = parse("a = 1\nb = [true, \"x\"]\n[t]\nk = \"v\"\n", "yaml-export").expect("parse");
    let (value, _) = Engine::hermetic().evaluate(&module).expect("evaluate");
    let yaml = value_to_yaml_string(&value).expect("yaml export");
    assert_eq!(yaml, "a: 1\nb:\n- true\n- x\nt:\n  k: v\n");
}

#[test]
fn syntax_errors_stay_hard_errors() {
    // Diagnostic stability: a YAML syntax failure is an Err, never a
    // partially imported document.
    assert!(yaml_text_to_active("[unclosed\n").is_err());
    assert!(yaml_text_to_active("a: b: c\n").is_err());
}
