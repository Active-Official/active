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

/// Minimal case with a real file context. Malformed-secret fixtures do
/// not parse strictly, so the independent target only exists on the
/// recovering path (`minimal_case` with `file_ctx`).
fn minimal_with_file(src: &str, code: &str) -> serde_json::Value {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "active-debug-r13-{}-{}.active",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, src).expect("write debug fixture");
    let report = debug::minimal_case_with(src, code, 300, Some(&path));
    let _ = std::fs::remove_file(&path);
    report
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
    let report = debug::replay(&module, None, None).expect("replay");
    assert_eq!(report["ok"], true);
    assert_eq!(report["steps_match"], true);
    let hash = report["hash"].as_str().expect("hash").to_string();
    let expected = debug::replay(&module, Some(&hash), None).expect("replay");
    assert_eq!(expected["expected_match"], true);
    let mismatch = debug::replay(&module, Some("sha256:00"), None).expect("replay");
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
fn step_trace_drops_a_materialised_payload_carrying_a_secret_literal() {
    // Review 44: base64 payload bytes cannot be matched by a literal
    // sweep, so a materialised blob step must drop the payload.
    let raw = "hunter2-debug-probe";
    let src =
        format!("m = \"{raw}\"\nk = secret(m)\nf = blob(\"text/plain\", \"pre {raw} post\")\n");
    let module = parse(&src, "debug-blob-payload").expect("parse");
    let mut engine = Engine::new();
    engine.materialise_blobs = true;
    let (_, _, steps) = engine.evaluate_with_steps(&module).expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(!rendered.contains("payload_b64"), "{rendered}");
    assert!(!rendered.contains(raw), "{rendered}");
    let text = debug::render_steps(&steps);
    assert!(!text.contains("payload_b64"), "{text}");
    assert!(!text.contains(raw), "{text}");
    // Review 45: the redacted identity must make the step tainted, or
    // `--break taint` misses it.
    assert!(
        steps.iter().any(|step| step.kind == "blob" && step.tainted),
        "redacted blob step must be tainted: {steps:?}"
    );
    assert!(
        steps
            .iter()
            .any(|step| step.kind == "table" && step.tainted),
        "container step with a redacted blob must be tainted: {steps:?}"
    );
}

#[test]
fn step_trace_redacts_copies_of_a_runtime_secret() {
    // Review 45: the sensitive set must be complete before rendering,
    // so a copy evaluated before (or after) the secret still redacts.
    use std::collections::HashMap;
    let raw = "sd-token-order-r45";
    let src = format!(
        "copy = \"pre {raw} post\"\nf = blob(\"text/plain\", \"pre {raw} post\")\nm = env.TOKEN\nk = secret(m)\n"
    );
    let module = parse(&src, "debug-runtime-order").expect("parse");
    let mut env = HashMap::new();
    env.insert("TOKEN".to_string(), raw.to_string());
    let mut engine = Engine::hermetic().with_env(env);
    engine.materialise_blobs = true;
    let (_, _, steps) = engine.evaluate_with_steps(&module).expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(!rendered.contains(raw), "{rendered}");
    assert!(!rendered.contains("payload_b64"), "{rendered}");
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
fn container_steps_mark_non_string_material_leaves() {
    // A number used as a secret locator is material. Containers keep
    // their shape but the material leaf is marked by path, not by the
    // runtime string sweep (which only sees strings).
    let module =
        parse("let x = 42\nk = secret(x)\nt = { a = x }\n", "debug-number").expect("parse");
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let table = steps
        .iter()
        .find(|step| step.path.as_deref() == Some("t"))
        .expect("t step");
    assert_eq!(table.value["a"], "***SECRET***", "{:?}", table.value);
    let root = steps.last().expect("root step");
    assert_eq!(root.value["t"]["a"], "***SECRET***", "{:?}", root.value);
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("42"),
        "non-string material leaked into a step: {rendered}"
    );
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
    // The target lives on an independent line, so removing the call
    // line would not break the target by itself — only the protected
    // line set keeps the call (and its fail-closed redaction) present.
    for src in [
        "loc = \"hunter2-KEEP\"\ntoken = secret.env.(loc)\nbad = 1 + true\n",
        "loc = \"hunter2-KEEP\"\ntoken = secret..(loc)\nbad = 1 + true\n",
        "loc = \"hunter2-KEEP\"\ntoken = secret env(loc)\nbad = 1 + true\n",
        "loc = \"hunter2-KEEP\"\ntoken = secret.env(\"NAME\" default = loc)\nbad = 1 + true\n",
    ] {
        let report = minimal_with_file(src, "T001");
        let reduced = report["reduced"].as_str().unwrap_or("");
        assert!(
            reduced.contains("secret"),
            "secret call was deleted for {src:?}: {reduced:?}"
        );
        assert!(
            !report.to_string().contains("hunter2"),
            "leak for {src:?}: {report}"
        );
    }
}

