//! Agent-native assist surfaces (CHECKLIST F.003/F.004):
//! deterministic failure explanations plus minimal diagnostic-driven
//! @rename/@patch directives. Same input, same output, or it is a bug.

use active_core::convert::{
    explain_all, explain_code, explain_structured, suggest_patches, suggest_patches_structured,
    PatchKind,
};
use active_core::{parse, Engine};

fn strict_diags(src: &str) -> (active_core::Module, Vec<active_core::Diagnostic>) {
    let module = parse(src, "assist").expect("must parse");
    let mut engine = Engine::new();
    engine.resilient = false;
    match engine.evaluate(&module) {
        Ok((_, diags)) => (module, diags),
        Err(active_core::EvalError::ValidationFailed { diagnostics }) => (module, diagnostics),
        Err(e) => panic!("unexpected engine failure: {e:?}"),
    }
}

#[test]
fn explain_codes_are_stable_and_value_free() {
    assert!(explain_code("T003").contains("format"));
    assert!(explain_code("T004").contains("Missing required key"));
    assert!(explain_code("T005").contains("closed table"));
    assert!(explain_code("H005").contains("@rename"));
    // Unknown codes fall back instead of inventing advice.
    assert!(explain_code("ZZZ").contains("No canned guidance"));
    for line in explain_all(&[("T003".to_string(), Some("a.b".to_string()), None)]) {
        assert!(line.starts_with("T003 a.b: "), "{line}");
        assert!(!line.contains("not-an-email"), "value leaked: {line}");
    }
}

#[test]
fn explain_structured_matches_text_field_for_field() {
    // Text and JSON renderings must never drift: same items in,
    // same content out, only the shape differs.
    let items = vec![(
        "T003".to_string(),
        Some("a.b".to_string()),
        Some("2:1-2:9".to_string()),
    )];
    let text = explain_all(&items);
    let structured = explain_structured(&items);
    assert_eq!(structured.len(), 1);
    let e = &structured[0];
    assert_eq!(e.code, "T003");
    assert_eq!(e.path.as_deref(), Some("a.b"));
    assert_eq!(e.span.as_deref(), Some("2:1-2:9"));
    assert!(!e.guidance.is_empty());
    assert_eq!(text, vec![e.line()]);
}

#[test]
fn explain_covers_every_documented_code() {
    // The explainer's contract: these 20 codes always get canned
    // guidance; anything else must fall back explicitly. Adding a
    // diagnostic code requires updating this list AND the table —
    // silent fallback for a known code is a bug.
    let covered = [
        "E001", "E002", "E003", "E004", "E006", "E007", "E011", "E019", "E020", "E023", "E024",
        "E025", "E032", "T001", "T002", "T003", "T004", "T005", "W004", "W005", "W013", "H005",
        "H006", "H008", "S001", "B001", "B002", "B003", "B004", "B005", "C040", "C041", "C042",
        "C043", "C044", "E040",
    ];
    assert_eq!(covered.len(), 36);
    for code in covered {
        let g = explain_code(code);
        assert!(
            !g.contains("No canned guidance"),
            "{code} regressed to fallback"
        );
        assert!(!g.is_empty(), "{code} has empty guidance");
    }
    // Unknown codes still fall back instead of inventing advice.
    assert!(explain_code("ZZZ-999").contains("No canned guidance"));
}

#[test]
fn suggest_structured_carries_kind_and_path() {
    let (module, diags) = strict_diags(
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n",
    );
    let out = suggest_patches_structured(&module, &diags);
    let rename = out
        .iter()
        .find(|s| s.kind == PatchKind::Rename)
        .expect("rename present");
    assert_eq!(rename.path, "Host");
    assert_eq!(rename.directive, "@rename(\"Host\", \"host\")");
    let fill = out
        .iter()
        .find(|s| s.kind == PatchKind::Fill)
        .expect("fill present");
    assert_eq!(fill.path, "port");
    assert_eq!(fill.directive, "@patch(\"port\", 8080)");
    // String form stays byte-identical to the structured directives.
    let flat = suggest_patches(&module, &diags);
    let structured: Vec<String> = out.into_iter().map(|s| s.directive).collect();
    assert_eq!(flat, structured);
}

