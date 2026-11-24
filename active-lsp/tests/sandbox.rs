//! L-P28 smoke: LSP analysis runs from an InputClosure.

use active_core::sandbox::InputClosure;
use active_ls::analysis;
use lsp_types::Url;
use std::collections::BTreeMap;

fn closure_with_includes(
    document: &str,
    includes: &[(&str, &str, &str)],
    env: BTreeMap<String, Option<String>>,
) -> InputClosure {
    use std::sync::Arc;
    fn hex32(text: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).expect("hex");
        }
        out
    }
    let includes = includes
        .iter()
        .map(|(path, text, sha)| {
            (
                path.to_string(),
                active_core::sandbox::IncludeFile {
                    bytes: Arc::from(*text),
                    sha256: hex32(sha),
                },
            )
        })
        .collect();
    InputClosure::new(
        Arc::<str>::from(document),
        "sandbox.active",
        includes,
        env,
        None,
    )
    .expect("closure")
}

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

#[test]
fn b4_parse_failure_is_a_doc_with_published_e048() {
    let temp = std::env::temp_dir().join("active-b4");
    std::fs::create_dir_all(&temp).expect("temp dir");
    let file_uri = Url::from_file_path(temp.join("main.active")).expect("file uri");
    for uri in [file_uri, Url::parse("untitled:active-b4").unwrap()] {
        let document = "x = 1\ninclude \"missing.active\"\n";
        let doc = analysis::analysis_with_closure(
            uri.clone(),
            document.to_string(),
            3,
            closure(document, BTreeMap::new()),
        )
        .expect("a parse-time closure failure is a Doc, not Err");
        assert!(doc.sandboxed, "closure Doc must carry the marker");
        assert!(
            doc.parse_diags.iter().any(|d| d.code == "E048"),
            "parse_diags missing E048 for {uri}: {:?}",
            doc.parse_diags
        );
        assert!(
            doc.module.arena.iter().any(|node| matches!(
                node,
                active_core::ir::Node::Literal(active_core::ir::Value::Int(1))
            )),
            "recovered tree lost `x = 1` for {uri}"
        );
        let published = analysis::diagnostics_for(&doc);
        let e048 = published
            .iter()
            .find(|d| format!("{:?}", d.code).contains("E048"))
            .unwrap_or_else(|| panic!("E048 not published for {uri}: {published:?}"));
        assert_eq!(e048.range.start.line, 1, "E048 line for {uri}: {e048:?}");
        assert!(
            e048.range.start.character > 0,
            "E048 column lost for {uri}: {e048:?}"
        );
    }
}

#[test]
fn b4_env_e048_publishes_at_its_span_on_a_non_file_uri() {
    let uri = Url::parse("untitled:active-b4-env").unwrap();
    let document = "p = env.OTHER\n";
    let doc = analysis::analysis_with_closure(
        uri,
        document.to_string(),
        5,
        closure(document, BTreeMap::new()),
    )
    .expect("analysis");
    let published = analysis::diagnostics_for(&doc);
    let e048 = published
        .iter()
        .find(|d| format!("{:?}", d.code).contains("E048"))
        .unwrap_or_else(|| panic!("env E048 not published: {published:?}"));
    assert!(
        e048.range.start.character > 0,
        "env E048 span lost: {e048:?}"
    );
}

#[test]
fn b4_syntax_failure_is_a_doc_with_parse_diags() {
    let uri = Url::parse("untitled:active-b4-syntax").unwrap();
    let document = "bad = 2e8\n";
    let doc = analysis::analysis_with_closure(
        uri,
        document.to_string(),
        4,
        closure(document, BTreeMap::new()),
    )
    .expect("syntax failure is a Doc");
    assert!(
        doc.parse_diags.iter().any(|d| d.code == "E002"),
        "expected E002: {:?}",
        doc.parse_diags
    );
    let published = analysis::diagnostics_for(&doc);
    let e002 = published
        .iter()
        .find(|d| format!("{:?}", d.code).contains("E002"))
        .unwrap_or_else(|| panic!("E002 not published: {published:?}"));
    assert!(e002.range.start.character > 0, "{e002:?}");
}