#[test]
fn reduction_is_case_insensitive_and_protects_dotted_calls() {
    for (src, code) in [
        ("token = Secret(\"hunter2-CAP\")\nbad = 1 + true\n", "E002"),
        (
            "loc = \"hunter2-STR\" extra\ntoken = secret.Env(loc)\nbad = 1 + true\n",
            "E003",
        ),
        (
            "loc = \"hunter2-MIX\" extra\ntoken = Secret.Env(loc)\nbad = 1 + true\n",
            "E002",
        ),
    ] {
        let report = debug::minimal_case(src, code, 250);
        let rendered = report.to_string();
        assert!(
            !rendered.contains("hunter2"),
            "leak for {src:?}: {rendered}"
        );
        let reduced = report["reduced"].as_str().unwrap_or("");
        assert!(
            reduced.to_ascii_lowercase().contains("secret"),
            "secret call was deleted for {src:?}: {reduced:?}"
        );
    }
}

#[test]
fn step_trace_marks_material_leaves_in_let_containers() {
    let module = parse(
        "let p = 42\nlet t = { a = p, b = 7 }\nk = secret(p)\n",
        "debug-let",
    )
    .expect("parse");
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let table = steps
        .iter()
        .find(|step| step.path.as_deref() == Some("t"))
        .expect("t step");
    assert_eq!(table.value["a"], "***SECRET***", "{:?}", table.value);
    assert_eq!(table.value["b"], 7, "{:?}", table.value);
    assert!(table.tainted);
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("42"),
        "let-container material leaked: {rendered}"
    );
}

#[test]
fn step_trace_marks_every_read_of_a_secret_bound_env_var() {
    use std::collections::HashMap;
    let mut env = HashMap::new();
    env.insert("R13SAME".to_string(), "ENVSECRET-same".to_string());
    let module = parse(
        "s = env.R13SAME\ny = env.R13SAME\nk = secret(s)\n",
        "debug-env-alias",
    )
    .expect("parse");
    let (_, _, steps) = Engine::hermetic()
        .with_env(env)
        .evaluate_with_steps(&module)
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("ENVSECRET"),
        "an aliased env read leaked: {rendered}"
    );
    for path in ["s", "y"] {
        let step = steps
            .iter()
            .find(|step| step.path.as_deref() == Some(path))
            .unwrap_or_else(|| panic!("{path} step"));
        assert_eq!(step.value, "***SECRET***", "{path}: {step:?}");
        assert!(step.tainted, "{path} must be tainted");
    }
}

#[test]
fn reduction_fails_closed_for_bare_case_variants_and_env_calls() {
    for src in [
        "loc = \"hunter2-CASE\" + 1\ntoken = Secret(loc)\nbad = 1 + true\n",
        "loc = \"hunter2-UP\" + 1\ntoken = SECRET(loc)\nbad = 1 + true\n",
        "loc = \"hunter2-SELF4\" + 1\ntoken = secret.env(f(loc))\nbad = 1 + true\n",
    ] {
        let report = minimal_with_file(src, "T001");
        let rendered = report.to_string();
        assert!(
            !rendered.contains("hunter2"),
            "leak for {src:?}: {rendered}"
        );
        let reduced = report["reduced"].as_str().unwrap_or("");
        assert!(
            reduced.to_ascii_lowercase().contains("secret"),
            "call line was deleted for {src:?}: {reduced:?}"
        );
    }
}

