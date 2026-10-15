//! Full-expression interpolation tests (C-023).
//!
//! Proves on the real path that `${ … }` holds a complete expression:
//! arithmetic, comparisons, env defaults, nested interpolation, and escaped
//! dollars; that each sub-expression participates in the dependency graph;
//! and that the new diagnostics are precise.

use solid_core::{parse, Engine};

fn json(src: &str) -> String {
    let module = parse(src, "interp").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    let engine = Engine::new();
    let (value, diags) = engine
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("evaluation failed: {e}"));
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    solid_core::convert::value_to_pretty_json(&value)
}

fn codes(src: &str) -> Vec<String> {
    match parse(src, "interp") {
        Ok(module) => match Engine::new().evaluate(&module) {
            Ok((_, diags)) => diags.into_iter().map(|d| d.code).collect(),
            Err(e) => vec![e.code().to_string()],
        },
        Err(diags) => diags.into_iter().map(|d| d.code).collect(),
    }
}

#[test]
fn arithmetic_inside_interpolation() {
    let out = json("[calc]\nbase = 10\ndoubled = \"value: ${base * 2}\"\n");
    assert!(out.contains("\"value: 20\""), "{out}");
}

#[test]
fn comparisons_and_booleans_inside_interpolation() {
    let out = json("[t]\na = 5\nb = 3\nm = \"sum=${a + b} gt=${a > b} both=${a > 0 && b > 0}\"\n");
    assert!(out.contains("\"sum=8 gt=true both=true\""), "{out}");
}

#[test]
fn env_default_inside_interpolation() {
    let out = json("[t]\ns = \"host=${env.SOLID_INTERP_UNSET ?: 'local'}\"\n");
    assert!(out.contains("\"host=local\""), "{out}");
}

#[test]
fn nested_interpolation() {
    // A double-quoted string inside an interpolation may itself interpolate;
    // the inner quotes are escaped in the source.
    let out = json("[t]\nname = \"x\"\ns = \"outer ${ \\\"inner ${name}\\\" }\"\n");
    assert!(out.contains("\"outer inner x\""), "{out}");
}

#[test]
fn single_quoted_strings_do_not_interpolate() {
    let out = json("[t]\nname = \"x\"\ns = 'literal ${name}'\n");
    assert!(out.contains("\"literal ${name}\""), "{out}");
}

#[test]
fn adjacent_interpolations_and_literals() {
    let out = json("[t]\nx = 1\ny = 2\ns = \"a${x}b${y}c\"\n");
    assert!(out.contains("\"a1b2c\""), "{out}");
}

#[test]
fn escaped_dollar_is_literal() {
    let out = json("[t]\ns = \"cost is \\${5}\"\n");
    assert!(out.contains("\"cost is ${5}\""), "{out}");
}

#[test]
fn secret_in_interpolation_is_refused() {
    let codes = codes("let p = secret('k')\n[t]\nmsg = \"a${p}\"\n");
    assert_eq!(codes, vec!["S004"], "secret interpolation must be S004");
}

#[test]
fn cycle_through_an_interpolation_is_detected() {
    // `a` interpolates `b`, `b` interpolates `a`: the dependency graph must
    // see the cycle through the interpolation sub-expressions.
    let codes = codes("let a = \"x${b}\"\nlet b = \"y${a}\"\n[t]\nv = a\n");
    assert_eq!(codes, vec!["S001"], "cycle through interpolation");
}

#[test]
fn unterminated_interpolation_is_e012() {
    let diags = codes("[t]\nbad = \"oops ${1\"\n");
    assert!(diags.contains(&"E012".to_string()), "{diags:?}");
}

#[test]
fn empty_interpolation_is_e013() {
    let diags = codes("[t]\nbad = \"oops ${}\"\n");
    assert!(diags.contains(&"E013".to_string()), "{diags:?}");
}

#[test]
fn interpolation_with_units() {
    let out = json("let t = 5s\n[x]\nmsg = \"timeout ${t}\"\n");
    assert!(out.contains("\"timeout 5000ms\""), "{out}");
}

