//! Debugger tests (TOOLING DEBUG.001-006): step traces redact secret
//! material, breakpoints select deterministically, replay compares
//! hashes and traces, and reduction preserves the target diagnostic.

use active_core::debug;
use active_core::{parse, Engine};

fn emits(src: &str, code: &str) -> bool {
    let (module, parse_diags) =
        active_core::parse_recovering(src, "target-check", active_core::ParseOptions::default());
    if parse_diags.iter().any(|d| d.code == code) {
        return true;
    }
    match active_core::Engine::new().evaluate(&module) {
        Ok((_, diagnostics)) => diagnostics.iter().any(|d| d.code == code),
        Err(error) => error.code() == code,
    }
}

fn module(src: &str) -> active_core::Module {
    parse(src, "debug").expect("parse")
}

#[test]
fn steps_redact_secret_locator_literals() {
    let src = "token = secret(\"hunter2-step\")\nport = 8080\n";
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("hunter2"),
        "raw secret in steps: {rendered}"
    );
    assert!(steps.iter().any(|s| s.tainted), "taint recorded");
    assert!(steps.iter().any(|s| s.path.as_deref() == Some("token")));
}

#[test]
fn breakpoints_select_by_taint_path_and_code() {
    let src = "let base = 1\nport = base\nsecret_value = secret(\"x\")\n";
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let taint = debug::breakpoint_index(&steps, "taint").expect("taint breakpoint");
    assert!(steps[taint].tainted);
    let by_path = debug::breakpoint_index(&steps, "path:port").expect("path breakpoint");
    assert_eq!(steps[by_path].path.as_deref(), Some("port"));
    let by_node = debug::breakpoint_index(&steps, "node:0").expect("node breakpoint");
    assert_eq!(by_node, 0);
    assert!(debug::breakpoint_index(&steps, "path:nope").is_none());
    assert!(debug::breakpoint_index(&steps, "bogus").is_none());
}

#[test]
fn replay_is_deterministic_and_honors_expected_hash() {
    let src = "port = 8080\nname = \"svc\"\n";
    let module = module(src);
    let report = debug::replay(&module, None).expect("replay");
    assert_eq!(report["ok"], true);
    assert_eq!(report["steps_match"], true);
    let hash = report["hash"].as_str().expect("hash").to_string();
    let expected = debug::replay(&module, Some(&hash)).expect("replay");
    assert_eq!(expected["expected_match"], true);
    let mismatch = debug::replay(&module, Some("sha256:00")).expect("replay");
    assert_eq!(mismatch["ok"], false);
    assert_eq!(mismatch["expected_match"], false);
}

#[test]
fn reduction_keeps_the_target_diagnostic_and_shrinks() {
    let src = "schema = { port = Int(max = 10) }\nport = 999\nunrelated = 1\n# comment\n";
    let (reduced, attempts) = debug::reduce(src, "T003", 100);
    assert!(attempts > 0);
    assert!(reduced.len() < src.len(), "reduced: {reduced:?}");
    assert!(
        reduced.contains("port = 999"),
        "target line kept: {reduced:?}"
    );
    assert!(!reduced.contains("unrelated"), "noise removed: {reduced:?}");
    let (_, diagnostics) = Engine::new()
        .evaluate(&parse(&reduced, "reduced").expect("reduced parses"))
        .expect("reduced evaluates");
    assert!(
        diagnostics.iter().any(|d| d.code == "T003"),
        "target survives: {diagnostics:?}"
    );
}

#[test]
fn minimal_case_reports_sizes_and_keeps_source_shape() {
    let src = "schema = { port = Int(max = 10) }\nport = 999\nunrelated = 1\n";
    let report = debug::minimal_case(src, "T003", 100);
    assert_eq!(report["target_code"], "T003");
    assert!(report["reduced_bytes"].as_u64().unwrap() < report["original_bytes"].as_u64().unwrap());
    assert!(report["reduced"].as_str().unwrap().contains("port = 999"));
}

