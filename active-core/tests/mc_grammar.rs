//! Monte Carlo grammar-guided exploration as a permanent CI citizen
//! (INNOVATIONS L-P66; the audit's 2026-11-14 run of this method found
//! LANG-L-001, which no reading pass had found).
//!
//! The generator is deterministic: a fixed seed and a fixed case count
//! reproduce every document. `ACTIVE_MC_SEED` and `ACTIVE_MC_CASES` scale
//! the run locally without changing CI.
//!
//! For every generated document the test drives the shipped surfaces:
//! parse → format (both styles) → re-parse → format again (idempotence),
//! evaluate the original and the formatted source (semantic preservation
//! on the parsed JSON value), and check the redacted export for secret
//! material. A panic, a non-idempotent format, a format that does not
//! re-parse, or a semantic change fails the test with the seed, the case
//! number, and the source in the message.
//!
//! Families mirror the audit run: flat scalars, let/refs, nested tables,
//! deep nesting across the 64-level cap, unicode, secrets/interpolation,
//! arrays/inline tables, profiles/schemas/duplicate keys, arithmetic, and
//! a kitchen sink.

use active_core::secret::SecretRedactor;
use active_core::{parse, Budget, Engine, Module, Style, Value};

/// Tiny deterministic xorshift64* PRNG (test scaffolding only).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    fn pick<'a>(&mut self, items: &'a [&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

fn scalar(rng: &mut Rng) -> String {
    match rng.below(13) {
        0 => format!("{}", rng.below(10_000)),
        1 => format!("-{}", rng.below(10_000)),
        2 => format!("{}.{}", rng.below(1000), rng.below(1000)),
        3 => "true".into(),
        4 => "false".into(),
        5 => "null".into(),
        6 => "42ms".into(),
        7 => "3.5h".into(),
        8 => "2GiB".into(),
        9 => "\"plain text\"".into(),
        10 => "\"a\\\"b\\\\c\"".into(),
        11 => format!("\"v{}\"", rng.below(100)),
        // Scaled and affine units: rendering the canonical dimension on the
        // source amount changes the value (`2Cel` is 275.15 K), so these
        // pin the source-spelling rule in the sweep as well (the fuzzer
        // found it first).
        _ => rng
            .pick(&[
                "2Cel", "-40Fahr", "0K", "2km", "2MHz", "5MB/s", "1kWh", "45deg", "2KiB", "250g",
            ])
            .to_string(),
    }
}

/// Family 0: flat scalars, including shapes the parser must reject.
fn family_flat(rng: &mut Rng) -> String {
    let mut out = String::new();
    for i in 0..(1 + rng.below(8)) {
        if rng.below(10) == 0 {
            // Deliberate rejects: no exponent/hex notation exists.
            out.push_str(&format!("bad{i} = {}e{}\n", rng.below(9), rng.below(9)));
        } else {
            out.push_str(&format!("k{i} = {}\n", scalar(rng)));
        }
    }
    out
}

/// Family 1: `let` bindings and references, including 2-cycles.
fn family_refs(rng: &mut Rng) -> String {
    let mut out = String::new();
    let n = 1 + rng.below(4);
    for i in 0..n {
        out.push_str(&format!("let l{i} = {}\n", scalar(rng)));
    }
    for i in 0..(1 + rng.below(4)) {
        let prev = rng.below(n.max(1));
        out.push_str(&format!("r{i} = l{prev}\n"));
    }
    if rng.below(4) == 0 {
        out.push_str("cyc_a = cyc_b\ncyc_b = cyc_a\n");
    }
    out
}

/// Family 2: nested tables in all three declaration shapes.
fn family_tables(rng: &mut Rng) -> String {
    let mut out = String::new();
    for t in 0..(1 + rng.below(5)) {
        match rng.below(3) {
            0 => out.push_str(&format!("[t{t}]\nk = {}\n", scalar(rng))),
            1 => out.push_str(&format!("t{t} {{\n  k = {}\n}}\n", scalar(rng))),
            _ => out.push_str(&format!("t{t}:\n  k = {}\n", scalar(rng))),
        }
        if rng.below(3) == 0 {
            out.push_str(&format!("[t{t}.nested]\nk = {}\n", scalar(rng)));
        }
    }
    out
}

