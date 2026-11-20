//! Formatter references keep their full dotted path (L-P09 follow-up).
//!
//! A `Ref` used to render its bare leaf key, so `a.port == b.port`
//! formatted to `port == port` and silently changed meaning. The
//! formatter now renders the target's dotted path, verifies that the text
//! resolves back to the same node from where it is written, and inlines a
//! scalar whose name does not verify. These tests use `try_format` (the
//! writing entry point) so a refusal cannot pass as a round trip.

use active_core::{Engine, Module, ParseOptions, Style};

fn parse(src: &str) -> Module {
    active_core::parse_with(
        src,
        "formatter-refs",
        ParseOptions {
            max_wall_ms: 30_000,
            ..ParseOptions::default()
        },
    )
    .unwrap_or_else(|diags| panic!("parse: {diags:?}"))
}

fn evaluate(module: &Module) -> (active_core::Value, Vec<active_core::Diagnostic>) {
    Engine::new()
        .evaluate(module)
        .unwrap_or_else(|e| panic!("evaluate: {e}"))
}

fn format(module: &Module, style: Style) -> String {
    active_core::fmt::try_format(module, style)
        .unwrap_or_else(|error| panic!("format refused ({style:?}): {error}"))
}

fn value_json(value: &active_core::Value) -> serde_json::Value {
    serde_json::from_str(&active_core::convert::value_to_pretty_json(value)).expect("json")
}

#[test]
fn a_schema_check_keeps_its_dotted_references_through_formatting() {
    let src = "schema {\n  a = Table { port = Int }\n  b = Table { port = Int }\n  check {\n    ports_equal = a.port == b.port\n  }\n}\na = { port = 1 }\nb = { port = 2 }\n";
    let module = parse(src);
    let (_, before) = evaluate(&module);
    assert!(
        before.iter().any(|d| d.code == "T009"),
        "the unequal check must fail before formatting: {before:?}"
    );
    for style in [Style::Brace, Style::Indent] {
        let out = format(&module, style);
        assert!(
            out.contains("a.port == b.port"),
            "check text lost its dots ({style:?}):\n{out}"
        );
        let reparsed = parse(&out);
        let (_, after) = evaluate(&reparsed);
        assert!(
            after.iter().any(|d| d.code == "T009"),
            "formatting made the failing check pass ({style:?}):\n{out}"
        );
    }
}

#[test]
fn data_expressions_keep_their_dotted_references_and_value() {
    let src = "a = { port = 1 }\nb = { port = 2 }\nports_equal = a.port == b.port\n";
    let module = parse(src);
    let (before, _) = evaluate(&module);
    for style in [Style::Brace, Style::Indent, Style::Preserve] {
        let out = format(&module, style);
        assert!(
            out.contains("a.port == b.port"),
            "data reference lost its dots ({style:?}):\n{out}"
        );
        let reparsed = parse(&out);
        let (after, _) = evaluate(&reparsed);
        assert_eq!(
            value_json(&before),
            value_json(&after),
            "formatting changed the value ({style:?}):\n{out}"
        );
    }
}

#[test]
fn sibling_tables_with_the_same_leaf_name_stay_distinct() {
    // The reported case: both tables own a `port`, and the merged
    // expression must address each one, not collapse to `port + port`.
    let src = "a = { port = 1 }\n\
               b = { port = 2 }\n\
               sum = a.port + b.port\n\
               deeper { x = { port = 10 }, y = { port = 20 }, total = x.port + y.port }\n";
    let module = parse(src);
    let (before, _) = evaluate(&module);
    for style in [Style::Brace, Style::Indent] {
        let out = format(&module, style);
        assert!(
            out.contains("a.port + b.port") && out.contains("x.port + y.port"),
            "a dotted reference collapsed ({style:?}):\n{out}"
        );
        let reparsed = parse(&out);
        let (after, diags) = evaluate(&reparsed);
        assert!(diags.is_empty(), "{diags:?}\n{out}");
        assert_eq!(
            value_json(&before),
            value_json(&after),
            "sibling round-trip changed the value ({style:?}):\n{out}"
        );
    }
}

#[test]
fn a_schema_default_through_a_binding_is_kept() {
    let src = "let default_port = 5432\n\
               schema { db = Table { port = Int(default = default_port) } }\n\
               db = { port = 1 }\n";
    let module = parse(src);
    let (before, _) = evaluate(&module);
    for style in [Style::Brace, Style::Indent] {
        let out = format(&module, style);
        assert!(
            out.contains("default = default_port"),
            "the binding reference was rewritten ({style:?}):\n{out}"
        );
        let reparsed = parse(&out);
        let (after, diags) = evaluate(&reparsed);
        assert!(diags.is_empty(), "{diags:?}\n{out}");
        assert_eq!(
            value_json(&before),
            value_json(&after),
            "schema default changed ({style:?}):\n{out}"
        );
    }
}

#[test]
fn references_survive_shadowed_names_and_profile_overlays() {
    // `outer` shadows the root `x`, and the profile shadows `b`. The
    // references below must keep reading what they read before, and the
    // let-bound ones must keep their binding spelling.
    let src = "x = { port = 1 }\n\
               let r = x.port\n\
               b = { port = 9 }\n\
               outer { x = { port = 2 }, v = r, sibling = x.port }\n\
               profile \"p\" { b = { port = 3 }, own = b.port, via_let = r }\n\
               active_profile = \"p\"\n";
    let module = parse(src);
    let (before, diags) = evaluate(&module);
    assert!(diags.is_empty(), "{diags:?}");
    for style in [Style::Brace, Style::Indent] {
        let out = format(&module, style);
        for expected in ["v = r", "sibling = x.port", "own = b.port", "via_let = r"] {
            assert!(
                out.contains(expected),
                "expected `{expected}` to survive ({style:?}):\n{out}"
            );
        }
        let reparsed = parse(&out);
        let (after, diags) = evaluate(&reparsed);
        assert!(diags.is_empty(), "{diags:?}\n{out}");
        assert_eq!(
            value_json(&before),
            value_json(&after),
            "a shadowed reference changed meaning ({style:?}):\n{out}"
        );
    }
}