#[test]
fn step_trace_redacts_locators_through_bindings_and_env() {
    use std::collections::HashMap;
    let mut env = HashMap::new();
    env.insert("SUPER_PASS".to_string(), "ENVSECRET-xyz789".to_string());
    let module = parse("p = env.SUPER_PASS\nk = secret(p)\n", "debug-env").expect("parse");
    let (_, _, steps) = Engine::hermetic()
        .with_env(env)
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("ENVSECRET"),
        "secret through a binding leaked: {rendered}"
    );
    assert!(
        steps.iter().any(|s| s.kind == "table" && s.tainted),
        "container step must be tainted: {steps:?}"
    );
}

#[test]
fn step_trace_redacts_literal_locator_behind_a_binding() {
    let module = parse("loc = \"hunter2-via-let\"\nk = secret(loc)\n", "debug-ref").expect("parse");
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(!rendered.contains("hunter2"), "{rendered}");
}

#[test]
fn taint_breakpoint_matches_a_tainted_blob() {
    let module = parse("f = blob(\"text/plain\", secret(\"x\"))\n", "debug-blob").expect("parse");
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let index = debug::breakpoint_index(&steps, "taint").expect("taint breakpoint");
    assert!(steps[index].tainted);
    assert!(
        steps.iter().any(|s| s.kind == "blob" && s.tainted),
        "tainted blob step missing: {steps:?}"
    );
}

#[test]
fn minimal_case_redacts_authored_secret_literals() {
    let src = "schema = { token = Int }\ntoken = secret(\"hunter2-min\")\nnoise = 1\n";
    let report = debug::minimal_case(src, "T002", 100);
    let rendered = report.to_string();
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert!(
        report["reduced"]
            .as_str()
            .unwrap()
            .contains("secret(\"***SECRET***\")"),
        "{}",
        report["reduced"]
    );
}

#[test]
fn minimal_case_redacts_every_secret_spelling_without_duplication() {
    let cases = [
        (
            "schema = { token = Int }\ntoken = secret (\"hunter2-a\")\nnoise = 1\n",
            "T002",
        ),
        (
            "schema = { token = Int }\ntoken = secret((\"hunter2-b\"))\nnoise = 1\n",
            "T002",
        ),
        (
            "schema = { token = Int }\ntoken = secret(\"\"\"hunter2-c\"\"\")\nnoise = 1\n",
            "T002",
        ),
        (
            "schema = { token = Int }\nlet p = \"hunter2-d\"\ntoken = secret(p)\nnoise = 1\n",
            "T002",
        ),
        (
            "schema = { token = Int }\ntoken = secret(\"hunter2-e\nnoise = 1\n",
            "E001",
        ),
    ];
    for (src, code) in cases {
        assert!(emits(src, code), "input does not emit {code}: {src:?}");
        let report = debug::minimal_case(src, code, 100);
        let reduced = report["reduced"].as_str().expect("reduced");
        assert!(!reduced.contains("hunter2"), "leak for {src:?}: {reduced}");
        assert_eq!(
            reduced.matches("token = secret").count(),
            1,
            "expected exactly one secret call for {src:?}: {reduced}"
        );
    }
}

#[test]
fn minimal_case_redacts_interpolated_and_malformed_secret_calls() {
    let cases = [
        (
            "schema = { token = Int }\ntoken = secret(\"hunter2-a-${'x'}-hunter3-b\")\nnoise = 1\n",
            "T002",
        ),
        (
            "schema = { token = Int }\ntoken = secret(\"hunter2-${'aaaaaaaaaa'}-é\")\nnoise = 1\n",
            "T002",
        ),
        (
            "schema = { token = Int }\ntoken = secret(\"hunter2-a\", \"hunter2-b\")\nnoise = 1\n",
            "E002",
        ),
        (
            "schema = { token = Int }\ntoken = secret\n(\"hunter2-nl\")\nnoise = 1\n",
            "E002",
        ),
    ];
    for (src, code) in cases {
        assert!(emits(src, code), "input does not emit {code}: {src:?}");
        let report = debug::minimal_case(src, code, 100);
        let rendered = report.to_string();
        assert!(
            !rendered.contains("hunter2"),
            "leak for {src:?}: {rendered}"
        );
        assert!(
            !rendered.contains("hunter3"),
            "leak for {src:?}: {rendered}"
        );
        assert!(
            report["reduced_bytes"].as_u64().unwrap() <= report["original_bytes"].as_u64().unwrap(),
            "reduced size must not be measured after redaction: {report}"
        );
    }
}