/// Family 3: deep nesting that straddles the 64-level cap.
fn family_deep(rng: &mut Rng) -> String {
    let depth = 1 + rng.below(72);
    let mut out = String::new();
    let mut path = String::new();
    for d in 0..depth {
        if path.is_empty() {
            path = format!("n{d}");
        } else {
            path.push_str(&format!(".n{d}"));
        }
        out.push_str(&format!("[{path}]\nk = {d}\n"));
    }
    if rng.below(5) == 0 {
        out.push_str(&format!(
            "deep = {}\n",
            "(".repeat(70) + "1" + &")".repeat(70)
        ));
    }
    out
}

/// Family 4: unicode keys and values (emoji, CJK, combining marks).
fn family_unicode(rng: &mut Rng) -> String {
    let keys = ["café", "cafe\u{301}", "日本", "🔑", "naïve"];
    let vals = ["héllo", "世界", "🚀 launch", "e\u{301}"];
    let mut out = String::new();
    for i in 0..(1 + rng.below(5)) {
        let key = keys[rng.below(keys.len() as u64) as usize];
        let val = vals[rng.below(vals.len() as u64) as usize];
        out.push_str(&format!("\"{key}{i}\" = \"{val}\"\n"));
    }
    out
}

/// Family 5: secrets, interpolation, and env fallback.
fn family_secrets(rng: &mut Rng) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "token = secret(\"{}\")\n",
        vec!["abcd1234", "x", "sh-or-very-long-secret"][rng.below(3) as usize]
    ));
    if rng.below(2) == 0 {
        out.push_str("host = \"h\"\nport = 8080\nu = \"${host}:${port}\"\n");
    }
    if rng.below(3) == 0 {
        out.push_str("envv = env.ACTIVE_MC_PROBE\n");
    }
    if rng.below(4) == 0 {
        // A secret in an interpolation is refused by the language.
        out.push_str("bad = \"${token}\"\n");
    }
    out
}

/// Family 6: arrays and inline tables, mixed types.
fn family_containers(rng: &mut Rng) -> String {
    let mut out = String::new();
    out.push_str("arr = [");
    for i in 0..(1 + rng.below(6)) {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&scalar(rng));
    }
    out.push_str("]\n");
    out.push_str(&format!(
        "tbl = {{ a = {}, b = \"{}{}\" }}\n",
        rng.below(100),
        rng.pick(&["x", "y", "z"]),
        rng.below(9)
    ));
    if rng.below(4) == 0 {
        out.push_str(&format!("nested = [{{ v = {} }}]\n", scalar(rng)));
    }
    out
}

/// Family 7: profiles, schemas, and duplicate keys.
fn family_profiles(rng: &mut Rng) -> String {
    let mut out = String::new();
    out.push_str("active_profile = \"dev\"\n");
    out.push_str("port = 8080\n");
    out.push_str("profile \"dev\" {\n  port = 9090\n}\n");
    if rng.below(2) == 0 {
        out.push_str("schema = {\n  port = Int(min = 1, max = 65535)\n}\n");
        out.push_str("port = 8081\n");
    }
    if rng.below(3) == 0 {
        out.push_str("dup = 1\ndup = 2\n");
    }
    out
}

/// Family 8: arithmetic, including overflow-prone magnitudes.
fn family_arithmetic(rng: &mut Rng) -> String {
    let big = "9".repeat(300);
    let mut out = String::new();
    out.push_str(&format!("a = {}\n", rng.below(1000)));
    out.push_str(&format!("b = a * {}\n", 1 + rng.below(100)));
    out.push_str(&format!("c = a + b - {}\n", rng.below(100)));
    out.push_str("d = 1.5 * a\n");
    if rng.below(3) == 0 {
        out.push_str(&format!("huge = {} * {}\n", big, big));
    }
    if rng.below(4) == 0 {
        out.push_str("div0 = 1 / 0\n");
    }
    if rng.below(4) == 0 {
        out.push_str("units = 2ms + 3ms\nmix = 1s + 1\n");
    }
    out
}

/// Family 9: kitchen sink — several features composed.
fn family_kitchen(rng: &mut Rng) -> String {
    format!(
        "{}{}{}{}",
        family_refs(rng),
        family_tables(rng),
        family_containers(rng),
        family_secrets(rng)
    )
}