#[test]
fn interpolation_span_is_reported_for_broken_expressions() {
    // The sub-expression error must point into the string, not at line 1.
    let src = "[t]\nbad = \"start ${1 +} end\"\n";
    match parse(src, "interp") {
        Err(diags) => {
            let d = diags
                .iter()
                .find(|d| d.code.starts_with('E'))
                .expect("an error diagnostic");
            let span = d.span.as_ref().expect("span");
            assert_eq!(span.line, 2, "span must be on the string's line: {span:?}");
            assert!(
                span.column > 9,
                "span must point inside the string, got column {}",
                span.column
            );
        }
        Ok(_) => panic!("broken embedded expression must fail"),
    }
}

#[test]
fn backslash_parity_is_preserved() {
    // `\\` is a literal backslash, so `${x}` after it still interpolates.
    let out = json("[t]\nx = 7\ns = \"a\\\\${x}b\"\n");
    assert!(
        out.contains("\"a\\\\7b\""),
        "backslash + interpolation: {out}"
    );
    // `\$` is a literal dollar: no interpolation.
    let out2 = json("[t]\nx = 7\ns = \"a\\${x}b\"\n");
    assert!(
        out2.contains("\"a${x}b\""),
        "escaped dollar is literal: {out2}"
    );
}

#[test]
fn inline_table_inside_interpolation_is_a_type_error() {
    // The brace scanner must not mistake the table's `}` for the
    // interpolation's own, and a structured value has no string form.
    let diags = codes("[t]\ns = \"${{k = 1}}\"\n");
    assert_eq!(
        diags,
        vec!["T001"],
        "interpolating a table must be a type error, never a placeholder"
    );
}

#[test]
fn leftover_tokens_after_the_expression_are_diagnosed() {
    let diags = codes("[t]\nbad = \"${1 2}\"\n");
    assert!(
        diags.iter().any(|c| c == "E002"),
        "trailing tokens must be diagnosed: {diags:?}"
    );
}

#[test]
fn sub_expression_lexical_errors_are_preserved() {
    // `@` is not a lexical rule the sub-lexer accepts: E005 must survive.
    let diags = codes("[t]\nbad = \"${1 @ 2}\"\n");
    assert!(
        diags.iter().any(|c| c == "E005" || c == "E002"),
        "sub-lexer diagnostics must be preserved: {diags:?}"
    );
}

#[test]
fn repeated_interpolation_does_not_blow_up_the_scope() {
    // Each `${a}` must transfer only its own identifiers; with the previous
    // copy-back the scope grew per segment, making this quadratic.
    let mut src = String::from("[t]\na = 1\ns = \"");
    for _ in 0..4000 {
        src.push_str("${a}");
    }
    src.push_str("\"\n");
    let module = parse(&src, "interp").expect("must parse");
    let engine = Engine::new();
    let (value, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let out = solid_core::convert::value_to_pretty_json(&value);
    assert!(
        out.contains("1111"),
        "each segment interpolates: {}",
        &out[..80.min(out.len())]
    );
}

#[test]
fn single_quoted_values_with_dollar_and_newline_round_trip() {
    use solid_core::{fmt::format, Style};
    let src = "[t]\ns = 'a ${b}\nc'\n";
    let module = parse(src, "interp").expect("must parse");
    let formatted = format(&module, Style::Brace);
    let reparsed = parse(&formatted, "interp")
        .unwrap_or_else(|d| panic!("formatted output must re-parse: {d:?}\n{formatted}"));
    let engine = Engine::new();
    let (a, _) = engine.evaluate(&module).expect("original evaluates");
    let (b, _) = engine.evaluate(&reparsed).expect("formatted evaluates");
    assert_eq!(
        solid_core::convert::value_to_pretty_json(&a),
        solid_core::convert::value_to_pretty_json(&b),
        "value must survive formatting:\n{formatted}"
    );
}
