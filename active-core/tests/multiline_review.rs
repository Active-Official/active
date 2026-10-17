//! Review-finding regressions for C-024 (second review round).
//!
//! Each test pins one fixed finding: single-pass ml escaping, the H001
//! triple delimiter suggestion, span-accurate ml interpolation errors,
//! comment attachment on bare semantic statements, and no duplicate
//! dependency edges for semantic blocks.

use active_core::{parse, Engine};

fn eval_json(src: &str) -> String {
    let module = parse(src, "mlr").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    active_core::convert::value_to_pretty_json(&value)
}

#[test]
fn escaped_dollar_and_quotes_survive_format_round_trip() {
    // A literal `\${name}` and embedded quotes must never become
    // interpolation or close the string early after formatting.
    let src = "[a]\nb = @prompt \"\"\"\n  cost: \\${5} and \"quoted\"\n  \"\"\"\n";
    let first = eval_json(src);
    assert!(
        first.contains("cost: ${5}"),
        "escaped dollar is literal: {first}"
    );
    assert!(first.contains("\\\"quoted\\\""), "quotes kept: {first}");
    let module = parse(src, "mlr").expect("must parse");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    let module2 = parse(&out, "mlr2").expect("formatted output must parse");
    let (v2, _) = Engine::new().evaluate(&module2).expect("must evaluate");
    let second = active_core::convert::value_to_pretty_json(&v2);
    assert_eq!(first, second, "format round-trip altered the value: {out}");
}

#[test]
fn h001_suggests_the_triple_delimiter() {
    let module = parse("x = \"\"\"abc\n", "mlr").expect("unterminated ml must heal");
    let d = module
        .healing
        .iter()
        .find(|d| d.code == "H001")
        .expect("H001 expected");
    let suggested = d
        .repair
        .as_ref()
        .expect("repair expected")
        .suggested
        .clone();
    assert_eq!(
        suggested, "\"\"\"",
        "repair must suggest the triple delimiter"
    );
    let module = parse("x = '''abc\n", "mlr").expect("unterminated ml must heal");
    let d = module
        .healing
        .iter()
        .find(|d| d.code == "H001")
        .expect("H001 expected");
    let suggested = d
        .repair
        .as_ref()
        .expect("repair expected")
        .suggested
        .clone();
    assert_eq!(suggested, "'''", "repair must suggest the triple delimiter");
}

#[test]
fn interpolation_error_points_at_true_source_line() {
    // The bad expression sits on source line 3 inside the multi-line
    // string; the diagnostic must name line 3 AND the true column of the
    // `!` operator (column 11), not a stripped offset.
    // (Type errors surface at evaluation, with spans via node_spans.)
    let src2 = "[a]\nb = \"\"\"\n  hello ${!5}\n  \"\"\"\n";
    let module = parse(src2, "mlr").expect("must parse");
    let err = Engine::new().evaluate(&module).expect_err("!5 must fail");
    assert_eq!(err.code(), "T001");
    let span = err.span().expect("T001 must carry a span");
    assert_eq!(
        span.line, 3,
        "error must point at source line 3, got {span:?}"
    );
    assert_eq!(
        span.column, 11,
        "error must point at the `!` operator, got {span:?}"
    );
}

#[test]
fn interpolation_error_on_later_body_line_maps_correctly() {
    // A second interpolation on source line 4 must map to line 4 with its
    // own column — not to the first interpolation's line.
    let src = "[a]\nb = \"\"\"\n  ok ${1}\n  bad ${!5}\n  \"\"\"\n";
    let module = parse(src, "mlr").expect("must parse");
    let err = Engine::new().evaluate(&module).expect_err("!5 must fail");
    assert_eq!(err.code(), "T001");
    let span = err.span().expect("T001 must carry a span");
    assert_eq!(
        span.line, 4,
        "error must point at source line 4, got {span:?}"
    );
    assert_eq!(
        span.column, 9,
        "error must point at the `!` operator, got {span:?}"
    );
}

#[test]
fn bare_semantic_statement_keeps_its_comments() {
    let src = "# lead prompt\n@prompt \"\"\"\n  Be helpful.\n  \"\"\"\n# after\nx = 1\n";
    let module = parse(src, "mlr").expect("must parse");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    let lead = out.find("# lead prompt").expect("leading comment survives");
    let tag = out.find("@prompt").expect("tag survives");
    assert!(
        lead < tag,
        "leading comment stays above the statement: {out}"
    );
    assert!(out.contains("# after"), "trailing comment survives: {out}");
}

