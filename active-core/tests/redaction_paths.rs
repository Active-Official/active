//! Redaction sweep: every core export path over one secret-bearing
//! document must show `***SECRET***` and never the raw value.
//!
//! The formatter is source-faithful by contract (it reprints the
//! `secret(...)` locator the user wrote, like any literal) while the
//! EVALUATED value redacts — both halves pinned here so neither drifts.

use active_core::convert::{
    collect_items, doc_to_html, doc_to_man, doc_to_markdown, value_to_pretty_json,
    value_to_toml_string, value_to_yaml_string,
};
use active_core::fmt::format;
use active_core::viz;
use active_core::{parse, Budget, Engine, Style};

const RAW: &str = "s3cr3t-sweep-9z";
const DOC: &str = "token = secret(\"s3cr3t-sweep-9z\")\nport = 8080\n[server]\nhost = \"h\"\n";

fn evaluated() -> active_core::Value {
    let module = parse(DOC, "sweep").expect("must parse");
    Engine::new().evaluate(&module).expect("must evaluate").0
}

fn items() -> Vec<active_core::convert::DocItem> {
    let module = parse(DOC, "sweep").expect("must parse");
    let value = evaluated();
    collect_items(&module, &value)
}

#[test]
fn json_yaml_toml_exports_redact() {
    let value = evaluated();
    for (label, text) in [
        ("json", value_to_pretty_json(&value)),
        ("yaml", value_to_yaml_string(&value).expect("yaml")),
        ("toml", value_to_toml_string(&value).expect("toml").0),
    ] {
        assert!(!text.contains(RAW), "{label} leaked raw secret:\n{text}");
        assert!(
            text.contains("***SECRET***"),
            "{label} lost marker:\n{text}"
        );
    }
}

#[test]
fn docgen_all_formats_redact() {
    let items = items();
    for (label, text) in [
        ("markdown", doc_to_markdown("Svc", &items)),
        ("html", doc_to_html("Svc", &items)),
        ("man", doc_to_man("Svc", &items)),
    ] {
        assert!(!text.contains(RAW), "{label} leaked raw secret:\n{text}");
        assert!(
            text.contains("***SECRET***"),
            "{label} lost marker:\n{text}"
        );
    }
}

#[test]
fn tooling_surfaces_redact_derived_material() {
    // A plain value used as a secret locator (and an interpolation over
    // it) is material without a `Value::Secret` marker. Sweep every
    // tooling emitter over one such document so a new surface cannot
    // regress the invariant silently.
    let src = "x = \"s3cr3t-derived\"\nmsg = \"pre-${x}\"\nk = secret(x)\nt = { a = x }\n";
    let module = parse(src, "sweep-derived").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module)
        .expect("must step");
    let taint = viz::taint_data(&module, &value);
    let budget = viz::budget_data(&module, &value, &Budget::default(), &diags);
    let graph = active_core::graph::secret_tainted_paths_with(&module, &value, None);
    let inspect_x = active_core::convert::inspect_path(&module, &value, "x").expect("inspect x");
    let inspect_msg =
        active_core::convert::inspect_path(&module, &value, "msg").expect("inspect msg");
    assert_eq!(inspect_x["tainted"], true, "{inspect_x}");
    assert_eq!(inspect_msg["tainted"], true, "{inspect_msg}");
    for (label, text) in [
        ("steps", serde_json::to_string(&steps).expect("step json")),
        ("taint", taint.to_string()),
        ("budget", budget.to_string()),
        ("graph", graph.join(",")),
        ("inspect-x", inspect_x.to_string()),
        ("inspect-msg", inspect_msg.to_string()),
    ] {
        assert!(
            !text.contains("s3cr3t-derived"),
            "{label} leaked derived material: {text}"
        );
    }
}

#[test]
fn a_material_table_key_never_reaches_any_surface() {
    // A key whose text is material is a copy: exports, inspection,
    // step traces, taint paths, and viz must never publish it.
    use active_core::secret::SecretProfile;
    let src = "token_key = secret(\"token_key\")\n\
               payload = { token_key = true, ok = 1 }\n";
    let module = parse(src, "keys").expect("parse");
    let engine = Engine::hermetic();
    let (value, diags, seeds) = engine.evaluate_with_seeds(&module).expect("evaluates");
    assert!(diags.is_empty(), "{diags:?}");
    let profile = SecretProfile::for_engine(&module, &engine, &seeds);
    for (label, text) in [
        (
            "json",
            active_core::convert::value_to_pretty_json_with(&profile, &value),
        ),
        (
            "yaml",
            active_core::convert::value_to_yaml_string_with(&profile, &value).expect("yaml"),
        ),
        (
            "toml",
            active_core::convert::value_to_toml_string_with(&profile, &value)
                .expect("toml")
                .0,
        ),
    ] {
        assert!(
            !text.contains("token_key"),
            "{label} leaked a material key:\n{text}"
        );
        assert!(text.contains("***SECRET***"), "{label}:\n{text}");
    }

    let (_, _, steps, _) = engine
        .evaluate_with_steps_and_seeds(&module)
        .expect("steps");
    let steps = serde_json::to_string(&steps).expect("steps json");
    assert!(!steps.contains("token_key"), "steps leaked a key: {steps}");

    let paths = active_core::graph::secret_tainted_paths_with(&module, &value, None);
    assert!(
        paths.iter().all(|path| !path.contains("token_key")),
        "{paths:?}"
    );
    let viz = active_core::viz::taint_data_with_profile(&module, &value, None);
    assert!(
        !viz.to_string().contains("token_key"),
        "viz leaked a key: {viz}"
    );
}

