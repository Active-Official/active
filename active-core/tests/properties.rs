//! L-P38: property tests as the spec (design `audit-pack/L-P38-DESIGN.md`).
//!
//! P1 determinism, P2 formatter idempotency, P3a formatting semantic
//! preservation, P3b wire fixed point, P4 budget monotonicity, and P5
//! secret non-leakage. Each property runs generated inputs through the
//! real parse/evaluate/format/export surfaces; a failure is a finding,
//! never a weakened assertion.

use active_core::convert::{
    json_value_to_active, redact_value, value_to_json, value_to_pretty_json, value_to_yaml_string,
};
use active_core::diag::Diagnostic;
use active_core::eval::EvalError;
use active_core::fmt::{format, try_format};
use active_core::ir::Value;
use active_core::secret::SecretProfile;
use active_core::{parse, Budget, Engine, Module, Style};
use indexmap::IndexMap;
use proptest::prelude::*;
use serde_json::Value as JsonValue;

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
}

/// A structured, always-valid document in both layouts' common syntax.
fn valid_source(seed: u64) -> String {
    let mut rng = Rng(seed | 1);
    let mut out = String::new();
    let n = 1 + rng.below(4);
    for i in 0..n {
        let key = format!("k{i}");
        match rng.below(9) {
            0 => out.push_str(&format!("{key} = {}\n", rng.below(1000))),
            1 => out.push_str(&format!("{key} = {}.5\n", rng.below(100))),
            2 => out.push_str(&format!("{key} = \"value{i}\"\n")),
            3 => out.push_str(&format!("{key} = {}\n", rng.below(2) == 0)),
            4 => out.push_str(&format!("{key} = null\n")),
            5 => out.push_str(&format!("{key} = {}ms\n", rng.below(100_000))),
            6 => out.push_str(&format!("{key} = {}Hz\n", 1 + rng.below(1000))),
            7 => out.push_str(&format!(
                "{key} = [{}, {}, {}]\n",
                rng.below(100),
                rng.below(100),
                rng.below(100)
            )),
            _ => out.push_str(&format!(
                "{key} {{\n  a = {}\n  b = \"nested{i}\"\n}}\n",
                rng.below(100)
            )),
        }
    }
    out
}

/// Half the documents carry a recoverable parse diagnostic.
fn maybe_invalid_source(seed: u64) -> String {
    let mut text = valid_source(seed);
    if seed.is_multiple_of(2) {
        text.push_str("bad = 2e8\n");
    }
    text
}

fn triples(diags: &[Diagnostic]) -> Vec<(String, String, Option<String>)> {
    diags
        .iter()
        .map(|d| (d.code.clone(), format!("{:?}", d.severity), d.path.clone()))
        .collect()
}

fn snapshot(source: &str) -> String {
    let module = match parse(source, "properties") {
        Ok(module) => module,
        Err(diags) => return format!("parse|{diags:?}"),
    };
    let engine = Engine::hermetic();
    match engine.evaluate_with_seeds(&module) {
        Err(error) => format!("err|{}", error.code()),
        Ok((value, diags, seeds)) => {
            let profile = SecretProfile::for_engine(&module, &engine, &seeds);
            let redacted = redact_value(&profile, &value);
            let redacted_debug = format!("{redacted:?}");
            format!(
                "ok|{}|{:?}|{}|{}",
                value_to_pretty_json(&redacted),
                triples(&diags),
                active_core::provenance::content_hash(&redacted),
                redacted_debug
            )
        }
    }
}

