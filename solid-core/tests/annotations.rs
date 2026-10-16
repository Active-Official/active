//! Annotation parsing tests (C-033, spec/annotations.md).
//!
//! Proves on the real shipped path that `# @intent`, `# @constraint`,
//! and `# @ai_immutable` comments attach to the following binding as
//! IR data with paths and spans: payload forms, stacking, tables and
//! lets, block rules (trailing comments, blank lines, plain comments),
//! E021 diagnostics, `value` rewriting, and formatter preservation.
//! Enforcement is C-034.

use solid_core::fmt::format;
use solid_core::ir::{AnnotationKind, Node};
use solid_core::{parse, parse_file, Engine, EvalError, ParseOptions, Style};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("annotations")
        .join(name)
}

fn parse_ok(src: &str) -> solid_core::Module {
    parse(src, "annotations").unwrap_or_else(|d| panic!("parse failed: {d:?}"))
}

fn root_field(module: &solid_core::Module, key: &str) -> solid_core::NodeId {
    let root = module.root.expect("root");
    match module.get(root).expect("root node") {
        Node::Table { fields } => *fields.get(key).unwrap_or_else(|| panic!("key {key}")),
        other => panic!("root must be a table, got {other:?}"),
    }
}

fn annotations_of(
    module: &solid_core::Module,
    id: solid_core::NodeId,
) -> &[solid_core::Annotation] {
    module
        .annotations
        .get(&id)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// All Ident("value") nodes reachable from `id` (must be none after
/// rewriting) and Ref-to-target nodes (must exist).
fn value_refs(
    module: &solid_core::Module,
    id: solid_core::NodeId,
    target: solid_core::NodeId,
) -> (usize, usize) {
    let mut idents = 0usize;
    let mut refs = 0usize;
    let mut stack = vec![id];
    let mut seen = std::collections::HashSet::new();
    while let Some(nid) = stack.pop() {
        if !seen.insert(nid) {
            continue;
        }
        match module.get(nid) {
            Some(Node::Ident(name)) if name == "value" => idents += 1,
            Some(Node::Ref(t)) if *t == target => refs += 1,
            Some(Node::Binary { lhs, rhs, .. }) => stack.extend([lhs, rhs].into_iter().copied()),
            Some(Node::Unary { expr, .. }) => stack.push(*expr),
            Some(Node::Interpolate { parts }) => {
                for part in parts {
                    if let solid_core::ir::InterpPart::Expr(e) = part {
                        stack.push(*e);
                    }
                }
            }
            Some(Node::Array { elements }) => stack.extend(elements.iter().copied()),
            Some(Node::Table { fields }) => stack.extend(fields.values().copied()),
            Some(Node::Semantic { inner, .. }) => stack.push(*inner),
            _ => {}
        }
    }
    (idents, refs)
}

#[test]
fn intent_attaches_with_text_path_and_span() {
    let module = parse_ok("# @intent: \"Public port\"\nport = 8080\n");
    let id = root_field(&module, "port");
    let anns = annotations_of(&module, id);
    assert_eq!(anns.len(), 1, "{anns:?}");
    assert!(matches!(&anns[0].kind, AnnotationKind::Intent(s) if s == "Public port"));
    assert_eq!(anns[0].path, "port");
    let span = &anns[0].span;
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
}

#[test]
fn all_three_annotations_stack_on_one_binding() {
    let src =
        "# @intent: \"Workers\"\n# @constraint: value >= 1\n# @ai_immutable: true\nworkers = 7\n";
    let module = parse_ok(src);
    let id = root_field(&module, "workers");
    let anns = annotations_of(&module, id);
    assert_eq!(anns.len(), 3, "{anns:?}");
    assert!(matches!(&anns[0].kind, AnnotationKind::Intent(_)));
    assert!(matches!(&anns[1].kind, AnnotationKind::Constraint { .. }));
    assert!(matches!(&anns[2].kind, AnnotationKind::AiImmutable(true)));
    assert!(anns.iter().all(|a| a.path == "workers"));
}

#[test]
fn constraint_rewrites_value_to_a_reference() {
    let module = parse_ok("# @constraint: value >= 1 && value <= 10\nport = 5\n");
    let id = root_field(&module, "port");
    let anns = annotations_of(&module, id);
    assert_eq!(anns.len(), 1);
    let expr = match &anns[0].kind {
        AnnotationKind::Constraint { source, expr } => {
            assert_eq!(source, "value >= 1 && value <= 10");
            *expr
        }
        other => panic!("constraint expected, got {other:?}"),
    };
    let (idents, refs) = value_refs(&module, expr, id);
    assert_eq!(idents, 0, "`value` must be fully rewritten");
    assert_eq!(refs, 2, "both occurrences reference the binding");
}

#[test]
fn annotations_attach_to_tables_with_dotted_paths() {
    let src = "# @intent: \"Web tier\"\n# @ai_immutable: false\n[server]\nport = 1\n";
    let module = parse_ok(src);
    let id = root_field(&module, "server");
    let anns = annotations_of(&module, id);
    assert_eq!(anns.len(), 2, "{anns:?}");
    assert!(anns.iter().all(|a| a.path == "server"));
    // Nested keys get nested paths.
    let nested = parse_ok("[server]\n# @intent: \"P\"\nport = 1\n");
    let server = root_field(&nested, "server");
    let port = match nested.get(server).expect("table") {
        Node::Table { fields } => fields["port"],
        _ => panic!("table expected"),
    };
    let anns = annotations_of(&nested, port);
    assert_eq!(anns.len(), 1);
    assert_eq!(anns[0].path, "server.port");
}

#[test]
fn annotations_attach_to_lets_by_name() {
    let module = parse_ok("# @constraint: value > 0\nlet limit = 5\nport = 1\n");
    let id = module.bindings["limit"];
    let anns = annotations_of(&module, id);
    assert_eq!(anns.len(), 1);
    assert_eq!(anns[0].path, "limit");
}

#[test]
fn single_quotes_and_escapes_in_intent() {
    let module = parse_ok("# @intent: 'it\\'s \"quoted\"'\nport = 1\n");
    let id = root_field(&module, "port");
    let anns = annotations_of(&module, id);
    assert!(
        matches!(&anns[0].kind, AnnotationKind::Intent(s) if s == "it's \"quoted\""),
        "{anns:?}"
    );
}

#[test]
fn blank_line_breaks_the_block_but_plain_comments_do_not() {
    // Blank line: no attachment.
    let module = parse_ok("# @intent: \"orphan\"\n\nport = 1\n");
    assert!(module.annotations.is_empty(), "blank line must break");
    // Plain comments inside the block are skipped, annotations attach.
    let module = parse_ok("# A note.\n# @intent: \"kept\"\nport = 1\n");
    let id = root_field(&module, "port");
    assert_eq!(annotations_of(&module, id).len(), 1);
}

#[test]
fn trailing_annotation_belongs_to_its_own_line() {
    let src = "host = \"h\" # @intent: \"for host\"\nport = 1\n";
    let module = parse_ok(src);
    assert!(
        annotations_of(&module, root_field(&module, "port")).is_empty(),
        "trailing comment must not attach upward"
    );
    assert!(
        annotations_of(&module, root_field(&module, "host")).is_empty(),
        "trailing comments never annotate"
    );
}

#[test]
fn unknown_at_kinds_are_plain_comments() {
    let module = parse_ok("# @todo fix later\n# @note x\nport = 1\n");
    assert!(module.annotations.is_empty());
}

#[test]
fn malformed_annotations_are_e021() {
    for (src, fragment) in [
        ("# @intent: \"open\nport = 1\n", "unterminated"),
        ("# @intent:\nport = 1\n", "string"),
        ("# @ai_immutable: yes\nport = 1\n", "'true' or 'false'"),
        ("# @constraint:\nport = 1\n", "empty predicate"),
        (
            "# @constraint: value > 1 extra\nport = 1\n",
            "after the predicate",
        ),
        ("# @intent missing colon\nport = 1\n", "':'"),
    ] {
        let diags = parse(src, "annotations").expect_err("must fail");
        let d = diags
            .iter()
            .find(|d| d.code == "E021")
            .unwrap_or_else(|| panic!("E021 expected for {src:?}: {diags:?}"));
        assert!(d.message.contains(fragment), "{d:?}");
        assert!(d.span.is_some(), "E021 must carry a span");
    }
}

#[test]
fn free_text_intent_and_bare_immutable_flag() {
    // Compatibility forms (pre-annotation documents used these as
    // plain comments): prose intents and a bare immutable flag.
    let module = parse_ok("# @intent: application identity\n# @ai_immutable\nport = 1\n");
    let id = root_field(&module, "port");
    let anns = annotations_of(&module, id);
    assert_eq!(anns.len(), 2, "{anns:?}");
    assert!(
        matches!(&anns[0].kind, AnnotationKind::Intent(s) if s == "application identity"),
        "{anns:?}"
    );
    assert!(matches!(&anns[1].kind, AnnotationKind::AiImmutable(true)));
}

#[test]
fn truncated_predicates_keep_expression_errors() {
    // A cut-off predicate is an expression error (precise, rebased),
    // not a blanket E021: `# @constraint: value >` ends mid-compare.
    let diags = parse("# @constraint: value >\nport = 1\n", "annotations").expect_err("must fail");
    assert!(!diags.is_empty());
    assert!(
        diags.iter().all(|d| d.span.is_some()),
        "spans must rebase onto the comment: {diags:?}"
    );
    assert_eq!(diags[0].span.as_ref().unwrap().line, 1);
}

#[test]
fn schemas_never_take_annotations() {
    let module = parse_ok("# @intent: \"not a schema doc\"\nschema = { port = Int }\nport = 1\n");
    assert!(module.annotations.is_empty(), "schema blocks stay metadata");
}

#[test]
fn file_parses_attach_root_annotations() {
    // Regression: root-file comments stamped None never matched the
    // file-identified block scan, so parse_file dropped main-file
    // annotations while includes kept theirs.
    let module =
        parse_file(&fixture("main_own.solid"), ParseOptions::default()).expect("must parse");
    let root = module.root.expect("root");
    let port = match module.get(root).expect("table") {
        Node::Table { fields } => fields["port"],
        _ => panic!("table expected"),
    };
    let anns = annotations_of(&module, port);
    assert_eq!(anns.len(), 1, "{anns:?}");
    assert!(matches!(&anns[0].kind, AnnotationKind::Intent(s) if s == "Main file contract"));
}

#[test]
fn annotations_survive_formatting_in_both_styles() {
    let src = "# @intent: \"Public port\"\n# @constraint: value >= 1\nport = 8080\n";
    for style in [Style::Brace, Style::Indent] {
        let once = format(&parse_ok(src), style);
        assert!(
            once.contains("# @intent: \"Public port\""),
            "intent kept:\n{once}"
        );
        assert!(
            once.contains("# @constraint: value >= 1"),
            "constraint kept:\n{once}"
        );
        // And they still attach after a fmt round-trip.
        let module = parse_ok(&once);
        let id = root_field(&module, "port");
        assert_eq!(annotations_of(&module, id).len(), 2, "re-attach:\n{once}");
    }
}

// ------------------------------------------------------------------
// C-034 enforcement.
// ------------------------------------------------------------------

fn strict_engine() -> Engine {
    let mut engine = Engine::new();
    engine.resilient = false;
    engine
}

fn eval_strict(src: &str) -> Result<serde_json::Value, Vec<solid_core::Diagnostic>> {
    let module = parse(src, "annotations").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    match strict_engine().evaluate(&module) {
        Ok((value, diags)) => {
            assert!(diags.is_empty(), "unexpected diags: {diags:?}");
            let json = solid_core::convert::value_to_pretty_json(&value);
            Ok(serde_json::from_str(&json).expect("valid json"))
        }
        Err(EvalError::ValidationFailed { diagnostics }) => Err(diagnostics),
        Err(e) => panic!("unexpected error: {e:?}"),
    }
}

fn eval_resilient(src: &str) -> (serde_json::Value, Vec<solid_core::Diagnostic>) {
    let module = parse(src, "annotations").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    let (value, diags) = Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("resilient must succeed: {e:?}"));
    let json = solid_core::convert::value_to_pretty_json(&value);
    (serde_json::from_str(&json).expect("valid json"), diags)
}