#[test]
fn a_short_material_key_segment_is_redacted_in_paths() {
    // A three-byte secret only matches a whole segment; the dotted path
    // around it must not make it invisible to redaction.
    let src = "x = secret(\"x\")\npayload = { outer = { x = secret(\"x\") } }\n";
    let module = parse(src, "short-key").expect("parse");
    let engine = Engine::hermetic();
    let (value, diags, _seeds) = engine.evaluate_with_seeds(&module).expect("evaluates");
    assert!(diags.is_empty(), "{diags:?}");

    let (_, _, steps, _) = engine
        .evaluate_with_steps_and_seeds(&module)
        .expect("steps");
    let steps = serde_json::to_string(&steps).expect("steps json");
    assert!(!steps.contains("outer.x"), "steps leaked a path: {steps}");
    assert!(
        steps.contains("outer.***SECRET***"),
        "the segment must render the marker: {steps}"
    );

    let paths = active_core::graph::secret_tainted_paths_with(&module, &value, None);
    assert!(paths.iter().all(|path| !path.ends_with(".x")), "{paths:?}");
    let viz = active_core::viz::taint_data_with_profile(&module, &value, None);
    assert!(
        !viz.to_string().contains("outer.x"),
        "viz leaked a path: {viz}"
    );
    let report =
        active_core::convert::inspect_path(&module, &value, "payload.outer.x").expect("inspect");
    assert_eq!(
        report["path"], "payload.outer.***SECRET***",
        "inspect leaked a path segment: {report}"
    );
}

#[test]
fn the_blob_query_redacts_a_short_material_key_segment() {
    use active_core::secret::SecretProfile;
    let src = "x = secret(\"x\")\n\
               payload = { outer = { x = blob(\"text/plain\", \"hi\") } }\n";
    let module = parse(src, "short-key-blob").expect("parse");
    let engine = Engine::hermetic();
    let (value, diags, seeds) = engine.evaluate_with_seeds(&module).expect("evaluates");
    assert!(diags.is_empty(), "{diags:?}");
    let profile = SecretProfile::for_engine(&module, &engine, &seeds);
    let blobs = active_core::graph::blobs_above_size_with(&value, 1, &profile);
    assert_eq!(blobs[0].0, "payload.outer.***SECRET***", "{blobs:?}");
}

#[test]
fn inspect_errors_redact_a_material_path_segment() {
    let module = parse(
        "x = secret(\"x\")\nv = { outer = { x = 1 } }\n",
        "inspect-err",
    )
    .expect("parse");
    let engine = Engine::hermetic();
    let (value, _, _) = engine.evaluate_with_seeds(&module).expect("evaluates");
    let error = active_core::convert::inspect_path(&module, &value, "v.outer.x.missing")
        .expect_err("missing path");
    assert!(!error.contains("outer.x"), "inspect leaked a path: {error}");
    assert!(error.contains("***SECRET***"), "{error}");
}

#[test]
fn runtime_secrets_redact_repair_fields() {
    // Runtime (host/computed) secrets live in the evaluator redactor,
    // not in the authored profile; the repair sweep must cover both.
    let mut redactor = active_core::secret::SecretRedactor::new();
    redactor.insert("runtime-key-value");
    let mut repair = active_core::Repair {
        original: "runtime-key-value".to_string(),
        suggested: "password".to_string(),
        kind: active_core::RepairKind::KeyTypo,
        safe: false,
        side_effects: vec!["runtime-key-value".to_string()],
        goal: None,
        steps: vec![],
    };
    redactor.redact_repair(&mut repair);
    assert!(!repair.original.contains("runtime-key-value"), "{repair:?}");
    assert!(
        !repair
            .side_effects
            .iter()
            .any(|path| path.contains("runtime-key-value")),
        "{repair:?}"
    );
    assert_eq!(repair.suggested, "password");
}

