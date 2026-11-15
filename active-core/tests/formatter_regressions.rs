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