#[test]
fn b4_include_content_diagnostic_never_lands_on_the_parent_or_disk() {
    let uri = Url::parse("untitled:active-b4-include-content").unwrap();
    let document = "include \"lib.active\"\n";
    let mut includes = BTreeMap::new();
    let bytes: std::sync::Arc<str> = std::sync::Arc::from("bad = 2e8\n");
    includes.insert(
        "lib.active".to_string(),
        active_core::IncludeFile {
            bytes,
            sha256: [
                0xdd, 0xbe, 0x50, 0xdf, 0xc2, 0xe4, 0xcc, 0x3a, 0x52, 0x8e, 0x11, 0x0e, 0xb6, 0x7d,
                0xeb, 0x76, 0xc7, 0x64, 0x9b, 0xbe, 0xdf, 0x93, 0x54, 0xed, 0x97, 0x23, 0x00, 0xba,
                0x0a, 0x12, 0x72, 0x43,
            ],
        },
    );
    let closure = InputClosure::new(
        std::sync::Arc::<str>::from(document),
        "main.active",
        includes,
        BTreeMap::new(),
        None,
    )
    .expect("closure");
    let doc = analysis::analysis_with_closure(uri.clone(), document.to_string(), 6, closure)
        .expect("analysis");
    let groups = analysis::diagnostics_by_file(&doc, &|_: &str| None);
    let parent = groups
        .iter()
        .find(|(group_uri, _)| group_uri == &uri)
        .expect("parent group");
    assert!(
        !parent
            .1
            .iter()
            .any(|d| format!("{:?}", d.code).contains("E002")),
        "include diagnostic mis-ranged onto parent: {parent:?}"
    );
    assert!(
        groups.iter().any(|(group_uri, diags)| {
            group_uri.as_str().starts_with("active-sandbox://include/")
                && diags
                    .iter()
                    .any(|d| format!("{:?}", d.code).contains("E002"))
        }),
        "include diagnostic missing its own identity: {groups:?}"
    );
}

#[test]
fn b4_sandboxed_doc_offers_no_disk_reparsing_repair() {
    let temp = std::env::temp_dir().join("active-b4-repair");
    std::fs::create_dir_all(&temp).expect("dir");
    let main = temp.join("main.active");
    let lib = temp.join("lib.active");
    let document = "include \"lib.active\"\nport = \"x\"\n";
    std::fs::write(&main, document).expect("main");
    // Canary include on disk: a different schema the closure never saw.
    std::fs::write(&lib, "schema {\n  port = String\n}\n").expect("lib");
    let uri = Url::from_file_path(&main).expect("file uri");
    let closure = closure_with_includes(
        document,
        &[(
            "lib.active",
            "schema {\n  port = Int\n}\n",
            "eb471d52b30d20d501644ef088ce8ee7e4bf974d081c661502fb2dcb1e471a59",
        )],
        BTreeMap::new(),
    );
    let doc =
        analysis::analysis_with_closure(uri, document.to_string(), 7, closure).expect("analysis");
    let actions = analysis::code_actions(
        &doc,
        lsp_types::Range {
            start: lsp_types::Position {
                line: 0,
                character: 0,
            },
            end: lsp_types::Position {
                line: 2,
                character: 0,
            },
        },
    );
    assert!(
        actions.is_empty(),
        "sandboxed Doc offered disk-influenced code actions: {actions:?}"
    );
}