#[test]
fn satisfied_constraints_pass_silently_in_both_modes() {
    let src = "# @constraint: value >= 1 && value <= 10\nport = 5\n";
    let v = eval_strict(src).expect("must pass strict");
    assert_eq!(v["port"], 5);
    let (_, diags) = eval_resilient(src);
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn violated_constraints_fail_strict_and_collect_resilient() {
    let src = "# @constraint: value >= 1 && value <= 10\nport = 99\n";
    let diags = eval_strict(src).expect_err("must fail strict");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T006");
    assert_eq!(diags[0].path.as_deref(), Some("port"));
    // The message quotes the constraint source and the actual value.
    assert!(
        diags[0].message.contains("value >= 1 && value <= 10"),
        "{:?}",
        diags[0]
    );
    assert!(diags[0].message.contains("99"), "{:?}", diags[0]);
    // Value span: the violation sits on the value's line.
    assert_eq!(diags[0].span.as_ref().expect("span").line, 2);
    let (v, diags) = eval_resilient(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T006");
    assert_eq!(v["port"], 99, "resilient exports unchanged");
}

#[test]
fn non_boolean_predicates_are_t006() {
    let diags = eval_strict("# @constraint: value + 1\nport = 5\n").expect_err("must fail");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, "T006");
    assert!(
        diags[0].message.contains("must evaluate to a boolean"),
        "{:?}",
        diags[0]
    );
}

#[test]
fn constraints_see_let_bindings() {
    let src = "let max = 10\n# @constraint: value <= max\nport = 7\n";
    let v = eval_strict(src).expect("let reference must resolve");
    assert_eq!(v["port"], 7);
    let diags = eval_strict("let max = 10\n# @constraint: value <= max\nport = 99\n")
        .expect_err("must fail");
    assert_eq!(diags[0].code, "T006");
}

#[test]
fn secret_actuals_render_as_secret() {
    let src = "let pw = secret(\"s3cr3t-hunter2\")\n# @constraint: value == \"x\"\ncount = pw\n";
    let diags = eval_strict(src).expect_err("must fail");
    assert_eq!(diags[0].code, "T006");
    for d in &diags {
        assert!(!d.message.contains("s3cr3t"), "{d:?}");
        assert!(!d.message.contains("hunter2"), "{d:?}");
    }
    assert!(diags[0].message.contains("secret"), "{:?}", diags[0]);
}

#[test]
fn long_actuals_truncate() {
    let long = "a".repeat(200);
    let src = format!("# @constraint: value == \"x\"\nname = \"{long}\"\n");
    let diags = eval_strict(&src).expect_err("must fail");
    assert!(diags[0].message.contains("…"), "{:?}", diags[0]);
    assert!(diags[0].message.len() < 300, "{:?}", diags[0]);
}

#[test]
fn immutable_keys_reject_all_patch_ops() {
    // (document, patch op, immutable path quoted in the error)
    let cases = [
        (
            "# @ai_immutable: true\nport = 8080\n",
            "@patch(\"port\", 1)",
            "port",
        ),
        (
            "# @ai_immutable: true\nport = 8080\n",
            "@remove(\"port\")",
            "port",
        ),
        (
            "# @ai_immutable: true\nport = 8080\n",
            "@rename(\"port\", \"p2\")",
            "port",
        ),
        // Ancestor edits overlap the immutable descendant.
        (
            "[server]\n# @ai_immutable: true\nport = 1\n",
            "@patch(\"server\", {})",
            "server.port",
        ),
        // Descendant edits overlap the immutable table.
        (
            "# @ai_immutable: true\n[server]\nport = 1\n",
            "@patch(\"server.port\", 1)",
            "server",
        ),
    ];
    for (doc, op, immutable) in cases {
        let src = format!("{doc}{op}\n");
        let module = parse(&src, "annotations").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
        for resilient in [false, true] {
            let mut engine = Engine::new();
            engine.resilient = resilient;
            let err = engine
                .evaluate(&module)
                .expect_err("immutable patch must fail");
            assert_eq!(err.code(), "E019", "resilient={resilient}: {err:?}");
            let message = err.to_string();
            assert!(
                message.contains(immutable),
                "resilient={resilient}: {message}"
            );
            assert!(
                message.contains("ai_immutable"),
                "resilient={resilient}: {message}"
            );
        }
    }
}

#[test]
fn mutable_and_false_immutable_accept_patches() {
    let src = "# @ai_immutable: false\nport = 8080\n@patch(\"port\", 1)\n";
    let v = eval_strict(src).expect("explicit false is mutable");
    assert_eq!(v["port"], 1);
    let src = "port = 8080\n@patch(\"port\", 1)\n";
    let v = eval_strict(src).expect("unannotated is mutable");
    assert_eq!(v["port"], 1);
}

#[test]
fn profile_overrides_of_immutable_keys_stay_allowed() {
    // Documented boundary (spec §4): profiles are declared variants,
    // not automated edits.
    let src = "# @ai_immutable: true\nport = 8080\nprofile \"prod\" {\n  port = 9090\n}\nactive_profile = \"prod\"\n";
    let v = eval_strict(src).expect("profiles are not patch edits");
    assert_eq!(v["port"], 9090);
}

#[test]
fn constraint_violations_order_by_path() {
    let src = "# @constraint: value > 0\nb = -1\n# @constraint: value > 0\na = -2\n";
    let diags = eval_strict(src).expect_err("must fail");
    assert_eq!(diags.len(), 2, "{diags:?}");
    assert_eq!(diags[0].path.as_deref(), Some("a"));
    assert_eq!(diags[1].path.as_deref(), Some("b"));
}

#[test]
fn patched_values_face_constraints() {
    // Base port=5 satisfies the predicate; the patch drives it to 99.
    // The verdict must use the post-patch value, not the graph result.
    let src = "# @constraint: value >= 1 && value <= 10\nport = 5\n@patch(\"port\", 99)\n";
    let diags = eval_strict(src).expect_err("patched value violates");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T006");
    assert!(diags[0].message.contains("99"), "{:?}", diags[0]);
}

#[test]
fn profile_merges_face_constraints() {
    let src = "# @constraint: value <= 10\nport = 5\nprofile \"prod\" {\n  port = 99\n}\nactive_profile = \"prod\"\n";
    let diags = eval_strict(src).expect_err("merged value violates");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "T006");
}