#[test]
fn suggest_emits_safe_rename_and_static_fill() {
    let (module, diags) = strict_diags(
        "schema = { closed = true, host = String, port = Int(default = 8080) }\nHost = \"x\"\n",
    );
    let patches = suggest_patches(&module, &diags);
    assert!(
        patches.iter().any(|p| p == "@rename(\"Host\", \"host\")"),
        "{patches:?}"
    );
    assert!(
        patches.iter().any(|p| p == "@patch(\"port\", 8080)"),
        "{patches:?}"
    );
}

#[test]
fn suggest_fill_preserves_literal_interpolation_syntax() {
    // A single-quoted schema default is literal; the fill directive
    // must keep `${…}` literal too, not turn it into interpolation.
    let (module, diags) = strict_diags("schema = { path = String(default = '${HOME}/bin') }\n");
    let patches = suggest_patches(&module, &diags);
    assert!(
        patches
            .iter()
            .any(|p| p == "@patch(\"path\", '${HOME}/bin')"),
        "{patches:?}"
    );
}

#[test]
fn suggest_refuses_unrepresentable_control_characters() {
    let src = format!("schema = {{ path = String(default = '{}') }}\n", '\u{1}');
    let (module, diags) = strict_diags(&src);
    let patches = suggest_patches(&module, &diags);
    assert!(
        !patches.iter().any(|p| p.contains("@patch")),
        "a control character has no faithful directive form: {patches:?}"
    );
}

#[test]
fn suggest_float_fill_keeps_decimal_point() {
    // A Float default must fill with a parseable float literal: the
    // `{f:?}` formatting keeps the decimal point so 1.5 never degrades
    // into the Int literal 1, and the directive re-parses as Float.
    let (module, diags) = strict_diags("schema = { ratio = Float(default = 1.5) }\n");
    let patches = suggest_patches(&module, &diags);
    assert!(
        patches.iter().any(|p| p == "@patch(\"ratio\", 1.5)"),
        "{patches:?}"
    );
}

#[test]
fn suggest_emits_nothing_without_exact_mapping() {
    // Unsafe typo (no safe repair) and bare T003 (no directive form).
    let (module, diags) = strict_diags(
        "schema = { closed = true, host = String, contact = String(format = \"email\") }\nhots = \"x\"\ncontact = \"not-an-email\"\n",
    );
    let patches = suggest_patches(&module, &diags);
    assert!(
        !patches.iter().any(|p| p.contains("hots")),
        "unsafe typo must not map: {patches:?}"
    );
    assert!(
        !patches.iter().any(|p| p.contains("contact")),
        "T003 has no directive form: {patches:?}"
    );
    // H005 informational typos (diagnostic path already fixed) yield
    // nothing: there is no rename left to emit.
    use active_core::diag::{Diagnostic, Repair, RepairKind, Severity};
    let fixed = Diagnostic {
        severity: Severity::Info,
        code: "H005".into(),
        message: "unknown key 'hots', did you mean 'host'?".into(),
        path: Some("host".into()),
        span: None,
        repair: Some(Repair {
            original: "hots".into(),
            suggested: "host".into(),
            kind: RepairKind::KeyTypo,
            safe: true,
            side_effects: Vec::new(),
            goal: None,
            steps: Vec::new(),
        }),
        confidence: 1.0,
        file: None,
    };
    let (module, _) = strict_diags("port = 1\n");
    assert!(suggest_patches(&module, &[fixed]).is_empty());
}

#[test]
fn suggest_fills_requires_triggered_missing_keys() {
    // A requires-triggered missing key with a static default is just
    // as mechanical as a strict-explicit fill: the trigger is present,
    // the value is known, write it out.
    let (module, diags) = strict_diags(
        "schema = { tls = Bool, cert = String(requires = \"tls\", default = \"none\") }\ntls = true\n",
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == "T004" && d.message.contains("required by present")),
        "{diags:?}"
    );
    let patches = suggest_patches(&module, &diags);
    assert!(
        patches.iter().any(|p| p == "@patch(\"cert\", \"none\")"),
        "{patches:?}"
    );
}

