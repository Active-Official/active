//! Multi-line strings and semantic blocks (C-024).
//!
//! Drives the real shipped path (parse → evaluate → export): common-indent
//! stripping, verbatim `'''` semantics, interpolation in `"""`, semantic
//! tags in both statement and value position, E014 diagnostics, and
//! formatter round-trips.

use active_core::{parse, Engine};

fn eval_json(src: &str) -> (String, Vec<active_core::Diagnostic>) {
    let module = parse(src, "ml").expect("must parse");
    let engine = Engine::new();
    let (value, diags) = engine.evaluate(&module).expect("must evaluate");
    (active_core::convert::value_to_pretty_json(&value), diags)
}

#[test]
fn triple_double_quote_strips_common_indent() {
    let (json, diags) = eval_json("[doc]\nbody = \"\"\"\n  Hello.\n    Deep.\n  Back.\n  \"\"\"\n");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(
        json.contains("Hello.\\n  Deep.\\nBack."),
        "common indent stripped, relative indent kept: {json}"
    );
}

#[test]
fn triple_single_quote_is_verbatim() {
    let (json, diags) = eval_json("[a]\nb = '''\n  C:\\path\\x\n  ${no_interp}\n  '''\n");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(
        json.contains("C:\\\\path\\\\x") && json.contains("${no_interp}"),
        "backslashes and interpolation kept verbatim: {json}"
    );
}

#[test]
fn triple_double_quote_interpolates() {
    let (json, diags) = eval_json("let who = \"x\"\n[a]\nb = \"\"\"\n  hi ${who}\n  \"\"\"\n");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(json.contains("hi x"), "interpolation resolves: {json}");
}

#[test]
fn single_line_content_without_surrounding_newlines() {
    // `"""one-liner"""` — no newlines at all; nothing to strip.
    let (json, _) = eval_json("x = \"\"\"one-liner\"\"\"\n");
    assert!(json.contains("one-liner"), "{json}");
}

#[test]
fn blank_lines_inside_become_empty() {
    let (json, _) = eval_json("[a]\nb = \"\"\"\n  line one\n\n  line two\n  \"\"\"\n");
    assert!(json.contains("line one\\n\\nline two"), "{json}");
}

#[test]
fn semantic_tag_value_position_keeps_value() {
    let (json, diags) = eval_json("[a]\nb = @prompt \"\"\"\n  Be helpful.\n  \"\"\"\n");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(json.contains("Be helpful."), "{json}");
}

#[test]
fn bare_semantic_statement_binds_tag_name() {
    let (json, diags) = eval_json("@prompt \"\"\"\n  Be helpful.\n  \"\"\"\n");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(json.contains("\"prompt\": \"Be helpful.\""), "{json}");
}

#[test]
fn semantic_tag_all_three_kinds() {
    let (json, diags) =
        eval_json("@prompt \"\"\"p\"\"\"\n@policy '''q'''\n@template \"\"\"t\"\"\"\n");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(json.contains("\"prompt\": \"p\""), "{json}");
    assert!(json.contains("\"policy\": \"q\""), "{json}");
    assert!(json.contains("\"template\": \"t\""), "{json}");
}

#[test]
fn unknown_tag_is_e014() {
    let diags = parse("@promtp \"\"\"x\"\"\"\n", "ml").expect_err("unknown tag must fail");
    assert!(diags.iter().any(|d| d.code == "E014"), "{diags:?}");
}

#[test]
fn tag_on_single_line_string_is_e014() {
    let diags = parse("x = @prompt \"nope\"\n", "ml").expect_err("tag needs a multi-line string");
    assert!(diags.iter().any(|d| d.code == "E014"), "{diags:?}");
}

#[test]
fn unterminated_multiline_records_h001() {
    let module = parse("x = \"\"\"abc\n", "ml").expect("unterminated ml must heal");
    assert!(
        module.healing.iter().any(|d| d.code == "H001"),
        "H001 expected in {:?}",
        module.healing
    );
}

#[test]
fn formatter_round_trips_semantic_block() {
    let src = "@prompt \"\"\"\n  Be helpful.\n  \"\"\"\n";
    let module = parse(src, "ml").expect("must parse");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    assert!(out.contains("@prompt"), "tag survives formatting: {out}");
    // Reparse the formatted output: same value.
    let (first, _) = eval_json(src);
    let module2 = parse(&out, "ml2").expect("formatted output must parse");
    let (v2, _) = Engine::new().evaluate(&module2).expect("must evaluate");
    let second = active_core::convert::value_to_pretty_json(&v2);
    assert_eq!(first, second, "formatter round-trip changed the value");
}

#[test]
fn formatter_round_trips_interpolated_multiline() {
    let src = "let who = \"x\"\n[a]\nb = \"\"\"\n  hi ${who}\n  \"\"\"\n";
    let module = parse(src, "ml").expect("must parse");
    for style in [active_core::Style::Brace, active_core::Style::Indent] {
        let out = active_core::fmt::format(&module, style);
        let module2 = parse(&out, "ml2").expect("formatted output must parse");
        let (v2, _) = Engine::new().evaluate(&module2).expect("must evaluate");
        let second = active_core::convert::value_to_pretty_json(&v2);
        assert!(
            second.contains("hi x"),
            "interpolation survived {style:?} formatting: {out} -> {second}"
        );
    }
}