#[test]
fn minimal_case_handles_a_bom_prefixed_first_line() {
    let src = "\u{feff}token = secret(\"hunter2-bom\")\nschema = { token = Int }\nnoise = 1\n";
    let report = debug::minimal_case(src, "T002", 100);
    let rendered = report.to_string();
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert!(
        report["reduced"]
            .as_str()
            .unwrap()
            .contains("secret(\"***SECRET***\")"),
        "{}",
        report["reduced"]
    );
}

#[test]
fn minimal_case_does_not_redact_env_names_or_unrelated_strings() {
    let env_src = "schema = { token = Int }\ntoken = secret.env(\"HUNTER2_ENV_NAME\")\nnoise = 1\n";
    let report = debug::minimal_case(env_src, "T002", 100);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(
        reduced.contains("HUNTER2_ENV_NAME"),
        "env names are metadata, not material: {reduced}"
    );
    let malformed = "schema = { token = Int }\ntoken = secret + (\"NOT_A_SECRET\")\nnoise = 1\n";
    let report = debug::minimal_case(malformed, "E002", 100);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(
        reduced.contains("NOT_A_SECRET"),
        "a string outside a secret call is not material: {reduced}"
    );
}

#[test]
fn reduced_bytes_is_measured_before_redaction() {
    let long = "A".repeat(200);
    let src = format!("token = secret(\"{long}\")\n");
    let report = debug::minimal_case(&src, "T999", 10);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert_eq!(
        report["reduced_bytes"].as_u64().unwrap(),
        report["original_bytes"].as_u64().unwrap()
    );
    assert!(
        reduced.len() < report["reduced_bytes"].as_u64().unwrap() as usize,
        "redaction must not inflate the reported size: {report}"
    );
}

#[test]
fn minimal_case_redacts_newline_split_calls_with_nonliteral_locators() {
    let cases = [
        (
            "loc = \"hunter2-r1\"\ntoken = secret\n(loc)\nnoise = 1\n",
            "E002",
        ),
        (
            "t = { a = \"hunter2-t\" }\ntoken = secret\n(t.a)\nnoise = 1\n",
            "E002",
        ),
        (
            "schema = { token = Int }\ntoken = secret\n\n\n(\"hunter2-3nl\")\nnoise = 1\n",
            "E002",
        ),
    ];
    for (src, code) in cases {
        let report = debug::minimal_case(src, code, 100);
        let reduced = report["reduced"].as_str().expect("reduced");
        assert!(!reduced.contains("hunter2"), "leak for {src:?}: {reduced}");
    }
}

#[test]
fn reduction_never_strands_a_secret_literal() {
    let src = "schema = { token = Int, loc = Int }\ntoken = secret(loc)\nloc = \"hunter2-ref\"\nnoise = 1\n";
    let report = debug::minimal_case(src, "T002", 100);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(
        !reduced.contains("hunter2"),
        "stranded literal leaked: {reduced}"
    );
    assert!(
        reduced.contains("secret"),
        "secret use was deleted: {reduced}"
    );
}

#[test]
fn env_extra_arguments_are_redacted_but_the_name_is_not() {
    let src = "token = secret.env(\"HUNTER2_ENV_NAME\", default = \"hunter2-def\")\nnoise = 1\n";
    let report = debug::minimal_case(src, "E001", 100);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(
        reduced.contains("HUNTER2_ENV_NAME"),
        "env name must survive: {reduced}"
    );
    assert!(!reduced.contains("hunter2"), "default leaked: {reduced}");
}

