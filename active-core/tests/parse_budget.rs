//! Parse-phase budget tests (LANG-C-003).
//!
//! A parse must be bounded before evaluation starts: input bytes, arena
//! nodes, and wall clock. Each bound fails closed with one E029, and each
//! test pairs the failure with a negative control that disables the same
//! bound and watches the document parse, so the test proves the bound is
//! doing the work rather than passing by accident.

use active_core::parser::{
    parse_file, parse_recovering, parse_wall_budget_ms, parse_with, ParseOptions,
    DEFAULT_MAX_INPUT_BYTES, DEFAULT_MAX_PARSE_NODES, DEFAULT_MAX_PARSE_WALL_MS,
};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("includes")
        .join(name)
}

/// A document of exactly `bytes` bytes: one assignment plus a comment pad.
fn document_of_exactly(bytes: usize) -> String {
    let head = "x = 1\n#";
    assert!(bytes > head.len(), "requested size too small");
    let mut s = String::from(head);
    s.push_str(&"a".repeat(bytes - head.len()));
    assert_eq!(s.len(), bytes);
    s
}

fn codes(diags: &[active_core::diag::Diagnostic]) -> Vec<&str> {
    diags.iter().map(|d| d.code.as_str()).collect()
}

/// Options with the input cap lifted: the other bounds keep their
/// shipped values unless a test overrides them.
fn caps_off() -> ParseOptions {
    // Zero means the shipped default (LANG-C-015); disabling is explicit.
    ParseOptions {
        max_input_bytes: active_core::parser::UNLIMITED_PARSE_BYTES,
        ..ParseOptions::default()
    }
}

#[test]
fn shipped_defaults_are_the_documented_parse_bounds() {
    let options = ParseOptions::default();
    assert_eq!(options.max_input_bytes, DEFAULT_MAX_INPUT_BYTES);
    assert_eq!(options.max_wall_ms, DEFAULT_MAX_PARSE_WALL_MS);
    assert_eq!(options.max_nodes, DEFAULT_MAX_PARSE_NODES);
    assert_eq!(DEFAULT_MAX_INPUT_BYTES, 4 * 1024 * 1024);
    assert_eq!(DEFAULT_MAX_PARSE_WALL_MS, 100);
    assert_eq!(DEFAULT_MAX_PARSE_NODES, 1_000_000);
    assert_eq!(
        ParseOptions::strict().max_input_bytes,
        DEFAULT_MAX_INPUT_BYTES
    );
    assert_eq!(
        ParseOptions::resilient_mode().max_input_bytes,
        DEFAULT_MAX_INPUT_BYTES
    );
}

#[test]
fn wall_budget_scales_with_input_size_and_zero_disables_it() {
    // 0 base disables the clock entirely.
    assert_eq!(parse_wall_budget_ms(0, 4 * 1024 * 1024), 0);
    // Sub-KiB input still pays for one KiB of allowance (div_ceil).
    assert_eq!(parse_wall_budget_ms(100, 1), 130);
    assert_eq!(parse_wall_budget_ms(100, 1024), 130);
    // Exact scaling at the shipped default base.
    assert_eq!(parse_wall_budget_ms(100, 1024 * 1024), 100 + 30 * 1024);
    assert_eq!(parse_wall_budget_ms(100, 4 * 1024 * 1024), 100 + 30 * 4096);
    // Saturating: absurd sizes cannot overflow into a zero (disabled) budget.
    assert!(parse_wall_budget_ms(100, usize::MAX) > 0);
}

#[test]
fn input_size_cap_is_exact_at_the_boundary_and_fails_with_one_e029() {
    let exact = document_of_exactly(4096);
    // At the cap: parses.
    let at_cap = ParseOptions {
        max_input_bytes: exact.len(),
        ..ParseOptions::default()
    };
    let module = parse_with(&exact, "at-cap", at_cap).expect("a document at the cap parses");
    assert!(module.root.is_some());

    // One byte over: one E029, no phantom syntax errors from a truncated
    // token stream, and no parsed tree.
    let over_cap = ParseOptions {
        max_input_bytes: exact.len() - 1,
        ..ParseOptions::default()
    };
    let (module, diags) = parse_recovering(&exact, "over-cap", over_cap);
    assert_eq!(codes(&diags), ["E029"], "exactly one budget diagnostic");
    assert!(
        diags[0].message.contains("4096 bytes") && diags[0].message.contains("the limit is 4095"),
        "message names both sizes: {}",
        diags[0].message
    );
    assert!(module.root.is_none(), "no tree survives a budget exit");

    // Negative control: the same document parses with the cap disabled.
    assert!(parse_with(&exact, "unbounded", caps_off()).is_ok());
}

#[test]
fn node_cap_fails_closed_and_is_load_independent() {
    let mut src = String::new();
    for i in 0..2_000 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let capped = ParseOptions {
        max_nodes: 500,
        ..ParseOptions::default()
    };
    let err = parse_with(&src, "node-cap", capped.clone()).expect_err("node cap must trip");
    assert_eq!(codes(&err), ["E029"], "exactly one diagnostic: {err:?}");
    assert!(
        err[0].message.contains("node limit 500"),
        "message names the cap: {}",
        err[0].message
    );

    // Determinism: the same document trips at the same point twice.
    let again = parse_with(&src, "node-cap", capped).expect_err("node cap must trip again");
    assert_eq!(err[0].message, again[0].message);

    // Negative control: the explicit sentinel disables the cap and the
    // document parses.
    let unbounded = ParseOptions {
        max_nodes: active_core::parser::UNLIMITED_PARSE_NODES,
        ..ParseOptions::default()
    };
    let module = parse_with(&src, "node-cap-off", unbounded).expect("cap disabled: parses");
    assert!(module.arena.len() > 500, "control really exceeds the cap");
}

