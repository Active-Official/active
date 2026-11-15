//! Formatter crash regression (libFuzzer, formatter target, 2026-11-15).
//!
//! The fuzzer found an input whose formatted output did not re-parse
//! (an unnamed `Ref` rendered as `<?>`) and, once that was fixed, whose
//! formatting was not idempotent (a reference to a table rendered as a
//! scalar expression on the first pass and as a block on the second).
//! Both must hold for every parseable input: output re-parses and
//! formatting reaches a fixed point.

use active_core::{fmt, parse, Style};

const CRASH: &str = "ye=o[o]yye=o[o]ye=o#[o]=e=o[o]=?v";

fn check(src: &str, style: Style) {
    let module = parse(src, "fmt-crash").expect("input parses");
    let once = fmt::format(&module, style);
    assert!(
        !once.contains("<?>"),
        "no unparseable placeholder may be emitted ({style:?}):\n{once}"
    );
    let reparsed = parse(&once, "fmt-crash")
        .unwrap_or_else(|d| panic!("formatted output must re-parse ({style:?}):\n{once}\n{d:?}"));
    let twice = fmt::format(&reparsed, style);
    assert_eq!(once, twice, "formatter must be idempotent ({style:?})");
}

#[test]
fn the_fuzz_crash_input_round_trips_in_both_styles() {
    check(CRASH, Style::Brace);
    check(CRASH, Style::Indent);
}

#[test]
fn references_to_tables_render_as_blocks() {
    // The minimal shape behind the crash: a key whose value is a
    // reference to a table.
    let src = "t = { a = 1 }\nr = t\n";
    let module = parse(src, "fmt-crash").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-crash").expect("re-parse");
        assert_eq!(once, fmt::format(&reparsed, style), "{style:?}:\n{once}");
    }
}

#[test]
fn an_aliased_empty_table_renders_inline_and_stays_idempotent() {
    // Fuzz seed crash-f438d19: `ye` resolves to an EMPTY table through a
    // Ref. The empty-vs-block decision read the Ref node (not a table), so
    // pass one emitted `ye {\n}`; that re-parses as an empty table node
    // whose inline form is `ye = {}`, so pass two differed.
    let src = "e=o[o]+++++[o]ye=o[oo]";
    let module = parse(src, "fmt-alias-empty").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-alias-empty").expect("re-parse");
        let twice = fmt::format(&reparsed, style);
        assert_eq!(once, twice, "{style:?} not idempotent:\n{once}");
        assert!(
            !once.contains("ye {\n}"),
            "{style:?} must emit the inline empty form: {once}"
        );
    }

    // The same shape by hand: an alias to an empty table.
    let src = "empty = {}\nalias = empty\n";
    let module = parse(src, "fmt-alias-empty-2").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-alias-empty-2").expect("re-parse");
        assert_eq!(once, fmt::format(&reparsed, style), "{style:?}:\n{once}");
    }
}

#[test]
fn a_parent_trailing_comment_hoists_into_an_aliased_last_table() {
    // Fuzz seed crash-8e989b62: a block's trailing comment must land inside
    // the nested block the parser will re-attach it to. The last entry was
    // an ALIAS to a table, so the hoist (which looked at the raw node) did
    // not fire, the comment stayed after the closing brace, and the next
    // parse moved it inside - a non-idempotent rewrite.
    let src = "ye=o[o]yye=o[o]ye=o\n\n\n\n#[o}=e\n\n=\n";
    let module = parse(src, "fmt-hoist").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-hoist").expect("re-parse");
        let twice = fmt::format(&reparsed, style);
        assert_eq!(once, twice, "{style:?} not idempotent:\n{once}");
    }

    // The minimal hand-written shape: a block whose last entry aliases a
    // table, with the block's trailing comment after it.
    let src = "[a]\nx = 1\n[b]\nc = { d = 1 }\ne = c\n# trailing\n";
    let module = parse(src, "fmt-hoist-2").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-hoist-2").expect("re-parse");
        assert_eq!(once, fmt::format(&reparsed, style), "{style:?}:\n{once}");
    }
}