#[test]
fn a_material_typo_key_yields_no_rename_to_the_marker() {
    // A redacted repair names the marker; a rename from or to it cannot
    // be applied and must not be planned.
    let src = "s = secret(\"passwrod\")\n\
               schema = { password = String, closed = true }\n\
               password = \"x\"\npasswrod = \"y\"\n";
    let engine = Engine::hermetic();
    let outcome = active_core::convert::repair_with_engine(
        src,
        active_core::diag::RepairGoal::Validate,
        &[],
        16,
        &engine,
        None,
    );
    for step in &outcome.steps {
        assert!(
            !step.directive.contains("***SECRET***") && !step.path.contains("***SECRET***"),
            "the plan claims an unappliable step: {} {}",
            step.directive,
            step.path
        );
    }
}

#[test]
fn structural_material_never_reaches_exports_or_the_content_hash() {
    use std::collections::HashMap;
    // A plain env read used as a secret locator, its aliases, a
    // container, and an interpolation are all material. Exports must
    // mark them, and the content hash must be stable across the
    // material value itself.
    let src = "let t = env.R20SECRET\nk = secret(t)\nleak = t\ntable = { inner = t }\ncomposed = \"pre-${t}\"\n";
    let module = parse(src, "export-sweep").expect("must parse");
    let evaluate = |value: &str| {
        let mut env = HashMap::new();
        env.insert("R20SECRET".to_string(), value.to_string());
        Engine::hermetic()
            .with_env(env)
            .evaluate(&module)
            .expect("must evaluate")
            .0
    };
    let first = evaluate("ENVSECRET-export");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    let redacted = active_core::convert::redact_value(&profile, &first);
    let json = active_core::convert::value_to_json(&redacted);
    let yaml = active_core::convert::value_to_yaml_string(&redacted).expect("yaml");
    let toml = active_core::convert::value_to_toml_string(&redacted)
        .expect("toml")
        .0;
    for (label, text) in [("json", json.to_string()), ("yaml", yaml), ("toml", toml)] {
        assert!(
            !text.contains("ENVSECRET"),
            "{label} leaked structural material: {text}"
        );
        assert!(
            text.contains("***SECRET***"),
            "{label} lost markers: {text}"
        );
    }
    let first_hash = active_core::provenance::content_hash(&redacted);
    let second = evaluate("OTHER-SECRET-999");
    let second_redacted = active_core::convert::redact_value(&profile, &second);
    assert_eq!(
        first_hash,
        active_core::provenance::content_hash(&second_redacted),
        "the content hash must not depend on material values"
    );
}

#[test]
fn formatter_keeps_source_locator_but_value_redacts() {
    // Source fidelity: the formatter reprints what the user wrote,
    // including the secret locator (formatters echo source; `cat`
    // does the same). The evaluated tree still redacts.
    let module = parse(DOC, "sweep").expect("must parse");
    for style in [Style::Brace, Style::Indent, Style::Preserve] {
        let out = format(&module, style);
        assert!(out.contains("secret("), "locator dropped:\n{out}");
    }
    let json = value_to_pretty_json(&evaluated());
    assert!(!json.contains(RAW), "{json}");
    assert!(json.contains("***SECRET***"), "{json}");
}

#[test]
fn eval_past_100k_paths_keeps_benign_values() {
    // LANG-K-001: past MAX_PATHS (100,000) the SecretProfile path map
    // truncated and path_is_secret failed closed, replacing benign
    // values with `***SECRET***`. A 100,098-path document with zero
    // secrets must evaluate with every value intact.
    let mut src = String::new();
    for t in 0..498 {
        src.push_str(&format!("[t{t}]\n"));
        src.push_str(&format!("k0 = {t}\n"));
        for k in 1..200 {
            src.push_str(&format!("k{k} = k{} + 1\n", k - 1));
        }
    }
    let module = parse(&src, "k001").expect("must parse");
    let mut engine = Engine::new();
    engine.budget = Budget::trusted();
    // A zero field resolves to the shipped default (LANG-C-015); the
    // sentinel is the explicit "no deadline" this values-only test wants.
    engine.budget.max_wall_ms = active_core::eval::UNLIMITED;
    let (value, _) = engine.evaluate(&module).expect("must evaluate");
    let profile = active_core::secret::SecretProfile::analyze(&module);
    let redacted = active_core::convert::redact_value(&profile, &value);
    let json = active_core::convert::value_to_pretty_json(&redacted);
    assert!(
        !json.contains("***SECRET***"),
        "benign values redacted past 100k paths"
    );
    assert!(
        !profile.truncated(),
        "the path map must cover an accepted document"
    );
    // Spot-check the earliest values (the ones the old code redacted).
    match &value {
        active_core::Value::Table(map) => {
            let t0 = match map.get("t0").expect("t0 present") {
                active_core::Value::Table(t) => t,
                other => panic!("table expected: {other:?}"),
            };
            assert!(matches!(t0.get("k0"), Some(active_core::Value::Int(0))));
            assert!(matches!(t0.get("k199"), Some(active_core::Value::Int(199))));
        }
        other => panic!("table expected: {other:?}"),
    }
}