#[test]
fn the_lexer_trips_on_an_expired_deadline_and_reports_a_prefix_free_stop() {
    // The clock is a backstop against superlinear work: with the per-KiB
    // allowance, a linear parse of a legal document never trips it (by
    // construction - measured worst case 18.6 ms/KiB, allowance 30). This
    // drives the sampling mechanism itself with a deadline that has already
    // passed, which is the only deterministic way to observe a trip.
    let mut src = String::new();
    for i in 0..5_000 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let mut lexer = active_core::parser::Lexer::new(&src);
    lexer.deadline = Some(std::time::Instant::now());
    let lexed = lexer.tokenize();
    assert!(lexed.budget_exceeded, "an expired deadline must trip");
    assert!(
        lexed.tokens.is_empty(),
        "a tripped lex must not hand a truncated token stream to the parser"
    );

    // Control: the same source with the clock off tokenizes fully.
    let mut lexer = active_core::parser::Lexer::new(&src);
    lexer.deadline = None;
    let lexed = lexer.tokenize();
    assert!(!lexed.budget_exceeded);
    assert!(lexed.tokens.len() > 5_000);
}

#[test]
fn include_bytes_extend_the_wall_budget_they_consume() {
    // The budget scales with the bytes charged, and include sources are
    // charged as they are read: an include-heavy document is legitimate
    // work, not a pathological shape. With a 1 ms base the root alone would
    // allow ~1 ms; the included 600 KB earn their own allowance and the
    // document parses. (Before the bytes were charged, an unrelated
    // include fixture failed intermittently under parallel test load.)
    let loader: active_core::parser::IncludeLoader = std::sync::Arc::new(|_path: &str| {
        let mut body = String::new();
        for i in 0..750 {
            body.push_str(&format!("i{i} = {i}\n"));
        }
        Ok(body)
    });
    let mut root = String::new();
    for i in 0..60 {
        root.push_str(&format!("include \"inc{i}.active\"\n"));
    }
    let options = ParseOptions {
        enable_includes: true,
        include_loader: Some(loader),
        max_wall_ms: 1,
        ..ParseOptions::default()
    };
    let module = parse_with(&root, "include-budget", options)
        .expect("charged include bytes earn their own wall allowance");
    assert!(module.arena.len() > 40_000, "includes really merged");
}

#[test]
fn a_large_document_below_the_default_bounds_still_parses() {
    // 10k keys (about 160 KB) is the size the research battery exercises;
    // it must stay inside the shipped defaults on an unoptimized build.
    let mut src = String::new();
    for i in 0..10_000 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let module = parse_with(&src, "ten-thousand", ParseOptions::default())
        .expect("10k-key document stays inside the default parse bounds");
    assert!(module.arena.len() > 10_000);
}

#[test]
fn parse_file_rejects_an_oversized_file_from_metadata_before_reading() {
    let path = fixture("lib_base.active");
    let len = std::fs::metadata(&path).expect("fixture exists").len();
    assert!(len > 10, "fixture is larger than the test cap");
    let capped = ParseOptions {
        max_input_bytes: 10,
        ..ParseOptions::default()
    };
    let err = parse_file(&path, capped).expect_err("oversized file must be rejected");
    assert_eq!(codes(&err), ["E029"], "exactly one diagnostic: {err:?}");
    assert!(
        err[0].message.contains(&format!("{len} bytes")),
        "message uses the file's metadata length: {}",
        err[0].message
    );
    assert_eq!(
        err[0].file.as_deref(),
        Some(path.to_string_lossy().as_ref())
    );

    // Negative control: same file, no cap.
    assert!(parse_file(&path, caps_off()).is_ok());
}

#[test]
fn parse_options_zero_means_default_and_sentinels_disable() {
    // LANG-C-015's one-meaning rule extends to the parse bounds (review
    // finding): a zero field is the shipped default; disabling takes the
    // explicit sentinel or ParseOptions::unlimited().
    use active_core::parser::{
        ParseOptions, UNLIMITED_PARSE_BYTES, UNLIMITED_PARSE_NODES, UNLIMITED_PARSE_WALL_MS,
    };
    let zeroed = ParseOptions {
        max_input_bytes: 0,
        max_wall_ms: 0,
        max_nodes: 0,
        ..ParseOptions::default()
    };
    let resolved = zeroed.clone().resolve();
    assert_eq!(resolved.max_input_bytes, DEFAULT_MAX_INPUT_BYTES);
    assert_eq!(resolved.max_wall_ms, DEFAULT_MAX_PARSE_WALL_MS);
    assert_eq!(resolved.max_nodes, DEFAULT_MAX_PARSE_NODES);

    let lifted = ParseOptions::unlimited();
    assert_eq!(lifted.max_input_bytes, UNLIMITED_PARSE_BYTES);
    assert_eq!(lifted.max_wall_ms, UNLIMITED_PARSE_WALL_MS);
    assert_eq!(lifted.max_nodes, UNLIMITED_PARSE_NODES);

    // End to end: a zeroed ParseOptions still enforces the shipped caps, so
    // an oversized document is refused rather than silently accepted.
    // A valid document one byte over the default cap: the padding is a
    // comment, so the lifted parse is cheap (no tokens, no nodes).
    let big = format!("x = 1\n#{}", "a".repeat(DEFAULT_MAX_INPUT_BYTES));
    let err = parse_with(&big, "zeroed", zeroed).expect_err("zero keeps the cap");
    assert_eq!(codes(&err), ["E029"]);
    assert!(parse_with(&big, "lifted", lifted).is_ok());
}