#[test]
fn reduction_fails_closed_when_the_literal_line_must_survive() {
    // The schema violation lives on the literal line, so the reducer
    // must keep it; only the fail-closed sweep can protect the value.
    for src in [
        "schema = { loc = Int }\nloc = \"hunter2-X\"\ntoken = SECRET(loc)\n",
        "schema = { loc = Int }\nloc = \"hunter2-Y\"\ntoken = secret.env(loc)\n",
    ] {
        let report = minimal_with_file(src, "T002");
        let rendered = report.to_string();
        assert!(
            !rendered.contains("hunter2"),
            "leak for {src:?}: {rendered}"
        );
        let reduced = report["reduced"].as_str().unwrap_or("");
        assert!(
            reduced.contains("loc"),
            "the target line must survive: {reduced:?}"
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

#[test]
fn steps_redact_a_profile_selected_env_read() {
    // A profile may place a binding's env read at a material path. The
    // step at that path (and the internal profile containers) redact
    // it even though the shared node is not globally marked.
    let src = "let e = env.R29_TRACE_LIVE\nlet p = env.R29_TRACE_SRC\nk = secret(p)\nn = p\nactive_profile = \"p\"\nprofile \"p\" { n = e }\n";
    let mut env = std::collections::HashMap::new();
    env.insert("R29_TRACE_LIVE".to_string(), "trace-live-r29".to_string());
    env.insert("R29_TRACE_SRC".to_string(), "trace-source-r29".to_string());
    let (_, _, steps) = Engine::new()
        .with_env(env)
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("trace-live-r29"),
        "profile env leaked: {rendered}"
    );
    assert!(
        steps
            .iter()
            .any(|step| step.path.as_deref() == Some("n") && step.tainted),
        "material step marked: {rendered}"
    );
}

#[test]
fn steps_keep_a_shared_benign_data_node_visible() {
    // The same node used as a benign data value stays visible in its
    // own step; only the material profile path redacts.
    let src = "let s = env.R29_TRACE_SRC2\nk = secret(s)\nn = s\ny = \"BENIGN-DATA\"\nactive_profile = \"p\"\nprofile \"p\" { n = y }\n";
    let mut env = std::collections::HashMap::new();
    env.insert("R29_TRACE_SRC2".to_string(), "trace-source-r29".to_string());
    let (_, _, steps) = Engine::new()
        .with_env(env)
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        rendered.contains("BENIGN-DATA"),
        "benign data hidden: {rendered}"
    );
    assert!(
        !rendered.contains("trace-source-r29"),
        "source leaked: {rendered}"
    );
}

#[test]
fn steps_follow_a_renamed_material_path() {
    // F4 (review 30): the trace path overlay follows Set/Remove/Rename;
    // a renamed material profile path is labeled with its new name.
    let src = "let p = env.R30_TRACE\nk = secret(p)\nn = p\nactive_profile = \"p\"\nprofile \"p\" { n = \"BENIGN-PROFILE\" }\n@rename(\"n\", \"m\")\n";
    let mut env = std::collections::HashMap::new();
    env.insert("R30_TRACE".to_string(), "trace-source-r30".to_string());
    let (_, _, steps) = Engine::new()
        .with_env(env)
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("trace-source-r30"),
        "source leaked: {rendered}"
    );
    assert!(
        steps.iter().any(|step| step.path.as_deref() == Some("m")),
        "renamed path missing from steps: {rendered}"
    );
    // Steps are evaluation-order events; the pre-transform path for a\n    // node evaluated before the patch is expected, and the overlay for\n    // the renamed material path is m.\n
}

