//! libFuzzer target: the formatter must never panic, its output must
//! re-parse, and formatting must be idempotent (spec/formatter.md
//! section 2). Both shipped block styles are exercised.

#![no_main]

use active_core::{fmt, parse, Style};
use libfuzzer_sys::fuzz_target;

fn check(src: &str, style: Style) {
    let Ok(module) = parse(src, "fuzz-fmt") else {
        return;
    };
    let once = fmt::format(&module, style);
    let Ok(reparsed) = parse(&once, "fuzz-fmt") else {
        panic!("formatted output must re-parse ({style:?}):\n{once}");
    };
    let twice = fmt::format(&reparsed, style);
    assert_eq!(once, twice, "formatter is not idempotent ({style:?})");
}

fuzz_target!(|data: &[u8]| {
    // Lossy conversion is deliberate: invalid UTF-8 must not panic.
    let src = String::from_utf8_lossy(data);
    check(&src, Style::Brace);
    check(&src, Style::Indent);
});