fn evaluate_pair(module: &Module) -> (String, Option<Value>, Vec<Diagnostic>, String) {
    let engine = Engine::hermetic();
    match engine.evaluate_with_seeds(module) {
        Err(error) => (
            format!("err|{}", error.code()),
            None,
            Vec::new(),
            String::new(),
        ),
        Ok((value, diags, seeds)) => {
            let profile = SecretProfile::for_engine(module, &engine, &seeds);
            let redacted = redact_value(&profile, &value);
            let hash = active_core::provenance::content_hash(&redacted);
            (format!("ok|{hash}"), Some(value), diags, hash)
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    /// P1: the same input tuple produces identical diagnostics, values,
    /// redacted bytes, and content hash.
    #[test]
    fn p1_determinism(seed in any::<u64>()) {
        let source = maybe_invalid_source(seed);
        prop_assert_eq!(snapshot(&source), snapshot(&source));
    }

    /// P2: formatter output re-parses and is a byte fixed point in both
    /// styles (invalid-but-parseable documents included).
    #[test]
    fn p2_formatter_idempotency(seed in any::<u64>()) {
        let source = maybe_invalid_source(seed);
        let module = match parse(&source, "properties") {
            Ok(module) => module,
            Err(_) => return Ok(()),
        };
        for style in [Style::Indent, Style::Brace] {
            let first = match try_format(&module, style) {
                Ok(text) => text,
                Err(_) => continue,
            };
            let reparsed = parse(&first, "properties").expect("formatter output must parse");
            let second = format(&reparsed, style);
            prop_assert_eq!(&first, &second, "second format differed in {:?}", style);
        }
    }

    /// P3a: formatting preserves the evaluated outcome, parsed JSON value,
    /// diagnostic triples, and hash (spans excluded by design).
    #[test]
    fn p3a_formatting_semantic_preservation(seed in any::<u64>()) {
        let source = valid_source(seed);
        let module = parse(&source, "properties").expect("valid source parses");
        for style in [Style::Indent, Style::Brace] {
            let formatted = match try_format(&module, style) {
                Ok(text) => text,
                Err(_) => continue,
            };
            let other = parse(&formatted, "properties").expect("formatted parses");
            let (before_class, before_value, before_diags, before_hash) = evaluate_pair(&module);
            let (after_class, after_value, after_diags, after_hash) = evaluate_pair(&other);
            prop_assert_eq!(before_class, after_class);
            prop_assert_eq!(triples(&before_diags), triples(&after_diags));
            prop_assert_eq!(before_hash, after_hash);
            if let (Some(before), Some(after)) = (before_value, after_value) {
                prop_assert_eq!(value_to_json(&before), value_to_json(&after));
            }
        }
    }

    /// P3b: a wire-domain value imports without errors, re-evaluates, and
    /// emits the same canonical JSON bytes.
    #[test]
    fn p3b_wire_fixed_point(value in wire_value()) {
        let wire = value_to_json(&value_to_value(&value));
        let (source, notes) = json_value_to_active(&wire);
        prop_assert!(
            !notes.iter().any(|n| format!("{:?}", n.severity) == "Error"),
            "importer notes: {notes:?}\n{source}"
        );
        let module = parse(&source, "properties")
            .unwrap_or_else(|diags| panic!("imported source parses: {diags:?}\nSOURCE:\n{source}"));
        let (evaluated, _) = Engine::hermetic().evaluate(&module).expect("imported source evaluates");
        prop_assert_eq!(
            serde_json::to_string(&value_to_json(&evaluated)).unwrap(),
            serde_json::to_string(&wire).unwrap()
        );
    }

    /// P4: a tighter budget never admits what a looser budget rejects, and
    /// an admitted value is identical.
    #[test]
    fn p4_budget_monotonicity(
        seed in any::<u64>(),
        limit in 0usize..7,
        tight in 1u64..64,
        slack in 0u64..64,
    ) {
        let source = valid_source(seed);
        let module = parse(&source, "properties").expect("valid source parses");
        let mut tight_budget = Budget::default();
        let mut loose_budget = Budget::default();
        // Wall-clock is excluded from the deterministic property.
        tight_budget.max_wall_ms = u64::MAX;
        loose_budget.max_wall_ms = u64::MAX;
        set_limit(&mut tight_budget, limit, tight);
        set_limit(&mut loose_budget, limit, tight + slack);
        let mut tight_engine = Engine::hermetic();
        tight_engine.budget = tight_budget;
        let mut loose_engine = Engine::hermetic();
        loose_engine.budget = loose_budget;
        let tight_result = tight_engine.evaluate(&module);
        let loose_result = loose_engine.evaluate(&module);
        match (tight_result, loose_result) {
            (Ok((tight_value, _)), Ok((loose_value, _))) => {
                prop_assert_eq!(format!("{tight_value:?}"), format!("{loose_value:?}"));
            }
            (Ok(_), Err(error)) => prop_assert!(false, "looser budget rejected: {error:?}"),
            (Err(tight_error), Err(loose_error)) => {
                let tight_budget_err = matches!(tight_error, EvalError::BudgetExceeded { .. });
                let loose_budget_err = matches!(loose_error, EvalError::BudgetExceeded { .. });
                prop_assert!(
                    !loose_budget_err || tight_budget_err,
                    "loose budget tripped but tight did not: {loose_error:?}"
                );
            }
            (Err(_), Ok(_)) => {}
        }
    }

    /// P5: no export surface carries the runtime secret; material values
    /// still render the marker.
    #[test]
    fn p5_secret_non_leakage(seed in any::<u64>(), secret in "[a-z]{6,16}") {
        let value = value_with_secret(seed, &secret);
        let json = value_to_pretty_json(&value);
        let yaml = value_to_yaml_string(&value).expect("yaml");
        prop_assert!(!json.contains(&secret), "secret leaked into JSON");
        prop_assert!(!yaml.contains(&secret), "secret leaked into YAML");
        prop_assert!(json.contains("***SECRET***"), "marker missing");
    }
}

fn set_limit(budget: &mut Budget, index: usize, value: u64) {
    match index {
        0 => budget.max_nodes = value,
        1 => budget.max_depth = value,
        2 => budget.max_array_len = value,
        3 => budget.max_keys = value,
        4 => budget.max_value_size = value,
        5 => budget.max_total_value_size = value,
        _ => budget.max_wall_ms = value,
    }
}

fn value_to_value(json: &JsonValue) -> Value {
    // Build a Value for the wire input so the first J(V) comes from the
    // shipped mapping (the importer is the reverse direction).
    match json {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(value) => Value::Bool(*value),
        JsonValue::Number(number) => match number.as_i64() {
            Some(value) => Value::Int(value),
            None => Value::Float(number.as_f64().unwrap_or(0.0)),
        },
        JsonValue::String(text) => Value::String(text.clone()),
        JsonValue::Array(items) => Value::Array(items.iter().map(value_to_value).collect()),
        JsonValue::Object(map) => {
            if let Some(inner) = map.get("$active.duration").and_then(JsonValue::as_object) {
                if let Some(ms) = inner.get("ms").and_then(JsonValue::as_u64) {
                    return Value::DurationMs(ms);
                }
            }
            if let Some(inner) = map.get("$active.quantity").and_then(JsonValue::as_object) {
                if let (Some(unit), Some(value)) = (
                    inner.get("unit").and_then(JsonValue::as_str),
                    inner.get("value").and_then(JsonValue::as_f64),
                ) {
                    if let Some(unit) = active_core::types::Unit::from_suffix(unit) {
                        return Value::Quantity(value, unit);
                    }
                }
            }
            Value::Table(
                map.iter()
                    .map(|(key, value)| (key.clone(), value_to_value(value)))
                    .collect(),
            )
        }
    }
}

fn wire_value() -> impl Strategy<Value = JsonValue> {
    let leaf = prop_oneof![
        Just(JsonValue::Null),
        any::<bool>().prop_map(JsonValue::Bool),
        any::<i64>().prop_map(JsonValue::from),
        any::<f64>()
            .prop_filter("finite", |value| value.is_finite())
            .prop_map(|value| serde_json::json!(value)),
        "[A-Za-z0-9 _-]{0,12}".prop_map(JsonValue::String),
        (0u64..=i64::MAX as u64)
            .prop_map(|ms| { serde_json::json!({ "$active.duration": { "ms": ms } }) }),
        (
            prop::sample::select(vec!["Hz"]),
            any::<f64>().prop_filter("finite", |value| value.is_finite()),
        )
            .prop_map(|(unit, value)| {
                serde_json::json!({
                    "$active.quantity": { "unit": unit, "value": value }
                })
            }),
    ];
    let nested = leaf.prop_recursive(3, 32, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(JsonValue::Array),
            prop::collection::btree_map("[a-z][a-z0-9_]{0,6}", inner, 0..4)
                .prop_map(|map| JsonValue::Object(map.into_iter().collect())),
        ]
    });
    prop::collection::btree_map("[a-z][a-z0-9_]{0,6}", nested, 1..4)
        .prop_map(|map| JsonValue::Object(map.into_iter().collect()))
}