#[test]
fn secret_predicates_render_generic() {
    let src = "# @constraint: value == secret(\"hunter2\")\nname = \"x\"\n";
    let diags = eval_strict(src).expect_err("must fail");
    assert_eq!(diags[0].code, "T006");
    for d in &diags {
        assert!(!d.message.contains("hunter2"), "{d:?}");
        assert!(!d.message.contains("secret(\""), "{d:?}");
    }
    assert!(
        diags[0].message.contains("constraint does not hold"),
        "{:?}",
        diags[0]
    );
}

#[test]
fn profile_bodies_take_no_annotations() {
    let src = "profile \"prod\" {\n  # @intent: \"docs\"\n  # @constraint: value > 0\n  port = 1\n}\nport = 2\n";
    let module = parse_ok(src);
    assert!(
        module.annotations.is_empty(),
        "profiles stay annotation-free"
    );
    let v = eval_strict(src).expect("no enforcement without attachment");
    assert_eq!(v["port"], 2);
}

#[test]
fn removed_keys_skip_constraints() {
    let src = "# @constraint: value > 0\nport = -1\n@remove(\"port\")\n";
    eval_strict(src).expect("removed keys must not fail on stale values");
}

#[test]
fn removed_tables_skip_nested_constraints() {
    let src = "[server]\n# @constraint: value > 0\nport = -1\n@remove(\"server\")\n";
    eval_strict(src).expect("removed subtrees must not fail");
}

