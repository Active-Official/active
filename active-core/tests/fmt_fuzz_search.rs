//! Deterministic random search for formatter failures (manual).
//!
//! Mirrors the libFuzzer target exactly - arbitrary bytes, lossy UTF-8,
//! parse, format in both styles, re-parse, idempotence - but runs from a
//! fixed seed under `cargo test --release`, so a failure is reproducible
//! without the sanitizer runtime. The libFuzzer job in CI remains the
//! authority; this is the local convergence loop.
//!
//! Run with: `cargo test --release -p active-core --test fmt_fuzz_search -- --ignored --nocapture`

use active_core::{fmt, parse, Style};

fn check(src: &str) -> Result<(), String> {
    let Ok(module) = parse(src, "search") else {
        return Ok(());
    };
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let Ok(reparsed) = parse(&once, "search") else {
            return Err(format!(
                "{style:?} output does not re-parse: {once:?} from {src:?}"
            ));
        };
        let twice = fmt::format(&reparsed, style);
        if once != twice {
            return Err(format!(
                "{style:?} not idempotent: once={once:?} twice={twice:?} from {src:?}"
            ));
        }
    }
    Ok(())
}

/// A small grammar-aware generator plus byte mutations: pure random bytes
/// are almost all rejected by the parser, so the useful cases come from
/// tokens the formatter must place (headers, blocks, comments, aliases).
fn generate(seed: &mut u64) -> String {
    fn next(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state
    }
    const PIECES: &[&str] = &[
        "a",
        "b",
        "x",
        "y",
        "ye",
        "k",
        "=",
        " = ",
        "[",
        "]",
        "{",
        "}",
        "#",
        "# c",
        "\n",
        "\n\n",
        " ",
        "  ",
        ", ",
        ":",
        "\"s\"",
        "1",
        "+",
        "schema",
        "t",
        "let ",
        "@patch(\"a\")\n",
        "@remove(\"a\")\n",
        "profile \"p\"\n",
        "include \"i\"\n",
        "env.E",
        "secret(\"s\")",
        "blob(\"text/plain\", \"hi\")",
        "Array",
        "Int",
        "Int(min = 1)",
        "Table",
        "depends_on = [\"a\"]",
        "active_mode = \"data\"\n",
    ];
    let mut out = String::new();
    let count = 1 + next(seed) % 14;
    for _ in 0..count {
        let piece = PIECES[(next(seed) as usize) % PIECES.len()];
        out.push_str(piece);
    }
    out
}

#[test]
#[ignore = "manual random search; run with --release --ignored"]
fn formatter_random_search_is_idempotent() {
    let iterations: u64 = std::env::var("FMT_SEARCH_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200_000);
    let mut seed = std::env::var("FMT_SEARCH_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5eed_1234_abcd_0001u64);
    let mut failures = 0usize;
    for i in 0..iterations {
        let src = generate(&mut seed);
        if let Err(problem) = check(&src) {
            println!("FAIL at {i}: {problem}");
            failures += 1;
            if failures >= 10 {
                panic!("stopping after {failures} failures");
            }
        }
    }
    assert_eq!(failures, 0, "formatter failures found");
}