#[test]
fn steps_mark_a_material_path_as_tainted() {
    // N3 (review 31): a step whose own path is material reports the
    // marker AND tainted=true, so `--break taint` still selects it.
    let src = "let p = env.R31_TRACE\nk = secret(p)\nn = p\nactive_profile = \"p\"\nprofile \"p\" { n = \"BENIGN-PROFILE\" }\n";
    let mut env = std::collections::HashMap::new();
    env.insert("R31_TRACE".to_string(), "trace-source-r31".to_string());
    let (_, _, steps) = Engine::new()
        .with_env(env)
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let step = steps
        .iter()
        .find(|step| step.path.as_deref() == Some("n"))
        .expect("material step");
    assert_eq!(step.value, serde_json::json!("***SECRET***"), "{step:?}");
    assert!(step.tainted, "material path must be a taint hit: {step:?}");
}

#[test]
fn steps_follow_a_nested_rename() {
    // N2 (review 31): the trace overlay renames within the same parent.
    let src = "let p = env.R31_NEST\nk = secret(p)\nactive_profile = \"p\"\nprofile \"p\" { a = { x = p } }\n@rename(\"a.x\", \"zz\")\n";
    let mut env = std::collections::HashMap::new();
    env.insert("R31_NEST".to_string(), "nested-trace-r31".to_string());
    let (_, _, steps) = Engine::new()
        .with_env(env)
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("nested-trace-r31"),
        "source leaked: {rendered}"
    );
    assert!(
        steps
            .iter()
            .any(|step| step.path.as_deref() == Some("a.zz")),
        "same-parent path missing: {rendered}"
    );
    assert!(
        !steps.iter().any(|step| step.path.as_deref() == Some("zz")),
        "bare destination fabricated: {rendered}"
    );
}

#[test]
fn steps_flag_a_profile_override_of_a_base_path_as_tainted() {
    // N3 residual (review 32): a profile overrides a base data path
    // with a material value; the natural data path wins, and the step
    // must still report tainted=true.
    let src = "k = secret(\"S3\")\nm = \"benign-base\"\nactive_profile = \"p\"\nprofile \"p\" { m = secret(\"S3\") }\n";
    let (_, _, steps) = Engine::new()
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let step = steps
        .iter()
        .find(|step| step.path.as_deref() == Some("m"))
        .expect("base path step");
    assert_eq!(step.value, serde_json::json!("***SECRET***"), "{step:?}");
    assert!(step.tainted, "marker step must be a taint hit: {step:?}");
}

#[test]
fn steps_follow_a_nested_computed_profile_rename() {
    // N2a residual (review 32): the fail-closed computed branch also
    // updates the trace overlay to the same-parent destination.
    let src = "let p = env.R32_NEST\nk = secret(p)\nsel = env.R32_SEL\nactive_profile = sel\nprofile \"b\" { a = { b = { x = p } } }\n@rename(\"a.b.x\", \"q\")\n";
    let mut env = std::collections::HashMap::new();
    env.insert("R32_NEST".to_string(), "nested-trace-r32".to_string());
    env.insert("R32_SEL".to_string(), "b".to_string());
    let (_, _, steps) = Engine::new()
        .with_env(env)
        .evaluate_with_steps(&module(src))
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("nested-trace-r32"),
        "source leaked: {rendered}"
    );
    assert!(
        steps
            .iter()
            .any(|step| step.path.as_deref() == Some("a.b.q")),
        "same-parent destination missing: {rendered}"
    );
}

#[test]
fn steps_do_not_fabricate_an_array_element_rename() {
    // N2b residual (review 32): renaming an array element is a W004
    // no-op; the overlay must not move it to `arr.zz`.
    let src =
        "let p = env.R32_ARR\nk = secret(p)\narr = [{ x = p }]\n@rename(\"arr[0]\", \"zz\")\n";
    let mut env = std::collections::HashMap::new();
    env.insert("R32_ARR".to_string(), "array-trace-r32".to_string());
    let (_, _, steps) = Engine::new()
        .with_env(env)
        .evaluate_with_steps(&module(src))
        .expect("resilient evaluate");
    assert!(
        !steps.iter().any(
            |step| step.path.as_deref() == Some("arr.zz") || step.path.as_deref() == Some("zz")
        ),
        "array-element rename fabricated a path: {steps:?}"
    );
}