#[test]
fn an_unrenderable_reference_chain_is_refused() {
    // A chain the parser never builds (no name, over the inline bound).
    // `try_format` must refuse it; `format` is the display form.
    use active_core::ir::{BinOp, Node};
    let mut module = parse("v = 0\n");
    let root = module.root.expect("root");
    let field = match module.get(root) {
        Some(Node::Table { fields }) => fields["v"],
        _ => panic!("root is a table"),
    };
    let one = module.add_node(Node::Literal(active_core::Value::Int(1)));
    let mut tail = module.add_node(Node::Ref(one));
    for _ in 0..200 {
        tail = module.add_node(Node::Ref(tail));
    }
    module.arena[field.0 as usize] = Node::Binary {
        op: BinOp::Add,
        lhs: tail,
        rhs: one,
    };
    assert_eq!(
        active_core::fmt::try_format(&module, Style::Brace),
        Err(active_core::fmt::FormatError::UnrenderableReference)
    );
}

#[test]
fn the_error_is_displayable() {
    assert!(active_core::fmt::FormatError::UnrenderableReference
        .to_string()
        .contains("reference"));
    assert!(active_core::fmt::FormatError::UnverifiedReference
        .to_string()
        .contains("resolve"));
    assert!(active_core::fmt::FormatError::OutputDoesNotParse
        .to_string()
        .contains("re-parse"));
}

#[test]
fn a_shadowed_table_reference_is_refused() {
    // Rewire `ref` from its sibling `b.a` to the shadowed root `a`: the
    // recorded path "a" would read `b.a` at that site, so the formatter
    // must not claim it can write this document.
    use active_core::ir::Node;
    let mut module = parse("a = { p = 1 }\nb = { a = { p = 2 }, ref = a }\n");
    let root = module.root.expect("root");
    let (root_a, b) = match module.get(root) {
        Some(Node::Table { fields }) => (fields["a"], fields["b"]),
        _ => panic!("root is a table"),
    };
    let ref_node = match module.get(b) {
        Some(Node::Table { fields }) => fields["ref"],
        _ => panic!("b is a table"),
    };
    module.arena[ref_node.0 as usize] = Node::Ref(root_a);
    assert_eq!(
        active_core::fmt::try_format(&module, Style::Brace),
        Err(active_core::fmt::FormatError::UnverifiedReference)
    );
}

#[test]
fn a_deep_profile_body_formats_and_round_trips() {
    // Deep enough that the formatter switches to dotted headers, inside a
    // profile: the flat path must keep the profile's own scope.
    let mut body = "value = 1\n".to_string();
    for _ in 0..60 {
        body = format!("next {{\n{body}}}\n");
    }
    let src = format!("active_profile = \"p\"\nprofile \"p\" {{\n{body}}}\n");
    let module = parse(&src);
    let (before, _) = evaluate(&module);
    for style in [Style::Brace, Style::Indent] {
        let out = format(&module, style);
        let reparsed = parse(&out);
        let (after, diags) = evaluate(&reparsed);
        assert!(diags.is_empty(), "{diags:?}\n{out}");
        assert_eq!(
            value_json(&before),
            value_json(&after),
            "deep profile changed ({style:?}):\n{out}"
        );
    }
}

#[test]
fn a_flat_tree_keeps_repeated_body_fields() {
    // 65 header segments put the tree past the flat threshold. A repeated
    // leaf header under it must keep every body: dropping one would
    // change the merged table with no diagnostic.
    let deep = std::iter::repeat_n("n", 65).collect::<Vec<_>>().join(".");
    let src = format!("[{deep}]\nv = 1\n[{deep}.leaf]\nk = 1\n[{deep}.leaf]\nj = 2\n");
    let module = parse(&src);
    let (before, diags) = evaluate(&module);
    assert!(diags.is_empty(), "{diags:?}");
    for style in [Style::Brace, Style::Indent] {
        let out = format(&module, style);
        let reparsed = parse(&out);
        let (after, diags) = evaluate(&reparsed);
        assert!(diags.is_empty(), "{diags:?}\n{out}");
        assert_eq!(
            value_json(&before),
            value_json(&after),
            "flat mode dropped a repeated body ({style:?}):\n{out}"
        );
    }
}

#[test]
fn dotted_references_round_trip_at_depth() {
    // let-bound table ref, nested tables, and a profile-internal ref.
    let src = "let base = { port = 8080 }\n\
               a = { port = 1 }\n\
               c = a.port\n\
               nested { inner { value = 1 } }\n\
               copy = nested.inner.value\n\
               profile \"p\" { from_root = a.port\n  from_let = base }\n\
               active_profile = \"p\"\n";
    let module = parse(src);
    let (before, _) = evaluate(&module);
    for style in [Style::Brace, Style::Indent] {
        let out = format(&module, style);
        for expected in [
            "c = a.port",
            "copy = nested.inner.value",
            "from_root = a.port",
            "from_let = base",
        ] {
            assert!(
                out.contains(expected),
                "expected `{expected}` to survive ({style:?}):\n{out}"
            );
        }
        let reparsed = parse(&out);
        let (after, diags) = evaluate(&reparsed);
        assert!(diags.is_empty(), "{diags:?}\n{out}");
        assert_eq!(
            value_json(&before),
            value_json(&after),
            "round-trip changed the value ({style:?}):\n{out}"
        );
    }
}