fn value_with_secret(seed: u64, secret: &str) -> Value {
    fn gen(rng: &mut Rng, secret: &str, depth: u32) -> Value {
        if depth == 0 {
            return match rng.below(4) {
                0 => Value::Secret(zeroize::Zeroizing::new(secret.to_string())),
                1 => Value::Int(rng.below(100) as i64),
                2 => Value::String(format!("plain{}", rng.below(100))),
                _ => Value::Bool(rng.below(2) == 0),
            };
        }
        match rng.below(5) {
            0 => Value::Secret(zeroize::Zeroizing::new(secret.to_string())),
            1 => Value::Int(rng.below(100) as i64),
            2 => Value::String(format!("plain{}", rng.below(100))),
            3 => Value::Array(
                (0..1 + rng.below(3))
                    .map(|_| gen(rng, secret, depth - 1))
                    .collect(),
            ),
            _ => {
                let mut table = IndexMap::new();
                for index in 0..1 + rng.below(3) {
                    table.insert(format!("k{index}"), gen(rng, secret, depth - 1));
                }
                Value::Table(table)
            }
        }
    }
    let mut rng = Rng(seed | 1);
    let mut root = IndexMap::new();
    root.insert("payload".to_string(), gen(&mut rng, secret, 3));
    root.insert(
        "credential".to_string(),
        Value::Secret(zeroize::Zeroizing::new(secret.to_string())),
    );
    Value::Table(root)
}