#[test]
fn steps_shift_material_paths_after_an_array_remove() {
    // N2c residual (review 32): removing an element shifts later
    // indices; the trace overlay follows.
    let src = "let p = env.R32_SHIFT\nk = secret(p)\narr = [{ x = \"A\" }, { y = \"B\" }]\nactive_profile = \"p\"\nprofile \"p\" { arr = [{ x = \"A\" }, { y = p }] }\n@remove(\"arr[0]\")\n";
    let mut env = std::collections::HashMap::new();
    env.insert("R32_SHIFT".to_string(), "shift-trace-r32".to_string());
    let (_, _, steps) = Engine::new()
        .with_env(env)
        .evaluate_with_steps(&module(src))
        .expect("resilient evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(
        !rendered.contains("shift-trace-r32"),
        "source leaked: {rendered}"
    );
    assert!(
        steps
            .iter()
            .any(|step| step.path.as_deref() == Some("arr[0].y")),
        "shifted material path missing: {rendered}"
    );
    assert!(
        !steps
            .iter()
            .any(|step| step.path.as_deref() == Some("arr[1].y")),
        "stale index kept: {rendered}"
    );
}

#[test]
fn steps_are_deterministic_for_a_node_material_at_two_paths() {
    // F4 (review 33): the overlay picks the smallest path, so replay
    // and trace labels do not depend on map iteration order.
    let src = "let p = env.R33_SHARED\nk = secret(p)\nactive_profile = \"p\"\nprofile \"p\" { x = p, y = p }\n";
    let mut env = std::collections::HashMap::new();
    env.insert("R33_SHARED".to_string(), "shared-trace-r33".to_string());
    let run = || {
        let (_, _, steps) = Engine::new()
            .with_env(env.clone())
            .evaluate_with_steps(&module(src))
            .expect("evaluate");
        serde_json::to_string(&steps).expect("json")
    };
    let first = run();
    for _ in 0..5 {
        assert_eq!(first, run(), "trace must be byte-stable");
    }
    assert!(
        first.contains("\"path\":\"x\""),
        "smallest path wins: {first}"
    );
    assert!(
        !first.contains("shared-trace-r33"),
        "source leaked: {first}"
    );
}

#[test]
fn replay_respects_a_caller_budget() {
    // Review 51: replay is an input-bounded evaluation, not a trusted
    // pass; a budget that rejects the document rejects replay too.
    let module = parse("x = 1\ny = 2\n", "replay-budget").expect("parse");
    let budget = active_core::Budget {
        max_nodes: 1,
        ..Default::default()
    };
    let error = debug::replay_with_budget(&module, None, None, &budget, false)
        .expect_err("node budget must bind replay");
    assert!(error.contains("node"), "{error}");
}

#[test]
fn step_events_never_carry_blob_payloads() {
    // Review 51: a materialised tree feeds the taint check, but a step
    // is identity-only, so payload bytes never enter a trace.
    let payload = "B".repeat(64);
    let src = format!("b = blob(\"text/plain\", \"{payload}\")\nc = b\n");
    let module = parse(&src, "step-payload").expect("parse");
    let mut engine = Engine::hermetic();
    engine.materialise_blobs = true;
    engine.taint_probe = true;
    let (value, _, steps, _) = engine
        .evaluate_with_steps_and_seeds(&module)
        .expect("evaluate");
    let rendered = serde_json::to_string(&steps).expect("json");
    assert!(!rendered.contains("payload_b64"), "{rendered}");
    let blob_steps: Vec<_> = steps.iter().filter(|step| step.kind == "blob").collect();
    assert_eq!(blob_steps.len(), 2, "{rendered}");
    for step in blob_steps {
        assert!(step.value.to_string().contains("\"size\":64"), "{rendered}");
        assert!(!step.value.to_string().contains("payload"), "{rendered}");
    }
    assert!(matches!(value, active_core::Value::Table(_)));
}