#[test]
fn b4_recovery_never_reads_a_disk_include() {
    let temp = std::env::temp_dir().join("active-b4-recovery");
    std::fs::create_dir_all(&temp).expect("dir");
    let main = temp.join("main.active");
    let lib = temp.join("lib.active");
    let document = "include \"lib.active\"\nx = 1\n";
    std::fs::write(&main, document).expect("main");
    std::fs::write(&lib, "canary = \"CANARY-LP28-B4\"\nleaked = 1\n").expect("canary");
    let uri = Url::from_file_path(&main).expect("file uri");
    let doc = analysis::analysis_with_closure(
        uri.clone(),
        document.to_string(),
        8,
        closure(document, BTreeMap::new()),
    )
    .expect("parse failure Doc");
    assert!(
        !doc.module.arena.iter().any(|node| matches!(
            node,
            active_core::ir::Node::Literal(active_core::ir::Value::Int(1))
                if doc.module.node_spans.values().any(|_| true)
        )) || !doc
            .module
            .arena
            .iter()
            .any(|node| matches!(node, active_core::ir::Node::Ident(name) if name == "leaked")),
        "recovery read the disk include"
    );
    assert!(!doc
        .module
        .arena
        .iter()
        .any(|node| matches!(node, active_core::ir::Node::Ident(name) if name == "leaked")));
    assert!(
        !doc.module.arena.iter().any(|node| matches!(
            node,
            active_core::ir::Node::Literal(active_core::ir::Value::String(text))
                if text.contains("CANARY-LP28-B4")
        )),
        "recovery read disk include content"
    );
    assert!(
        doc.module.include_pins.is_empty(),
        "recovery resolved an include"
    );
    let leaked_in_root = match doc.module.root.and_then(|root| doc.module.get(root)) {
        Some(active_core::ir::Node::Table { fields, .. }) => fields.contains_key("leaked"),
        _ => false,
    };
    assert!(
        !leaked_in_root,
        "recovery brought an include field into the tree"
    );
    assert!(
        doc.module
            .node_files
            .values()
            .all(|stamp| stamp == uri.as_str()),
        "recovery used a host path: {:?}",
        doc.module.node_files
    );
}

#[test]
fn f2_include_named_main_active_keeps_its_own_identity() {
    let uri = Url::parse("untitled:f2-root").unwrap();
    let document = "include \"main.active\"\n";
    let closure = closure_with_includes(
        document,
        &[(
            "main.active",
            "bad = 2e8\n",
            "ddbe50dfc2e4cc3a528e110eb67deb76c7649bbedf9354ed972300ba0a127243",
        )],
        BTreeMap::new(),
    );
    let doc = analysis::analysis_with_closure(uri.clone(), document.to_string(), 9, closure)
        .expect("Doc");
    let groups = analysis::diagnostics_by_file(&doc, &|_: &str| None);
    let parent = groups.iter().find(|(u, _)| u == &uri).expect("parent");
    assert!(
        !parent
            .1
            .iter()
            .any(|d| format!("{:?}", d.code).contains("E002")),
        "include main.active aliased the parent: {parent:?}"
    );
    assert!(
        groups
            .iter()
            .any(|(u, diags)| u.as_str().contains("/include/")
                && diags
                    .iter()
                    .any(|d| format!("{:?}", d.code).contains("E002"))),
        "missing include identity group: {groups:?}"
    );
}

#[test]
fn f3_synthetic_include_uris_are_parent_scoped() {
    let mut uris = Vec::new();
    for name in ["doc-a", "doc-b"] {
        let uri = Url::parse(&format!("untitled:{name}")).unwrap();
        let document = "include \"lib.active\"\n";
        let closure = closure_with_includes(
            document,
            &[(
                "lib.active",
                "bad = 2e8\n",
                "ddbe50dfc2e4cc3a528e110eb67deb76c7649bbedf9354ed972300ba0a127243",
            )],
            BTreeMap::new(),
        );
        let doc =
            analysis::analysis_with_closure(uri, document.to_string(), 1, closure).expect("Doc");
        let groups = analysis::diagnostics_by_file(&doc, &|_: &str| None);
        let include_uri = groups
            .iter()
            .find(|(u, _)| u.as_str().contains("/include/"))
            .map(|(u, _)| u.clone())
            .expect("include group");
        uris.push(include_uri);
    }
    assert_ne!(uris[0], uris[1], "include URI collided across documents");
}

