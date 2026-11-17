//! Unit round-trip tests.
//!
//! Guards the invariant that every unit suffix produced by `Unit::suffix()`
//! (used by JSON/TOML export and interpolation) is re-parseable by the
//! lexer and `Unit::from_suffix`, so exported configurations can be fed
//! back into Active unchanged. Drives the real shipped path:
//! parse → evaluate → export → parse → evaluate.

use active_core::convert::{value_to_pretty_json, value_to_simple_toml, value_to_toml_string};
use active_core::{parse, Engine};

fn eval_json(src: &str) -> String {
    let module = parse(src, "roundtrip").expect("must parse");
    let engine = Engine::new();
    let (value, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    value_to_pretty_json(&value)
}

#[test]
fn rate_unit_suffixes_round_trip_through_export() {
    // Covers every rate unit and both slash-prefixed and bare spellings.
    let sources = [
        "x = 100/req/s",
        "x = 100/req/m",
        "x = 100/req/h",
        "x = 100req/s",
        "x = 100req/m",
        "x = 100req/h",
        "x = 100/s",
        "x = 100/m",
        "x = 100/h",
    ];
    for src in sources {
        let first = eval_json(src);
        let module = parse(src, "roundtrip").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(
            first, second,
            "round-trip failed for {src}: exported as {exported:?}"
        );
    }
}

#[test]
fn percent_round_trips_through_export() {
    // Percent canonicalizes to the ratio (100% = 1.0) but respells in
    // percent-form, so export → parse → evaluate is a fixed point.
    let sources = [
        "x = 25%",
        "x = 50%",
        "x = 12.5%",
        "x = 150%",
        "x = -5%",
        "x = 0.1%",
    ];
    for src in sources {
        let first = eval_json(src);
        let module = parse(src, "roundtrip").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(
            first, second,
            "round-trip failed for {src}: exported as {exported:?}"
        );
    }
}

/// Round-trip for time and byte units (pre-existing coverage, untouched).
#[test]
fn time_and_byte_units_round_trip_through_export() {
    let sources = [
        "x = 5s",
        "x = 500ms",
        "x = 15m",
        "x = 2h",
        "x = 512MiB",
        "x = 2KiB",
        "x = 1GiB",
        "x = 100MB",
    ];
    for src in sources {
        let first = eval_json(src);
        let module = parse(src, "roundtrip").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(
            first, second,
            "round-trip failed for {src}: exported as {exported:?}"
        );
    }
}

#[test]
fn frequency_round_trips_through_export() {
    // All spellings canonicalize to Hz-form; export re-parses to the same
    // canonical value (fixed point of export o parse o evaluate).
    let sources = [
        "x = 60Hz",
        "x = 1kHz",
        "x = 2.4GHz",
        "x = 1MHz",
        "x = 1THz",
        "x = 0.5Hz",
    ];
    for src in sources {
        let first = eval_json(src);
        let module = parse(src, "roundtrip").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let exported = value_to_simple_toml(&value, "");
        let second = eval_json(&exported);
        assert_eq!(
            first, second,
            "round-trip failed for {src}: exported as {exported:?}"
        );
    }
}

#[test]
fn unit_spellings_reparse_to_the_same_variant() {
    // `Unit::spelling` is the inverse of `from_suffix`: every variant must
    // survive `spelling -> from_suffix`. This is what makes a formatted
    // source amount (`2km`, `2Cel`, `45deg`) mean the same quantity again.
    use active_core::Unit;
    let units = [
        Unit::Nanoseconds,
        Unit::Microseconds,
        Unit::Milliseconds,
        Unit::Seconds,
        Unit::Minutes,
        Unit::Hours,
        Unit::Days,
        Unit::Weeks,
        Unit::Bytes,
        Unit::Kibibytes,
        Unit::Mebibytes,
        Unit::Gibibytes,
        Unit::Tebibytes,
        Unit::Pebibytes,
        Unit::Exbibytes,
        Unit::Kilobytes,
        Unit::Megabytes,
        Unit::Gigabytes,
        Unit::Terabytes,
        Unit::PerSecond,
        Unit::PerMinute,
        Unit::PerHour,
        Unit::RequestsPerSecond,
        Unit::RequestsPerMinute,
        Unit::RequestsPerHour,
        Unit::Percent,
        Unit::Hertz,
        Unit::Kilohertz,
        Unit::Megahertz,
        Unit::Gigahertz,
        Unit::Terahertz,
        Unit::Bits,
        Unit::Kilobits,
        Unit::Megabits,
        Unit::Gigabits,
        Unit::BitsPerSecond,
        Unit::KilobitsPerSecond,
        Unit::MegabitsPerSecond,
        Unit::GigabitsPerSecond,
        Unit::MegabytesPerSecond,
        Unit::Tokens,
        Unit::KiloTokens,
        Unit::MegaTokens,
        Unit::TokensPerSecond,
        Unit::Flops,
        Unit::TeraFlops,
        Unit::PetaFlops,
        Unit::Pascal,
        Unit::Bar,
        Unit::Psi,
        Unit::Volts,
        Unit::Amperes,
        Unit::Watts,
        Unit::Joules,
        Unit::KilowattHours,
        Unit::Newtons,
        Unit::Degrees,
        Unit::Radians,
        Unit::Rpm,
        Unit::Meters,
        Unit::Kilometers,
        Unit::Kilograms,
        Unit::Grams,
        Unit::Kelvin,
        Unit::Celsius,
        Unit::Fahrenheit,
        Unit::Decibel,
        Unit::Custom("furlongs".into()),
    ];
    for unit in units {
        let spelling = unit.spelling();
        assert_eq!(
            Unit::from_suffix(&spelling),
            Some(unit.clone()),
            "spelling {spelling:?} does not re-parse as {unit:?}"
        );
    }
}

#[test]
fn formatting_preserves_scaled_and_affine_quantities() {
    // Regression (fuzz: `top = 2Cel`): the formatter rendered the canonical
    // DIMENSION suffix on the unnormalized source amount, so `2Cel` became
    // `2K` (2 K instead of 275.15 K), `2km` became `2meter`, and so on.
    // `fmt` must preserve the evaluated value for every unit family.
    use active_core::fmt::format;
    use active_core::Style;
    let sources = [
        "x = 2Cel",
        "x = -40Fahr",
        "x = 0K",
        "x = 2km",
        "x = 2MHz",
        "x = 5MB/s",
        "x = 1kWh",
        "x = 45deg",
        "x = 3rpm",
        "x = 2KiB",
        "x = 5MiB",
        "x = 2h",
        "x = 500ms",
        "x = 3kg",
        "x = 250g",
        "x = 3dB",
        "x = 1000tokens",
        "x = 2Pa",
        "x = 100%",
        "x = 7bar",
        "x = 2Gb",
        "x = 15Mbps",
        "x = 3PFLOPS",
        "x = 100req/m",
        "x = 1week",
    ];
    for src in sources {
        let first = eval_json(src);
        let module = parse(src, "fmt-units").expect("must parse");
        for style in [Style::Indent, Style::Brace] {
            let formatted = format(&module, style);
            let reparsed = parse(&formatted, "fmt-units")
                .unwrap_or_else(|d| panic!("{src} formatted to unparseable {formatted:?}: {d:?}"));
            let engine = Engine::new();
            let (value, _) = engine.evaluate(&reparsed).expect("must evaluate");
            let second = value_to_pretty_json(&value);
            assert_eq!(
                first, second,
                "formatting changed {src} to {formatted:?} ({style:?})"
            );
        }
    }
}

#[test]
fn toml_wrappers_round_trip_durations_and_quantities() {
    // LANG-B-018: the TOML exporter used to emit normalized magnitude
    // strings, so the imported document lost the type (a duration became a
    // string). The reserved wrappers are the same ones JSON uses, so the
    // importer recognises them without guessing.
    let sources = [
        "x = 5000ms",
        "x = 2h",
        "x = 25%",
        "x = 2MHz",
        "x = 45deg",
        "x = 3dB",
        "x = 100/req/s",
        "x = 9.8N",
    ];
    for src in sources {
        let module = parse(src, "toml-roundtrip").expect("must parse");
        let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let (toml, _) = value_to_toml_string(&value).expect("toml export");
        assert!(
            toml.contains("$active.duration") || toml.contains("$active.quantity"),
            "{src}: expected a typed wrapper, got {toml:?}"
        );
        let (source, _) =
            active_core::convert::toml_text_to_active(&toml).expect("wrapper must import");
        let imported = parse(&source, "toml-roundtrip-import").expect("emitted source parses");
        let (back, _) = Engine::new()
            .evaluate(&imported)
            .expect("imported document evaluates");
        assert_eq!(
            value_to_pretty_json(&value),
            value_to_pretty_json(&back),
            "{src} -> {toml} -> {source}"
        );
    }
}