#[test]
fn minimize_redacts_duplicate_values_beyond_plain_tokens() {
    // Review 55: comments, interpolation, multi-line dedent, escaped
    // dollars, and base64 encodings of a known secret must not survive
    // in a reduction bundle.
    let cases = [
        "schema = { t = Int }\nt = secret(\"hunter2-COMMENT\") # rotate hunter2-COMMENT\n",
        "schema = { t = Int }\nt = secret(\"abc-${'resolved'}\")\ndup = \"abc-resolved\"\n",
        "schema = { t = Int }\nt = secret(\"\"\"\n    hunter2-ML\n\"\"\")\ndup = \"hunter2-ML\"\n",
        "schema = { t = Int }\nt = secret(\"pa\\$\\$word\")\ndup = \"pa$$word\"\n",
        "schema = { t = Int }\nt = secret(\"hunter2-b64\")\ndup = \"aHVudGVyMi1iNjQ=\"\n",
        "schema = { t = Int }\nt = secret(\"hunter2-b64\")\ndup = \"aHVudGVyMi1iNjQ\"\n",
        "schema = { t = Int }\nt = secret(\"hunter2-b64\")\ndup = \"aHVudGVyMi1iNjQ\" # unpadded\n",
    ];
    let raw_values = [
        "hunter2-COMMENT",
        "abc-resolved",
        "hunter2-ML",
        "pa$$word",
        "hunter2-b64",
        "aHVudGVyMi1iNjQ=",
        "aHVudGVyMi1iNjQ",
    ];
    for source in cases {
        let report = debug::minimal_case(source, "T002", 0);
        let reduced = report["reduced"].as_str().expect("reduced");
        for raw in raw_values {
            assert!(!reduced.contains(raw), "{raw} survived: {reduced}");
        }
    }
}

