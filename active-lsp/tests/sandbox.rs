//! L-P28 smoke: LSP analysis runs from an InputClosure.

use active_core::sandbox::InputClosure;
use active_ls::analysis;
use lsp_types::Url;
use std::collections::BTreeMap;

fn closure(document: &str, env: BTreeMap<String, Option<String>>) -> InputClosure {
    InputClosure::new(
        std::sync::Arc::<str>::from(document),
        "sandbox.active",
        BTreeMap::new(),
        env,
        None,
    )
    .expect("closure")
}

#[test]
fn analysis_with_closure_uses_declared_env_and_refuses_other_reads() {
    let uri = Url::parse("active://sandbox/main.active").unwrap();
    let mut env = BTreeMap::new();
    env.insert("P".to_string(), Some("closed".to_string()));
    let doc = analysis::analysis_with_closure(
        uri.clone(),
        "p = env.P ?: \"default\"\n".to_string(),
        1,
        closure("p = env.P ?: \"default\"\n", env),
    )
    .expect("analysis");
    let value = doc.value.as_ref().expect("value");
    assert_eq!(active_core::convert::value_to_json(value)["p"], "closed");

    let refused = analysis::analysis_with_closure(
        uri,
        "p = env.OTHER\n".to_string(),
        1,
        closure("p = env.OTHER\n", BTreeMap::new()),
    )
    .expect("analysis");
    assert!(refused.eval_diags.iter().any(|d| d.code == "E048"));
}

#[test]
fn steps_snapshot_with_closure_is_closure_backed() {
    let mut env = BTreeMap::new();
    env.insert("P".to_string(), Some("closed".to_string()));
    let snapshot =
        analysis::steps_snapshot_with_closure(closure("p = env.P ?: \"default\"\n", env), None);
    let text = snapshot.to_string();
    assert!(!text.contains("error"), "{snapshot}");
    assert!(
        text.contains("closed"),
        "closure env missing from steps: {snapshot}"
    );

    let refused =
        analysis::steps_snapshot_with_closure(closure("p = env.OTHER\n", BTreeMap::new()), None);
    assert!(
        refused.to_string().contains("E048"),
        "undeclared env must surface E048, not a clean snapshot: {refused}"
    );
}
