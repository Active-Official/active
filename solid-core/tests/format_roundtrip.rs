//! Formatter round-trip tests (C-022).
//!
//! Proves the dual-syntax guarantees from spec/dual-syntax.md §8.1 on the
//! real path: canonical documents round-trip byte-for-byte, formatting is
//! idempotent, comments survive in every mode, and the two styles produce
//! the same tree.

use solid_core::fmt::format;
use solid_core::{parse, Engine, Style};

fn parse_ok(src: &str) -> solid_core::Module {
    parse(src, "fmt").unwrap_or_else(|d| panic!("parse failed: {d:?}"))
}

fn fmt(src: &str, style: Style) -> String {
    format(&parse_ok(src), style)
}

fn tree(src: &str) -> String {
    let module = parse_ok(src);
    let engine = Engine::new();
    let (value, _) = engine.evaluate(&module).expect("must evaluate");
    solid_core::convert::value_to_pretty_json(&value)
}

const BRACE_DOC: &str = "[app]\nname = \"svc\"\nport = 8080\n\n[database]\nhost = \"db\"\n";

const INDENT_DOC: &str = "app:\n  name = \"svc\"\n  port = 8080\n\ndatabase:\n  host = \"db\"\n";

#[test]
fn canonical_brace_document_round_trips_byte_for_byte() {
    assert_eq!(fmt(BRACE_DOC, Style::Brace), BRACE_DOC);
    // Preserve mode of a canonical brace doc is the same bytes.
    assert_eq!(fmt(BRACE_DOC, Style::Preserve), BRACE_DOC);
}

#[test]
fn canonical_indent_document_round_trips_byte_for_byte() {
    assert_eq!(fmt(INDENT_DOC, Style::Indent), INDENT_DOC);
    assert_eq!(fmt(INDENT_DOC, Style::Preserve), INDENT_DOC);
}

#[test]
fn formatting_is_idempotent() {
    for src in [BRACE_DOC, INDENT_DOC] {
        for style in [Style::Brace, Style::Indent, Style::Preserve] {
            let once = fmt(src, style);
            let twice = fmt(&once, style);
            assert_eq!(once, twice, "not idempotent for {style:?}:\n{once}");
        }
    }
}

#[test]
fn formatting_is_idempotent_on_the_whole_positive_corpus() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/positive");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("positive corpus")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "solid"))
        .collect();
    files.sort();
    assert!(files.len() >= 20, "expected the full corpus");
    for f in files {
        let src = std::fs::read_to_string(&f).expect("read");
        let module = parse(&src, "fmt").unwrap_or_else(|d| panic!("{f:?}: {d:?}"));
        for style in [Style::Brace, Style::Indent] {
            let once = format(&module, style);
            let reparsed = parse(&once, "fmt").unwrap_or_else(|d| {
                panic!("{f:?}: formatted output must re-parse ({style:?}): {d:?}\n{once}")
            });
            let twice = format(&reparsed, style);
            assert_eq!(once, twice, "not idempotent for {f:?} ({style:?})");
            // The formatted output must describe the same tree.
            let engine = Engine::new();
            let original = engine.evaluate(&module);

            let restored = engine.evaluate(&reparsed);
            if let (Ok((a, _)), Ok((b, _))) = (original, restored) {
                let (ja, jb) = (
                    solid_core::convert::value_to_pretty_json(&a),
                    solid_core::convert::value_to_pretty_json(&b),
                );
                assert_eq!(
                    ja, jb,
                    "{f:?}: formatted output changed the tree ({style:?})"
                );
            }
        }
    }
}

#[test]
fn comments_survive_every_mode() {
    let src = "# document header\napp:\n  # leading on name\n  name = \"svc\"  # trailing\n  port = 8080\n  # trailing block\n\n# before database\n[database]\nhost = \"db\"\n";
    let module = parse_ok(src);
    assert_eq!(module.trivia.len(), 5, "all five comments must be recorded");
    for style in [Style::Brace, Style::Indent, Style::Preserve] {
        let out = format(&module, style);
        for needle in [
            "# document header",
            "# leading on name",
            "# trailing",
            "# trailing block",
            "# before database",
        ] {
            assert!(out.contains(needle), "{needle:?} lost in {style:?}:\n{out}");
        }
    }
}

#[test]
fn annotations_are_preserved_like_comments() {
    let src =
        "[app]\n# @intent: application identity\nname = \"svc\"\n# @ai_immutable\nport = 8080\n";
    let out = fmt(src, Style::Indent);
    assert!(out.contains("# @intent: application identity"), "{out}");
    assert!(out.contains("# @ai_immutable"), "{out}");
}

#[test]
fn styles_agree_on_the_tree_and_produce_each_other() {
    // Formatting the indentation document as brace style must produce the
    // canonical brace document, and vice versa.
    assert_eq!(fmt(INDENT_DOC, Style::Brace), BRACE_DOC);
    assert_eq!(fmt(BRACE_DOC, Style::Indent), INDENT_DOC);
    assert_eq!(tree(BRACE_DOC), tree(INDENT_DOC));
}

#[test]
fn preserve_mode_follows_the_source_style() {
    let from_indent = fmt(INDENT_DOC, Style::Preserve);
    assert!(from_indent.contains("app:"), "{from_indent}");
    let from_brace = fmt(BRACE_DOC, Style::Preserve);
    assert!(from_brace.contains("[app]"), "{from_brace}");
}

#[test]
fn let_bindings_and_expressions_round_trip() {
    let src = "let region = env.AWS_REGION ?: \"us-east-1\"\nlet timeout = 5s\n\n[app]\nregion = region\nurl = \"https://api.${region}.example.com\"\nwindow = timeout * 2\nflag = !false\nratio = 1.5\n";
    let module = parse_ok(src);
    for style in [Style::Brace, Style::Indent] {
        let once = format(&module, style);
        let reparsed = parse(&once, "fmt")
            .unwrap_or_else(|d| panic!("must re-parse ({style:?}): {d:?}\n{once}"));
        let twice = format(&reparsed, style);
        assert_eq!(once, twice, "not idempotent ({style:?})");
    }
}

#[test]
fn precedence_needs_no_extra_parentheses_but_keeps_meaning() {
    let src = "[calc]\na = 2 + 3 * 4\nb = (2 + 3) * 4\nc = 10 - 2 - 3\n";
    let module = parse_ok(src);
    let out = format(&module, Style::Brace);
    let reparsed = parse(&out, "fmt").expect("formatted output must re-parse");
    let engine = Engine::new();
    let (a, _) = engine.evaluate(&module).expect("evaluate original");
    let (b, _) = engine.evaluate(&reparsed).expect("evaluate formatted");
    assert_eq!(
        solid_core::convert::value_to_pretty_json(&a),
        solid_core::convert::value_to_pretty_json(&b),
        "parenthesization must preserve meaning:\n{out}"
    );
    assert!(
        out.contains("(2 + 3) * 4"),
        "explicit grouping must survive:\n{out}"
    );
}
