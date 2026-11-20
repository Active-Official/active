//! Repeated table headers under unification (L-P09 phase 3).
//!
//! A repeated `[table]` (or brace/indent body) now merges recursively at
//! Local priority: disjoint keys union, equal leaves agree, a scalar,
//! array, or shape disagreement is E046 and the first body stays.

use active_core::{parse, Engine};

fn eval(src: &str) -> (active_core::Value, Vec<active_core::Diagnostic>) {
    let module = parse(src, "headers").unwrap_or_else(|d| panic!("parse: {d:?}"));
    Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("evaluate: {e}"))
}

fn table_get(value: &active_core::Value, table: &str, key: &str) -> String {
    let active_core::Value::Table(root) = value else {
        panic!("root table");
    };
    let Some(active_core::Value::Table(inner)) = root.get(table) else {
        panic!("missing table {table}: {root:?}");
    };
    let Some(leaf) = inner.get(key) else {
        panic!("missing {table}.{key}: {inner:?}");
    };
    active_core::convert::value_to_pretty_json(leaf)
}

#[test]
fn repeated_headers_merge_disjoint_keys_in_every_style() {
    for src in [
        "[t]\na = 1\n[t]\nb = 2\n",
        "t {\n  a = 1\n}\nt {\n  b = 2\n}\n",
        "t:\n  a = 1\nt:\n  b = 2\n",
    ] {
        let module = parse(src, "headers").expect("parse");
        assert!(
            module.healing.iter().all(|d| d.code != "W011"),
            "no discard warning for tables: {:?}",
            module.healing
        );
        let (value, diags) = eval(src);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(table_get(&value, "t", "a"), "1");
        assert_eq!(table_get(&value, "t", "b"), "2");
    }
}

#[test]
fn repeated_headers_conflict_at_equal_priority() {
    let (value, diags) = eval("[t]\nport = 1\n[t]\nport = 2\n");
    let conflict = diags
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {diags:?}"));
    assert_eq!(conflict.path.as_deref(), Some("t.port"));
    assert!(conflict.message.contains("local"), "{conflict:?}");
    // First body stays.
    assert_eq!(table_get(&value, "t", "port"), "1");
}

#[test]
fn repeated_headers_shape_conflict_is_e046() {
    let (value, diags) = eval("[t]\nport = 1\n[t]\nport {\n  x = 2\n}\n");
    assert!(
        diags.iter().any(|d| d.code == "E046"),
        "expected E046, got {diags:?}"
    );
    assert_eq!(table_get(&value, "t", "port"), "1");
}

#[test]
fn repeated_nested_headers_merge() {
    let (value, diags) = eval("[t.inner]\na = 1\n[t.inner]\nb = 2\n");
    assert!(diags.is_empty(), "{diags:?}");
    let active_core::Value::Table(root) = &value else {
        panic!("root");
    };
    let Some(active_core::Value::Table(t)) = root.get("t") else {
        panic!("t");
    };
    let Some(active_core::Value::Table(inner)) = t.get("inner") else {
        panic!("inner");
    };
    assert!(
        inner.contains_key("a") && inner.contains_key("b"),
        "{inner:?}"
    );
}

#[test]
fn three_repeated_headers_union_all_bodies() {
    let (value, diags) = eval("[t]\na = 1\n[t]\nb = 2\n[t]\nc = 3\n");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(table_get(&value, "t", "a"), "1");
    assert_eq!(table_get(&value, "t", "b"), "2");
    assert_eq!(table_get(&value, "t", "c"), "3");
}

#[test]
fn duplicate_scalar_keys_keep_last_wins_and_w011() {
    // Phase 3 covers table headers. A repeated scalar key keeps its
    // long-standing last-wins policy, announced as W011.
    let module = parse("x = 1\nx = 2\n", "headers").expect("parse");
    assert!(
        module.healing.iter().any(|d| d.code == "W011"),
        "{:?}",
        module.healing
    );
    let (value, _) = eval("x = 1\nx = 2\n");
    let active_core::Value::Table(root) = &value else {
        panic!("root");
    };
    assert!(matches!(root.get("x"), Some(active_core::Value::Int(2))));
}

#[test]
fn repeated_header_formatting_is_stable_and_value_preserving() {
    let src = "[t]\na = 1\n[t]\nb = 2\n";
    let module = parse(src, "headers").expect("parse");
    for style in [
        active_core::Style::Brace,
        active_core::Style::Indent,
        active_core::Style::Preserve,
    ] {
        let once = active_core::fmt::format(&module, style);
        let reparsed = parse(&once, "fmt").expect("reparse");
        let again = active_core::fmt::format(&reparsed, style);
        assert_eq!(once, again, "formatting is not idempotent for {style:?}");
        let (source_value, _) = eval(src);
        let (formatted_value, _) = Engine::new().evaluate(&reparsed).expect("evaluate");
        assert_eq!(
            active_core::convert::value_to_pretty_json(&source_value),
            active_core::convert::value_to_pretty_json(&formatted_value),
            "formatting changed the merge for {style:?}"
        );
    }
}

