//! Replay one formatter input the way libFuzzer does (lossy UTF-8, both
//! styles), printing the trivia and any idempotence failure.
//!
//! ```text
//! FMT_SEARCH_INPUT='a = 1' cargo run -p active-core --example fmt_replay
//! ```
//!
//! This lives outside the test target so the normal suite never carries a
//! manual-only case.

use active_core::{fmt, parse, Style};

fn main() {
    let src = std::env::var("FMT_SEARCH_INPUT").expect("set FMT_SEARCH_INPUT");
    let Ok(module) = parse(&src, "probe") else {
        println!("input does not parse");
        return;
    };
    for c in &module.trivia {
        println!(
            "input trivia {:?} @{} -> {:?}",
            c.text, c.span.line, c.attachment
        );
    }
    let mut failure = None;
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        println!("{style:?} once: {once:?}");
        match parse(&once, "probe") {
            Ok(p2) => {
                for c in &p2.trivia {
                    println!(
                        "  {style:?} pass2 trivia {:?} @{} -> {:?}",
                        c.text, c.span.line, c.attachment
                    );
                }
                let twice = fmt::format(&p2, style);
                if once != twice {
                    failure = Some(format!(
                        "{style:?} not idempotent: once={once:?} twice={twice:?}"
                    ));
                }
            }
            Err(diags) => {
                failure = Some(format!("{style:?} output does not re-parse: {diags:?}"));
            }
        }
    }
    if let Some(problem) = failure {
        panic!("{problem}");
    }
}