#[test]
fn comments_inside_secret_calls_do_not_hide_material() {
    let array_src = "schema = { token = Int }\ntoken = secret([\n  # note secret\n  (\"hunter2-ARR3\")\n])\nnoise = 1\n";
    let report = debug::minimal_case(array_src, "T002", 100);
    let rendered = report.to_string();
    assert!(!rendered.contains("hunter2"), "{rendered}");

    let gap_src = "loc = \"hunter2-CMT\"\ntoken = secret # note\n(loc)\nnoise = 1\n";
    let report = debug::minimal_case(gap_src, "E002", 100);
    let rendered = report.to_string();
    assert!(!rendered.contains("hunter2"), "{rendered}");
}

#[test]
fn reduction_protects_indented_secret_calls_with_file_context() {
    let src = "schema = { loc = Int }\nloc = \"hunter2-STRAND-SCHEMA\"\ntoken = secret\n  (loc)\n";
    let report = debug::minimal_case_with(
        src,
        "T002",
        100,
        Some(std::path::Path::new("strand.active")),
    );
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(
        !reduced.contains("hunter2"),
        "stranded literal leaked: {reduced}"
    );
    assert!(
        reduced.contains("secret"),
        "secret use was deleted: {reduced}"
    );
}

#[test]
fn env_default_with_nonliteral_name_is_redacted() {
    let src = "token = secret.env(NAME, default = \"hunter2-ENVREF\")\nnoise = 1\n";
    let report = debug::minimal_case(src, "E001", 100);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(
        !reduced.contains("hunter2"),
        "env default leaked: {reduced}"
    );

    let multiline = "token = secret.env\n(\"NAME\", default = \"hunter2-ML\")\nnoise = 1\n";
    let report = debug::minimal_case(multiline, "E002", 100);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(
        !reduced.contains("hunter2"),
        "multiline env default leaked: {reduced}"
    );
    assert!(reduced.contains("NAME"), "env name must survive: {reduced}");
}

#[test]
fn env_exemption_is_positionally_strict_and_dot_layout_is_handled() {
    let cases = [
        "token = secret.env(default = \"hunter2-NAMED\", \"NAME\")\nnoise = 1\n",
        "token = secret.env(f(\"hunter2-NESTED\"))\nnoise = 1\n",
        "loc = \"hunter2-ENVDEF\"\ntoken = secret.env(\"ENVNAME\", default = loc)\nnoise = 1\n",
        "token = secret\n.\nenv(\"NAME\", default = \"hunter2-DOT\")\nnoise = 1\n",
    ];
    for src in cases {
        let code = first_code(src).unwrap_or_else(|| "E001".to_string());
        assert!(emits(src, &code), "target {code} missing for {src:?}");
        let report = debug::minimal_case(src, &code, 200);
        let rendered = report.to_string();
        assert!(
            !rendered.contains("hunter2"),
            "leak for {src:?}: {rendered}"
        );
    }
}

fn first_code(src: &str) -> Option<String> {
    let (module, parse_diags) =
        active_core::parse_recovering(src, "first-code", active_core::ParseOptions::default());
    if let Some(diagnostic) = parse_diags.first() {
        return Some(diagnostic.code.clone());
    }
    match active_core::Engine::new().evaluate(&module) {
        Ok((_, diagnostics)) => diagnostics.first().map(|d| d.code.clone()),
        Err(error) => Some(error.code().to_string()),
    }
}

#[test]
fn step_trace_redacts_interpolated_secret_sources() {
    // A plain string used as a locator stays material through an
    // interpolation: the step must render the marker and report taint,
    // not the composed text.
    let module = parse(
        "x = \"hunter2-interp\"\nmsg = \"prefix ${x}\"\nk = secret(x)\n",
        "debug-interp",
    )
    .expect("parse");
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("hunter2"),
        "interpolation leaked: {rendered}"
    );
    let msg = steps
        .iter()
        .find(|step| step.path.as_deref() == Some("msg"))
        .expect("msg step");
    assert_eq!(msg.value, "***SECRET***", "msg must render the marker");
    assert!(msg.tainted, "msg step must be tainted");
    let root = steps.last().expect("root step");
    assert!(root.tainted, "container holding material is tainted");
}