#[test]
fn an_equal_earlier_body_still_exposes_a_later_internal_conflict() {
    let src = "[t]\ninner { a = 1 }\n[t]\ninner { a = 1 }\ninner { a = 2 }\n";
    let (value, diags) = eval(src);
    let conflict = diags
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {diags:?}"));
    assert_eq!(conflict.path.as_deref(), Some("t.inner.a"));
    let active_core::Value::Table(root) = &value else {
        panic!("root");
    };
    let Some(active_core::Value::Table(t)) = root.get("t") else {
        panic!("t");
    };
    let Some(active_core::Value::Table(inner)) = t.get("inner") else {
        panic!("inner");
    };
    assert!(matches!(inner.get("a"), Some(active_core::Value::Int(1))));
}

fn immutable_patch_is_refused(module: &active_core::Module) -> bool {
    match Engine::new().evaluate(module) {
        Ok((_, diags)) => diags.iter().any(|d| d.code == "E019"),
        Err(active_core::EvalError::PatchError { .. }) => true,
        Err(other) => panic!("unexpected evaluation failure: {other}"),
    }
}

#[test]
fn an_annotation_on_a_repeated_header_keeps_its_path_through_formatting() {
    let src = "[t]\na = 1\n# @ai_immutable: true\n[t]\nb = 2\n@patch(\"t.c\", 3)\n";
    let module = parse(src, "headers").expect("parse");
    assert!(
        immutable_patch_is_refused(&module),
        "immutability must guard t"
    );
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    let reparsed = parse(&out, "fmt").expect("reparse");
    assert!(
        immutable_patch_is_refused(&reparsed),
        "formatting moved the annotation off t:\n{out}"
    );
}

#[test]
fn repeated_header_trailing_comments_are_emitted_once() {
    let src = "[t]\na = 1\n[t]\nb = 2\n# end note\n";
    let module = parse(src, "headers").expect("parse");
    for style in [active_core::Style::Brace, active_core::Style::Indent] {
        let once = active_core::fmt::format(&module, style);
        assert_eq!(
            once.matches("# end note").count(),
            1,
            "comment duplicated for {style:?}:\n{once}"
        );
        let reparsed = parse(&once, "fmt").expect("reparse");
        let again = active_core::fmt::format(&reparsed, style);
        assert_eq!(once, again, "not a fixed point for {style:?}:\n{once}");
    }
}

#[test]
fn empty_nested_repeated_bodies_never_replace_the_merge() {
    let src = "outer {\n  t { a = 1 }\n  t {}\n}\n";
    let module = parse(src, "headers").expect("parse");
    for style in [active_core::Style::Brace, active_core::Style::Indent] {
        let once = active_core::fmt::format(&module, style);
        let reparsed = parse(&once, "fmt").expect("reparse");
        let again = active_core::fmt::format(&reparsed, style);
        assert_eq!(once, again, "not a fixed point for {style:?}:\n{once}");
        let (source_value, _) = eval(src);
        let (formatted_value, _) = Engine::new().evaluate(&reparsed).expect("evaluate");
        assert_eq!(
            active_core::convert::value_to_pretty_json(&source_value),
            active_core::convert::value_to_pretty_json(&formatted_value),
            "empty extra body changed the value for {style:?}:\n{once}"
        );
    }
}