#[test]
fn secret_touching_sources_render_generic() {
    let src = "let pw = secret(\"x\")\n# @constraint: value == pw\nname = \"y\"\n";
    let diags = eval_strict(src).expect_err("must fail");
    assert_eq!(diags[0].code, "T006");
    assert!(
        diags[0].message.contains("constraint does not hold"),
        "{:?}",
        diags[0]
    );
    assert!(!diags[0].message.contains("pw"), "{:?}", diags[0]);
}

#[test]
fn long_sources_truncate_to_120_total() {
    let chain = (0..30)
        .map(|_| "value == value")
        .collect::<Vec<_>>()
        .join(" && ");
    // Make it fail so the source renders.
    let payload = format!("{chain} && value == 2");
    assert!(payload.chars().count() > 120);
    let src = format!("# @constraint: {payload}\nport = 1\n");
    let diags = eval_strict(&src).expect_err("must fail");
    assert!(diags[0].message.contains("…"), "{:?}", diags[0]);
    assert!(diags[0].message.len() < 220, "{:?}", diags[0]);
}

#[test]
fn same_path_diagnostics_order_deterministically() {
    // Duplicate keys share one path across two nodes: tie-breaks must
    // be stable run to run.
    let src = "# @constraint: value > 100\nport = 1\n# @constraint: value > 100\nport = 2\n";
    let run = || match parse(src, "annotations") {
        Ok(module) => match strict_engine().evaluate(&module) {
            Err(EvalError::ValidationFailed { diagnostics }) => diagnostics
                .into_iter()
                .map(|d| (d.code, d.message, d.path))
                .collect::<Vec<_>>(),
            other => panic!("must fail strict: {other:?}"),
        },
        Err(diags) => panic!("parse failed: {diags:?}"),
    };
    let (first, second) = (run(), run());
    assert_eq!(first.len(), 2, "{first:?}");
    assert_eq!(first, second);
}