#[test]
fn semantic_block_has_no_duplicate_edges() {
    use std::collections::HashSet;
    let module = parse("[a]\nb = @prompt \"\"\"\n  hi\n  \"\"\"\n", "mlr").expect("must parse");
    let mut seen = HashSet::new();
    for e in module.graph.edge_indices() {
        let (a, b) = module.graph.edge_endpoints(e).expect("edge endpoints");
        assert!(
            seen.insert((a, b)),
            "duplicate dependency edge for semantic block: {a:?} -> {b:?}"
        );
    }
}

#[test]
fn non_ascii_padding_before_expression_maps_columns_by_chars() {
    // An em space (1 char, 3 bytes) pads the interpolation: the column
    // must count characters, so `!` is at column 7, not higher.
    let src = "[a]\nb = \"\"\"\n  x ${\u{2003}!5}\n  \"\"\"\n";
    let module = parse(src, "mlr").expect("must parse");
    let err = Engine::new().evaluate(&module).expect_err("!5 must fail");
    assert_eq!(err.code(), "T001");
    let span = err.span().expect("T001 must carry a span");
    assert_eq!(span.line, 3, "got {span:?}");
    assert_eq!(span.column, 8, "column counts characters, got {span:?}");
}

#[test]
fn nested_escape_inside_nested_string_maps_exactly() {
    // Triple-quoted outer string; its content line holds an interpolation
    // with an inner ordinary string containing a `\n` escape (backslash +
    // n: two source columns decoding to one newline). Source line 3:
    // `  x ${"ab\n${!5}"}` → `!` at column 14.
    let src = "[a]\nv = \"\"\"\n  x ${\"ab\\n${!5}\"}\n  \"\"\"\n";
    let module = parse(src, "mlr").expect("must parse");
    let err = Engine::new().evaluate(&module).expect_err("!5 must fail");
    assert_eq!(err.code(), "T001");
    let span = err.span().expect("T001 must carry a span");
    assert_eq!(span.line, 3, "got {span:?}");
    assert_eq!(span.column, 14, "got {span:?}");
}

#[test]
fn nested_interpolation_error_maps_through_both_frames() {
    // Inner `${!5}` sits inside an inner string on source line 3: the
    // error must name line 3 and the inner `!` column, not an
    // expression-local position.
    let src = "[a]\nb = \"\"\"\n  outer ${\"in ${!5}\"}\n  \"\"\"\n";
    let module = parse(src, "mlr").expect("must parse");
    let err = Engine::new().evaluate(&module).expect_err("!5 must fail");
    assert_eq!(err.code(), "T001");
    let span = err.span().expect("T001 must carry a span");
    assert_eq!(
        span.line, 3,
        "nested error must name source line 3, got {span:?}"
    );
    assert_eq!(
        span.column, 17,
        "nested error must point at the inner `!`, got {span:?}"
    );
}

#[test]
fn escaped_newline_does_not_shift_source_lines() {
    // `a\n${!5}` sits entirely on source line 1: the `\n` escape decodes
    // to a newline without advancing the physical source line, and `!`
    // sits at source column 13.
    let src = "x = \"\"\"a\\n${!5}\"\"\"\n";
    let module = parse(src, "mlr").expect("must parse");
    let err = Engine::new().evaluate(&module).expect_err("!5 must fail");
    assert_eq!(err.code(), "T001");
    let span = err.span().expect("T001 must carry a span");
    assert_eq!(
        span.line, 1,
        "decoded newline must not shift lines, got {span:?}"
    );
    assert_eq!(
        span.column, 13,
        "decoded newline must not shift columns either, got {span:?}"
    );
}

#[test]
fn unterminated_interpolation_maps_to_source_line() {
    // The `${` opener sits on source line 3; E012 must say line 3.
    let src = "[a]\nb = \"\"\"\n  hello ${\n  \"\"\"\n";
    let diags = parse(src, "mlr").expect_err("unterminated interpolation must fail");
    let d = diags
        .iter()
        .find(|d| d.code == "E012")
        .expect("E012 expected");
    let span = d.span.as_ref().expect("E012 must carry a span");
    assert_eq!(span.line, 3, "E012 must name source line 3, got {span:?}");
}