#[test]
fn assist_outputs_are_byte_identical_across_runs() {
    // New outputs obey the determinism invariant: same input, same
    // bytes, every time. Run everything twice and compare exactly.
    let src = "schema = { closed = true, host = String, port = Int(default = 8080), contact = String(format = \"email\") }\nHost = \"x\"\ncontact = \"not-an-email\"\n";
    let run_once = || {
        let (module, diags) = strict_diags(src);
        let items: Vec<(String, Option<String>, Option<String>)> = diags
            .iter()
            .map(|d| (d.code.clone(), d.path.clone(), None))
            .collect();
        (
            explain_all(&items),
            explain_structured(&items)
                .iter()
                .map(|e| format!("{}|{}|{}", e.code, e.guidance.len(), e.line().len()))
                .collect::<Vec<_>>(),
            suggest_patches(&module, &diags),
            suggest_patches_structured(&module, &diags)
                .iter()
                .map(|s| format!("{:?}|{}|{}", s.kind, s.path, s.directive))
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(run_once(), run_once());
}

#[test]
fn suggest_ignores_secret_and_deprecated_defaults() {
    // A secret default has no source-text literal form: suggesting it
    // would print the secret into a directive. Must yield nothing.
    let (module, diags) =
        strict_diags("schema = { t = Secret(default = secret(\"s3cr3t-d1\")) }\n");
    let patches = suggest_patches(&module, &diags);
    assert!(
        patches.is_empty(),
        "secret default must not map: {patches:?}"
    );
    // A deprecated field must never gain a fill even when a T004
    // names it explicitly: omitting it is the migration path, and
    // suggesting its re-insertion would contradict the deprecation.
    // (Strict eval exempts deprecated defaults entirely, so this
    // crafts the diagnostic directly to pin fill_for's own guard.)
    use active_core::diag::{Diagnostic, Severity};
    let module = parse(
        "schema = { closed = true, old = String(default = \"o\", deprecated = \"gone\") }\n",
        "assist",
    )
    .expect("must parse");
    let missing_key = Diagnostic {
        severity: Severity::Error,
        code: "T004".into(),
        message: "schema violation at 'old': missing required key (strict mode requires explicit values despite a schema default)".into(),
        path: Some("old".into()),
        span: None,
        repair: None,
        confidence: 1.0,
        file: None,
    };
    let patches = suggest_patches(&module, &[missing_key]);
    assert!(
        patches.is_empty(),
        "deprecated field must not map: {patches:?}"
    );
}

#[test]
fn suggest_handles_nested_paths_and_dynamic_defaults() {
    // Nested safe typo resolves against the full dotted path.
    let (module, diags) = strict_diags(
        "schema = { closed = true, server = Table { closed = true, host = String } }\n[server]\nHost = \"x\"\n",
    );
    let patches = suggest_patches(&module, &diags);
    assert!(
        patches
            .iter()
            .any(|p| p == "@rename(\"server.Host\", \"server.host\")"),
        "{patches:?}"
    );
    // A computed default has no source-text form: no @patch.
    let (module, diags) = strict_diags("schema = { port = Int(default = 1 + 1) }\n");
    let patches = suggest_patches(&module, &diags);
    assert!(
        !patches.iter().any(|p| p.contains("port")),
        "dynamic default must not map: {patches:?}"
    );
}

#[test]
fn suggest_skips_a_literal_default_that_is_a_secret_copy() {
    // Review 67: a default equal to a material value must not become a
    // patch directive.
    let (module, diags) = strict_diags(
        "s = secret(\"K\")\nschema = { tls = Bool, cert = String(requires = \"tls\", default = \"K\") }\ntls = true\n",
    );
    assert!(diags.iter().any(|d| d.code == "T004"), "{diags:?}");
    let patches = suggest_patches(&module, &diags);
    assert!(
        !patches.iter().any(|p| p.contains("@patch(\"cert\"")),
        "{patches:?}"
    );
    // Control: a benign default still produces the fill.
    let (module, diags) = strict_diags(
        "s = secret(\"K\")\nschema = { tls = Bool, cert = String(requires = \"tls\", default = \"none\") }\ntls = true\n",
    );
    let patches = suggest_patches(&module, &diags);
    assert!(
        patches.iter().any(|p| p.contains("@patch(\"cert\"")),
        "{patches:?}"
    );
}