#[test]
fn u64_duration_literal_and_wire_boundary() {
    let wire = serde_json::json!({
        "d": { "$active.duration": { "ms": u64::MAX } }
    });
    let (source, notes) = json_value_to_active(&wire);
    assert!(!notes.iter().any(|n| format!("{:?}", n.severity) == "Error"));
    let module = parse(&source, "properties").expect("u64 ms source parses");
    let (value, _) = Engine::hermetic()
        .evaluate(&module)
        .expect("u64 ms source evaluates");
    assert_eq!(
        value_to_json(&value)["d"]["$active.duration"]["ms"],
        u64::MAX
    );
    // Plain integer literals stay i64.
    assert!(parse("x = 18446744073709551615\n", "properties").is_err());
}

#[test]
fn quantity_spelling_stays_a_float() {
    let (source, _) = json_value_to_active(&serde_json::json!({
        "q": { "$active.quantity": { "unit": "Hz", "value": 5.0 } }
    }));
    assert!(source.contains("5.0Hz"), "{source}");
    let (source, _) = json_value_to_active(&serde_json::json!({
        "q": { "$active.quantity": { "unit": "Hz", "value": -0.0 } }
    }));
    let module = parse(&source, "properties").expect("quantity source parses");
    let (value, _) = Engine::hermetic().evaluate(&module).expect("evaluates");
    assert_eq!(value_to_json(&value)["q"]["$active.quantity"]["value"], 0.0);
}