fn document(family: usize, rng: &mut Rng) -> String {
    match family {
        0 => family_flat(rng),
        1 => family_refs(rng),
        2 => family_tables(rng),
        3 => family_deep(rng),
        4 => family_unicode(rng),
        5 => family_secrets(rng),
        6 => family_containers(rng),
        7 => family_profiles(rng),
        8 => family_arithmetic(rng),
        _ => family_kitchen(rng),
    }
}

fn engine() -> Engine {
    let mut engine = Engine::hermetic();
    engine.budget = Budget::trusted();
    engine.budget.max_wall_ms = active_core::eval::UNLIMITED;
    engine
}

/// Structural secret check, mirroring `fuzz/fuzz_targets/secrets.rs`:
/// strings are exact-match below four bytes and substrings at four or
/// more; blobs and their metadata are swept at every length.
fn assert_no_material(value: &Value, seeds: &SecretRedactor, ctx: &str) {
    match value {
        Value::Secret(text) => assert!(text.is_empty(), "material survived redaction: {ctx}"),
        Value::String(text) => {
            assert!(
                !seeds.contains_exact(text),
                "an exported string is exactly a runtime secret: {ctx}"
            );
            assert!(
                !seeds.contains_substring(text, 4),
                "an exported string embeds a runtime secret: {ctx}"
            );
        }
        Value::Array(items) => {
            for item in items {
                assert_no_material(item, seeds, ctx);
            }
        }
        Value::Table(map) => {
            for item in map.values() {
                assert_no_material(item, seeds, ctx);
            }
        }
        Value::Blob {
            media_type,
            payload,
            ..
        } => {
            assert!(
                !seeds.contains_substring(media_type, 1),
                "an exported blob media type embeds a runtime secret: {ctx}"
            );
            if let Some(bytes) = payload {
                assert!(
                    !seeds.contains_substring(&String::from_utf8_lossy(bytes), 1),
                    "an exported blob payload embeds a runtime secret: {ctx}"
                );
            }
        }
        _ => {}
    }
}

fn check_document(family: usize, case: usize, src: &str) {
    let ctx = format!("family {family}, case {case}\nsource:\n{src}");
    let Ok(module) = parse(src, "mc") else {
        // Rejected documents are a legitimate outcome; the sweep is about
        // crashes and invariants, not about accepting everything.
        return;
    };
    for (label, style) in [("indent", Style::Indent), ("brace", Style::Brace)] {
        let formatted = active_core::fmt::format(&module, style);
        let reparsed: Module = parse(&formatted, "mc-fmt").unwrap_or_else(|d| {
            panic!("{label} output must re-parse ({d:?})\n{ctx}\nformatted:\n{formatted}")
        });
        let again = active_core::fmt::format(&reparsed, style);
        assert_eq!(
            formatted, again,
            "{label} formatting is not idempotent\n{ctx}"
        );
        // Evaluation is order-independent of formatting: compare values,
        // never their pretty text (a format pass may reorder keys).
        let engine = engine();
        let before = engine.evaluate(&module);
        let after = engine.evaluate(&reparsed);
        if let (Ok((before, _)), Ok((after, _))) = (before, after) {
            assert_eq!(
                active_core::convert::value_to_json(&before),
                active_core::convert::value_to_json(&after),
                "{label} formatting changed the evaluated value\n{ctx}"
            );
        }
    }
    // Secret non-leakage on the shipped redacted-export path.
    let engine = engine();
    if let Ok((value, _, seeds)) = engine.evaluate_with_seeds(&module) {
        let profile = active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds);
        let redacted = active_core::convert::redact_value(&profile, &value);
        assert_no_material(&redacted, &seeds, &ctx);
        let _ = active_core::convert::value_to_pretty_json(&redacted);
    }
}

#[test]
fn monte_carlo_grammar_sweep() {
    let seed = std::env::var("ACTIVE_MC_SEED")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(20_261_114);
    let cases = std::env::var("ACTIVE_MC_CASES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(300);
    let mut rng = Rng(seed | 1);
    println!("monte carlo: seed={seed} cases={cases}");
    for case in 0..cases {
        let family = case % 10;
        let src = document(family, &mut rng);
        check_document(family, case, &src);
    }
}