#[test]
fn f1_include_nodes_never_resolve_to_the_parent_uri() {
    let uri = Url::parse("untitled:f1").unwrap();
    let document = "base = 1\ninclude \"lib.active\"\n";
    let closure = closure_with_includes(
        document,
        &[(
            "lib.active",
            "copy = base\n",
            "8299b14d96298bfec24f6e2b15f9915294b2827522d6d8fe92cfb0357b621b1e",
        )],
        BTreeMap::new(),
    );
    let doc = analysis::analysis_with_closure(uri.clone(), document.to_string(), 1, closure)
        .expect("Doc");
    let include_node = doc
        .module
        .node_files
        .iter()
        .find(|(_, stamp)| stamp.as_str() == "include:lib.active")
        .map(|(id, _)| *id)
        .expect("include node stamp");
    let node_uri = analysis::uri_for_node(&doc, include_node);
    assert_ne!(node_uri, uri, "include node resolved to the parent");
    assert_eq!(node_uri.scheme(), "active-sandbox");
}

#[test]
fn f4_profile_refusal_stays_err() {
    let document = "a = 1\n";
    let closure = active_core::InputClosure::new(
        std::sync::Arc::<str>::from(document),
        "sandbox.active",
        BTreeMap::new(),
        BTreeMap::new(),
        Some("missing".to_string()),
    )
    .expect("closure");
    let error = match analysis::analysis_with_closure(
        Url::parse("untitled:f4").unwrap(),
        document.to_string(),
        1,
        closure,
    ) {
        Ok(_) => panic!("a profile refusal is not a recovery Doc"),
        Err(diags) => diags,
    };
    assert!(error.iter().any(|d| d.code == "E048"));
}

#[test]
fn lsp_capabilities_narrow_the_held_set() {
    let document = "p = env.SECRET\n";
    let doc = analysis::analysis_with_closure_capabilities(
        Url::parse("untitled:lsp-caps").unwrap(),
        document.to_string(),
        1,
        closure(document, BTreeMap::new()),
        active_core::CapabilitySet::parse_list(["evaluate"]).unwrap(),
    )
    .expect("analysis");
    assert!(
        doc.eval_diags.iter().any(|d| d.code == "E049"),
        "{:?}",
        doc.eval_diags
    );
}

#[test]
fn lsp_steps_narrow_the_held_set() {
    // L-P35: the debugger stepper carries the same held-authority set as
    // the analyzer. `env.SECRET` needs read-secrets, so an evaluate-only
    // set must surface E049 instead of a snapshot. The env var is declared
    // so the two cases differ only by the held authority (an undeclared
    // read is E048 for either set).
    let document = "p = env.SECRET\n";
    let declared = || {
        let mut env = BTreeMap::new();
        env.insert("SECRET".to_string(), Some("s3cr3t".to_string()));
        env
    };
    let denied = analysis::steps_snapshot_with_closure_capabilities(
        closure(document, declared()),
        None,
        active_core::CapabilitySet::parse_list(["evaluate"]).unwrap(),
    );
    assert_eq!(denied["code"], "E049", "{denied}");
    assert!(denied["error"].is_string(), "{denied}");

    let allowed = analysis::steps_snapshot_with_closure_capabilities(
        closure(document, declared()),
        None,
        active_core::CapabilitySet::parse_list(["evaluate", "read-secrets"]).unwrap(),
    );
    assert!(allowed.get("error").is_none(), "{allowed}");
    assert!(allowed["steps"].is_array(), "{allowed}");

    // The convenience wrapper holds the full authority set: it agrees with
    // the granting set above.
    let wide = analysis::steps_snapshot_with_closure(closure(document, declared()), None);
    assert!(wide.get("error").is_none(), "{wide}");
}