#[test]
fn included_annotations_attach_with_file_attribution() {
    let module = parse_file(&fixture("main_contract.solid"), ParseOptions::default())
        .unwrap_or_else(|diags| panic!("parse failed: {diags:?}"));
    let port = root_field(&module, "port");
    let anns = annotations_of(&module, port);
    assert_eq!(anns.len(), 3, "{anns:?}");
    assert!(anns.iter().all(|a| a.path == "port"));
    // Spans are include-file lines; the value still validates clean.
    assert!(anns.iter().all(|a| a.span.line <= 4), "{anns:?}");
    let v = strict_engine()
        .evaluate(&module)
        .map(|(v, _)| solid_core::convert::value_to_pretty_json(&v))
        .expect("valid include must pass strict");
    assert!(v.contains("80"), "{v}");
}

#[test]
fn included_constraints_see_patched_values() {
    let module = parse_file(
        &fixture("main_plain_patched.solid"),
        ParseOptions::default(),
    )
    .expect("must parse");
    match strict_engine().evaluate(&module) {
        Err(EvalError::ValidationFailed { diagnostics }) => {
            assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
            assert_eq!(diagnostics[0].code, "T006");
            assert_eq!(diagnostics[0].path.as_deref(), Some("port"));
        }
        other => panic!("patched include must fail: {other:?}"),
    }
}

#[test]
fn included_immutable_blocks_main_file_patches() {
    let module = parse_file(
        &fixture("main_contract_immutable_patch.solid"),
        ParseOptions::default(),
    )
    .expect("must parse");
    let err = strict_engine().evaluate(&module).expect_err("must fail");
    assert_eq!(err.code(), "E019");
    assert!(err.to_string().contains("ai_immutable"), "{err:?}");
}

#[test]
fn aliased_includes_scope_annotation_paths() {
    let module = parse_file(
        &fixture("main_contract_alias.solid"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|diags| panic!("parse failed: {diags:?}"));
    let lib = root_field(&module, "lib");
    let port = match module.get(lib).expect("table") {
        Node::Table { fields } => fields["port"],
        _ => panic!("table expected"),
    };
    let anns = annotations_of(&module, port);
    assert_eq!(anns.len(), 3, "{anns:?}");
    assert!(anns.iter().all(|a| a.path == "lib.port"), "{anns:?}");
}
