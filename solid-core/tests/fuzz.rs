//! Fuzz harness (C-012).
//!
//! Feeds arbitrary byte streams to the lexer and parser; the only
//! required assertion is that the process never panics and always
//! terminates. This is the in-repo equivalent of the cargo-fuzz targets
//! under `fuzz/` (which drive the same entry points with libFuzzer on a
//! nightly toolchain with clang available). It runs under `cargo test`
//! on every CI job, so the crash-based attack surface stays closed
//! without special tooling.

use proptest::prelude::*;
use solid_core::parser::{parse, Lexer, TokenKind};
use solid_core::Engine;

fn source_from_bytes(bytes: &[u8]) -> String {
    // Lossy conversion is deliberate: invalid UTF-8 must not panic.
    String::from_utf8_lossy(bytes).into_owned()
}

proptest! {
    /// Arbitrary bytes through the parser must never panic, must always
    /// terminate, and must produce either a module or diagnostics.
    #[test]
    fn parser_never_panics_on_arbitrary_bytes(
        bytes in prop::collection::vec(any::<u8>(), 0..1024)
    ) {
        let src = source_from_bytes(&bytes);
        if let Ok(module) = parse(&src, "fuzz") {
            // A successfully parsed module must also evaluate without
            // panicking; cycles and type errors are legitimate errors.
            let engine = Engine::new();
            let _ = engine.evaluate(&module);
        }
    }

    /// Arbitrary bytes through the lexer must never panic, must always
    /// terminate, and must always produce a finite token stream ending
    /// with Eof.
    #[test]
    fn lexer_never_panics_on_arbitrary_bytes(
        bytes in prop::collection::vec(any::<u8>(), 0..1024)
    ) {
        let src = source_from_bytes(&bytes);
        let lexed = Lexer::new(&src).tokenize();
        assert!(
            matches!(lexed.tokens.last().map(|t| &t.kind), Some(TokenKind::Eof)),
            "token stream must always end with Eof"
        );
    }

    /// Interpolation-heavy inputs must not panic either.
    #[test]
    fn interpolation_never_panics(
        bytes in prop::collection::vec(any::<u8>(), 0..256)
    ) {
        let src = source_from_bytes(&bytes);
        let with_interp = format!("let x = \"${{{src}}}\"\ny = x\n");
        if let Ok(module) = parse(&with_interp, "fuzz-interp") {
            let _ = Engine::new().evaluate(&module);
        }
    }
}

/// A fixed deep-nesting input must fail with the depth guard, not a stack
/// overflow. (The stack-overflow class of bug is exactly what the fuzz
/// harness protects against.)
#[test]
fn deep_nesting_fails_with_depth_guard_not_crash() {
    let deep = format!("x = {}1{}", "(".repeat(200), ")".repeat(200));
    let result = parse(&deep, "deep");
    assert!(
        result.is_err(),
        "200 nested parentheses must exceed the depth guard and fail"
    );
    // Also verify a legal depth still parses.
    let legal = format!("x = {}1{}", "(".repeat(10), ")".repeat(10));
    assert!(
        parse(&legal, "legal-depth").is_ok(),
        "10 nested parentheses are within the depth limit"
    );
}
