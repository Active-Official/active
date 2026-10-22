//! Agent-native assist surfaces (CHECKLIST F.003/F.004):
//! deterministic failure explanations plus minimal diagnostic-driven
//! @rename/@patch directives. Same input, same output, or it is a bug.

use active_core::convert::{explain_all, explain_code, suggest_patches};
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
        }),
        confidence: 1.0,
        file: None,
    };
    let (module, _) = strict_diags("port = 1\n");
    assert!(suggest_patches(&module, &[fixed]).is_empty());
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