#[test]
fn minimize_keeps_env_names_and_unrelated_strings() {
    // Review 55: canonical env names are not material, and a short
    // secret must not rewrite unrelated longer strings.
    let source = "schema = { t = Int }\nt = secret(\"HUNTER2_ENVNAME\")\nother = secret.env(\"HUNTER2_ENVNAME\")\nname = \"banana\"\nshort = secret(\"a\")\n";
    let report = debug::minimal_case(source, "T002", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(
        reduced.contains("secret.env(\"HUNTER2_ENVNAME\")"),
        "{reduced}"
    );
    assert!(reduced.contains("\"banana\""), "{reduced}");
}

#[test]
fn minimize_is_hermetic_without_a_file_context() {
    // Review 55: string reduction must not resolve includes from the
    // process working directory.
    let report = debug::minimal_case("include \"probe_inc.active\"\nx = 1\n", "E015", 20);
    assert_eq!(report["target_present"], true, "{report}");
}

#[test]
fn strict_minimize_does_not_evaluate_parse_broken_modules() {
    // Review 55: the strict CLI never evaluates a module with parse
    // errors, so a runtime code cannot be "present".
    let source = "schema = { x = Int }\nx = 1 + \"s\"\ny = [1, 2}\n";
    let report = debug::minimal_case(source, "T001", 50);
    assert_eq!(report["target_present"], false, "{report}");
}

#[test]
fn minimize_never_evaluates_parse_errors_in_any_mode() {
    // Review 56: the shipped CLI aborts on parse errors in both modes.
    let source = "x = 1 + \"s\"\n$bad = 2\n";
    let strict =
        debug::minimal_case_with_options(source, "T001", 0, None, debug::DebugOptions::default());
    assert_eq!(strict["target_present"], false, "{strict}");
    let resilient = debug::minimal_case_with_options(
        source,
        "T001",
        0,
        None,
        debug::DebugOptions {
            resilient: true,
            ..Default::default()
        },
    );
    assert_eq!(resilient["target_present"], false, "{resilient}");
}

#[test]
fn minimize_does_not_exempt_a_string_literal_named_secret() {
    // Review 56: only the keyword may open an env call.
    let source = "t = secret(\"hunter2\")\nx = \"secret\".env(\"hunter2\")\n";
    let report = debug::minimal_case(source, "T002", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(!reduced.contains("hunter2"), "{reduced}");
}

#[test]
fn minimize_keeps_a_grouped_env_name() {
    // Review 56: grouping parentheses are canonical.
    let source = "schema = { t = Int }\nt = secret(\"HUNTER2_ENVNAME\")\nother = secret.env((\"HUNTER2_ENVNAME\"))\n";
    let report = debug::minimal_case(source, "T002", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(
        reduced.contains("secret.env((\"HUNTER2_ENVNAME\"))"),
        "{reduced}"
    );
}

#[test]
fn minimize_covers_interpolation_mlsingle_and_dotted_env_tricks() {
    // Review 57 edge cases: nested interpolation, triple-single-quoted
    // dedent, a dotted member named secret, and a genuine U+E000.
    let cases = [
        "schema = { t = Int }\nt = secret(\"hunter2-nested\")\ndup = \"prefix ${ \\\"hunter2-nested\\\" } suffix\"\n",
        "schema = { t = Int }\nt = secret(\"hunter2-mls\")\ndup = '''\n  hunter2-mls'''\n",
        "schema = { t = Int }\na = { secret = 1 }\nt = secret(\"hunter2-dot\")\ndup = a.secret.env(\"hunter2-dot\")\n",
    ];
    let raws = ["hunter2-nested", "hunter2-mls", "hunter2-dot"];
    for (source, raw) in cases.iter().zip(raws) {
        let report = debug::minimal_case(source, "T002", 0);
        let reduced = report["reduced"].as_str().expect("reduced");
        assert!(!reduced.contains(raw), "{raw} survived: {reduced}");
    }
}

#[test]
fn minimize_keeps_a_genuine_sentinel_character() {
    // Review 57: a literal U+E000 is user data, not an escaped dollar.
    let source =
        "schema = { t = Int }\nt = secret(\"pa\\u{E000}word\")\ndup = \"pa\\u{E000}word\"\n";
    let report = debug::minimal_case(source, "T002", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(!reduced.contains("pa$word"), "{reduced}");
}

#[test]
fn minimize_redacts_a_comment_after_a_secret_gap() {
    // Review 57: comment offsets use the source line index, not the
    // normalized one.
    let source = "t = secret(\"hunter2-one\")\ng = secret\n(\"hunter2-two\")\n# note hunter2-one leaked here\n";
    let report = debug::minimal_case(source, "T002", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(!reduced.contains("hunter2-one"), "{reduced}");
}

#[test]
fn minimize_collects_seeds_on_a_hard_error() {
    // Review 57: a type error after the secret node still leaves its
    // text available for duplicate redaction.
    let source = "t = secret(\"abc-${1}\")\ndup = \"abc-1\"\nbad = 1 + \"s\"\n";
    let report = debug::minimal_case(source, "T001", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(!reduced.contains("abc-1"), "{reduced}");
}

#[test]
fn minimize_redacts_escaped_dollar_forms_and_noncanonical_env_names() {
    // Both `"""` and `"..."` unmark `\$`. The source spelling is the
    // assertion: an unredacted token still reads `pa\$word`.
    for dup in ["\"pa\\$word\"", "\"\"\"pa\\$word\"\"\""] {
        let source = format!("schema = {{ t = Int }}\nt = secret(\"pa$word\")\ndup = {dup}\n");
        let report = debug::minimal_case(&source, "T002", 0);
        let reduced = report["reduced"].as_str().expect("reduced");
        assert!(!reduced.contains("pa\\$word"), "{dup} survived: {reduced}");
        assert!(!reduced.contains("pa$word"), "{dup} survived: {reduced}");
    }

    // Fail-closed sweep: a canonical env name stays readable so the
    // bundle can reproduce the lookup, while a non-identifier name is
    // not trusted and is redacted.
    let malformed = "t = secret(\"hunter2-x\")\nok = secret.env(\"ACTIVE_R59_OK\")\nbad = secret.env(\"canary-1\")\nboom = 1 + \"s\"\n";
    let report = debug::minimal_case(malformed, "T001", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(reduced.contains("ACTIVE_R59_OK"), "{reduced}");
    assert!(!reduced.contains("canary-1"), "{reduced}");
}

#[test]
fn minimize_redacts_a_secret_embedded_in_a_plain_duplicate() {
    // Review 59: a plain token went through exact matching only, so a
    // secret inside surrounding text survived. Substring matches apply
    // at four bytes and up.
    let source = "schema = { t = Int }\nt = secret(\"pa$word\")\ndup = \"pre pa$word post\"\nshort = \"pre x post\"\n";
    let report = debug::minimal_case(source, "T002", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(!reduced.contains("pa$word"), "{reduced}");
    assert!(reduced.contains("pre x post"), "{reduced}");
}

#[test]
fn minimize_keeps_include_profile_and_patch_structure_on_a_hard_error() {
    // Review 58/59: a fail-closed sweep must not replace the literals
    // that resolve an include, select a profile, or apply a patch.
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "active-debug-r59-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create dir");
    std::fs::write(dir.join("inc.active"), "included = 1\n").expect("write include");
    let main = dir.join("main.active");
    let source = "include \"inc.active\"\nt = secret(\"hunter2-inc\")\nactive_profile = \"prod\"\nprofile \"prod\" { level = \"debug\" }\n@patch(\"level\", \"info\")\nkeep = \"benign-value\"\nboom = 1 + \"s\"\n";
    std::fs::write(&main, source).expect("write main");
    let report = debug::minimal_case_with(source, "T001", 0, Some(&main));
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(reduced.contains("include \"inc.active\""), "{reduced}");
    assert!(reduced.contains("active_profile = \"prod\""), "{reduced}");
    assert!(reduced.contains("profile \"prod\""), "{reduced}");
    // Only the patch path is structure; its value is data.
    assert!(reduced.contains("@patch(\"level\""), "{reduced}");
    assert!(!reduced.contains("\"info\""), "{reduced}");
    assert!(!reduced.contains("hunter2-inc"), "{reduced}");
    assert!(!reduced.contains("benign-value"), "{reduced}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn minimize_redacts_a_patch_value() {
    // Review 60: the second `@patch` argument is a value, not a path.
    let source = "t = secret(\"hunter2-patch\")\n@patch(\"t\", \"hunter2-patch\" + 1)\n";
    let report = debug::minimal_case(source, "T001", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(!reduced.contains("hunter2-patch"), "{reduced}");
    assert!(reduced.contains("@patch(\"t\""), "{reduced}");
}

#[test]
fn minimize_redacts_extra_directive_arguments() {
    // Review 61: only path arguments are structural. A second string on
    // `@remove` is data even though the directive accepts one path.
    let source = "s = secret(\"hunter2-x\")\n@remove(\"a\", \"hunter2-x\")\n";
    let report = debug::minimal_case(source, "E019", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(reduced.contains("@remove(\"a\""), "{reduced}");
    assert!(!reduced.contains("hunter2-x"), "{reduced}");
}

#[test]
fn minimize_keeps_a_patch_path_before_a_parenthesis_value() {
    // Review 61: a value that spells `(` is a string token, not nesting.
    let source = "s = secret(\"hunter2-x\")\nx = 1 + \"a\"\n@patch(\"a\", \"(\")\n";
    let report = debug::minimal_case(source, "T001", 0);
    let reduced = report["reduced"].as_str().expect("reduced");
    assert!(reduced.contains("@patch(\"a\""), "{reduced}");
}