#[test]
fn extracted_metadata_is_not_reemitted_by_repeated_bodies() {
    let with_depends = "[t]\ndepends_on = [\"x.active\"]\n[t]\na = 1\n";
    let module = parse(with_depends, "headers").expect("parse");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    assert_eq!(
        out.matches("depends_on").count(),
        1,
        "depends_on duplicated:\n{out}"
    );
    assert!(!out.contains("schema = null"), "{out}");
    let reparsed = parse(&out, "fmt").expect("reparse");
    assert!(
        reparsed.healing.iter().all(|d| d.code != "W011"),
        "{:?}",
        reparsed.healing
    );

    let with_schema = "[t]\nschema { a = Int }\n[t]\na = 1\nb = 1\n";
    let module = parse(with_schema, "headers").expect("parse");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    assert!(!out.contains("schema = null"), "{out}");
    let reparsed = parse(&out, "fmt").expect("reparse");
    let (_, diags) = Engine::new().evaluate(&reparsed).expect("evaluate");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn a_moved_merge_reports_its_conflict_not_a_missing_value() {
    let src = "z = 0\n[t]\ninner { a = 0 }\n[t]\ninner { a = z }\ninner { a = 2 }\n";
    let (value, diags) = eval(src);
    let conflict = diags
        .iter()
        .find(|d| d.code == "E046")
        .unwrap_or_else(|| panic!("expected E046, got {diags:?}"));
    assert!(
        conflict.message.contains("disagree"),
        "genuine conflict misdescribed: {conflict:?}"
    );
    let active_core::Value::Table(root) = &value else {
        panic!("root");
    };
    let Some(active_core::Value::Table(t)) = root.get("t") else {
        panic!("t");
    };
    let Some(active_core::Value::Table(inner)) = t.get("inner") else {
        panic!("inner");
    };
    assert!(matches!(inner.get("a"), Some(active_core::Value::Int(0))));
}

#[test]
fn formatting_a_resolved_module_keeps_include_priority() {
    let dir = std::env::temp_dir().join(format!("active_fmt_resolved_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("d.active"), "x = \"inc\"\n").unwrap();
    let main = dir.join("main.active");
    std::fs::write(
        &main,
        "outer {\n  t { x = \"local\" }\n  t { include \"d.active\" }\n}\n",
    )
    .unwrap();
    let module =
        active_core::parse_file(&main, active_core::ParseOptions::default()).expect("parse");
    let (before, _) = Engine::new().evaluate(&module).expect("evaluate");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    let formatted = dir.join("formatted.active");
    std::fs::write(&formatted, &out).unwrap();
    let reparsed =
        active_core::parse_file(&formatted, active_core::ParseOptions::default()).expect("parse");
    let (after, diags) = Engine::new().evaluate(&reparsed).expect("evaluate");
    assert!(
        diags.iter().all(|d| d.code != "E046"),
        "formatted include became a literal: {diags:?}\n{out}"
    );
    assert_eq!(
        active_core::convert::value_to_pretty_json(&before),
        active_core::convert::value_to_pretty_json(&after)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn repeated_body_schemas_follow_local_priority() {
    let dir = std::env::temp_dir().join(format!("active_schema_prio_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("def_schema.active"), "schema { a = Int }\n").unwrap();

    let main = dir.join("string.active");
    std::fs::write(
        &main,
        "[t]\ninclude \"def_schema.active\"\n[t]\nschema { a = String }\na = \"x\"\n",
    )
    .unwrap();
    let module =
        active_core::parse_file(&main, active_core::ParseOptions::default()).expect("parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("evaluate");
    assert!(
        diags.iter().all(|d| d.code != "T003"),
        "local String schema must beat the included Int schema: {diags:?}"
    );

    let closed = dir.join("closed.active");
    std::fs::write(
        &closed,
        "[t]\nschema { closed = true, a = Int }\na = 1\n[t]\nschema { b = Int }\nb = 2\nextra = 3\n",
    )
    .unwrap();
    let module =
        active_core::parse_file(&closed, active_core::ParseOptions::default()).expect("parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("evaluate");
    assert!(
        diags.iter().any(|d| d.code == "T005"),
        "an included open schema must not reopen a local closed schema: {diags:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn nested_metadata_in_extra_bodies_is_not_reemitted_as_data() {
    let src = "[t]\ninner { x = 1 }\n[t]\ninner { schema { b = Int }\n  y = 2\n  b = 2\n}\n";
    let module = parse(src, "headers").expect("parse");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    assert!(!out.contains("schema = null"), "{out}");
    let reparsed = parse(&out, "fmt").expect("reparse");
    let (value, diags) = Engine::new().evaluate(&reparsed).expect("evaluate");
    assert!(
        diags.iter().all(|d| d.code != "E046"),
        "formatted metadata produced a conflict: {diags:?}\n{out}"
    );
    let active_core::Value::Table(root) = &value else {
        panic!("root");
    };
    let Some(active_core::Value::Table(t)) = root.get("t") else {
        panic!("t");
    };
    let Some(active_core::Value::Table(inner)) = t.get("inner") else {
        panic!("inner");
    };
    assert!(
        inner.contains_key("y") && inner.contains_key("b"),
        "{inner:?}"
    );
}

#[test]
fn a_profile_and_a_same_named_table_keep_separate_metadata() {
    let src = "profile \"a\" { x = 1 }\n[a]\ndepends_on = [\"x\"]\nx = 1\n";
    let module = parse(src, "headers").expect("parse");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    let reparsed = parse(&out, "fmt").expect("reparse");
    assert!(
        reparsed.diagnostics.iter().all(|d| d.code != "E022"),
        "metadata moved into the profile: {out}\n{:?}",
        reparsed.diagnostics
    );
    let (value, diags) = Engine::new().evaluate(&reparsed).expect("evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(table_get(&value, "a", "x"), "1");
}

#[test]
fn repeated_body_schemas_union() {
    let src = "[t]\na = 1\nschema { a = Int }\n[t]\nschema { b = Int(default = 42) }\n";
    let module = parse(src, "headers").expect("parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("evaluate");
    assert!(
        diags.iter().all(|d| d.code == "W005"),
        "only the default-fill notice is expected: {diags:?}"
    );
    assert_eq!(table_get(&value, "t", "a"), "1");
    assert_eq!(table_get(&value, "t", "b"), "42");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    assert!(!out.contains("schema = null"), "{out}");
    let reparsed = parse(&out, "fmt").expect("reparse");
    let (formatted, diags) = Engine::new().evaluate(&reparsed).expect("evaluate");
    assert!(
        diags.iter().all(|d| d.code == "W005"),
        "only the default-fill notice is expected: {diags:?}"
    );
    assert_eq!(
        active_core::convert::value_to_pretty_json(&value),
        active_core::convert::value_to_pretty_json(&formatted)
    );
}