#[test]
fn step_trace_redacts_env_interpolation_and_substrings() {
    use std::collections::HashMap;
    let mut env = HashMap::new();
    env.insert("INTERP_PASS".to_string(), "ENVSECRET-interp".to_string());
    let module = parse(
        "p = env.INTERP_PASS\nmsg = \"pre-${p}-post\"\nk = secret(p)\nname = \"svc\"\n",
        "debug-interp-env",
    )
    .expect("parse");
    let (_, _, steps) = Engine::hermetic()
        .with_env(env)
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("ENVSECRET"),
        "env interpolation leaked: {rendered}"
    );
    let msg = steps
        .iter()
        .find(|step| step.path.as_deref() == Some("msg"))
        .expect("msg step");
    assert_eq!(msg.value, "***SECRET***");
    assert!(msg.tainted);
    // Benign neighbours keep their value in the container render.
    let root = steps.last().expect("root step");
    assert_eq!(root.value["name"], "svc", "over-redacted: {:?}", root.value);
}

#[test]
fn reduction_fails_closed_for_dotted_and_malformed_env_calls() {
    // Shapes a reader still treats as a secret call but the canonical
    // grammar does not: dot runs beside `env`, `secret env(`, and
    // extra env arguments with or without a comma. The exported
    // minimal case must never carry the binding literal.
    let cases = [
        "loc = \"hunter2-BIND\"\ntoken = secret.env.(loc)\nnoise = 1\n",
        "loc = \"hunter2-BIND\"\ntoken = secret.env..(loc)\nnoise = 1\n",
        "loc = \"hunter2-BIND\"\ntoken = secret..(loc)\nnoise = 1\n",
        "loc = \"hunter2-BIND\"\ntoken = secret env(loc)\nnoise = 1\n",
        "loc = \"hunter2-BIND\"\ntoken = secret..env(\"NAME\", default = loc)\nnoise = 1\n",
        "loc = \"hunter2-BIND\"\ntoken = secret.env(\"NAME\" default = loc)\nnoise = 1\n",
        "token = secret.env.(\"hunter2-A\")\nnoise = 1\n",
        "token = secret..(\"hunter2-A\")\nnoise = 1\n",
    ];
    for src in cases {
        let code = first_code(src).unwrap_or_else(|| "E001".to_string());
        let report = debug::minimal_case(src, &code, 250);
        let rendered = report.to_string();
        assert!(
            !rendered.contains("hunter2"),
            "leak for {src:?}: {rendered}"
        );
    }
}

#[test]
fn reduction_never_deletes_a_malformed_secret_call() {
    // If the call line were deletable, the reducer could keep the
    // binding literal and strand it. The call's lines are protected.
    for src in [
        "loc = \"hunter2-KEEP\"\ntoken = secret.env.(loc)\nnoise = 1\n",
        "loc = \"hunter2-KEEP\"\ntoken = secret..(loc)\nnoise = 1\n",
        "loc = \"hunter2-KEEP\"\ntoken = secret env(loc)\nnoise = 1\n",
        "loc = \"hunter2-KEEP\"\ntoken = secret.env(\"NAME\" default = loc)\nnoise = 1\n",
    ] {
        let code = first_code(src).unwrap_or_else(|| "E001".to_string());
        let (reduced, _) = debug::reduce(src, &code, 250);
        assert!(
            reduced.contains("secret"),
            "secret call was deleted for {src:?}: {reduced:?}"
        );
    }
}

#[test]
fn malformed_secret_punctuation_still_redacts() {
    let cases = [
        "token = secret..env(\"NAME\", default = \"hunter2-A\")\nnoise = 1\n",
        "token = secret..(\"hunter2-D\")\nnoise = 1\n",
        "token = secret env(\"hunter2-E\")\nnoise = 1\n",
    ];
    for src in cases {
        let code = first_code(src).unwrap_or_else(|| "E001".to_string());
        assert!(emits(src, &code), "target {code} missing for {src:?}");
        let report = debug::minimal_case(src, &code, 100);
        let rendered = report.to_string();
        assert!(
            !rendered.contains("hunter2"),
            "leak for {src:?}: {rendered}"
        );
    }
}
