//! Schema language tests (C-030: parsing and IR only).
//!
//! Proves on the real shipped path that schema blocks parse in all three
//! forms (inline, brace, indent), lower into the schema tree detached
//! from value fields, and never export as data. Validation itself is
//! C-031; here schemas only need to parse, extract, and round-trip
//! through the formatter. Negative tests assert E020 with spans.

use active_core::fmt::format;
use active_core::ir::Node;
use active_core::types::ActiveType;
use active_core::{parse, parse_file, Engine, ParseOptions, Style};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("schemas")
        .join(name)
}

fn parse_ok(src: &str) -> active_core::Module {
    parse(src, "schema").unwrap_or_else(|d| panic!("parse failed: {d:?}"))
}

fn parse_err(src: &str) -> Vec<active_core::Diagnostic> {
    parse(src, "schema").expect_err("must fail")
}

fn e020(src: &str) -> active_core::Diagnostic {
    let diags = parse_err(src);
    diags
        .into_iter()
        .find(|d| d.code == "E020")
        .expect("E020 expected")
}

fn tree(module: &active_core::Module) -> serde_json::Value {
    let engine = Engine::new();
    let (value, diags) = engine.evaluate(module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    serde_json::from_str(&json).expect("valid json")
}

/// Field names and type names of the root schema table, in source order.
fn root_schema(module: &active_core::Module) -> Vec<(String, String)> {
    let id = module.schema.expect("root schema expected");
    match module.get(id).expect("schema node") {
        Node::SchemaTable { fields, .. } => fields
            .iter()
            .map(|(k, v)| (k.clone(), schema_type_name(module, *v)))
            .collect(),
        other => panic!("root schema must be a table, got {other:?}"),
    }
}

fn schema_type_name(module: &active_core::Module, id: active_core::NodeId) -> String {
    match module.get(id).expect("schema node") {
        Node::SchemaField { field } => field.typ.name(),
        Node::SchemaArray { .. } => "Array".into(),
        Node::SchemaTable { .. } => "Table".into(),
        other => panic!("schema node expected, got {other:?}"),
    }
}

const ALL_TYPES: &str = r#"schema = {
  int_f = Int(min = 1, max = 10, default = 5),
  float_f = Float(min = 0.5),
  str_f = String(min_len = 1, max_len = 8, default = "hi"),
  bool_f = Bool,
  null_f = Null,
  dur_f = Duration(min = 1s),
  bytes_f = Bytes(max = 1MiB),
  secret_f = Secret,
  any_f = Any,
  table_f = Table,
  enum_f = Enum("a", "b"),
  arr_f = Array<Int>,
  nested = Table { inner = String },
}

int_f = 5
float_f = 0.75
str_f = "hi"
bool_f = true
null_f = null
dur_f = 5s
bytes_f = 512KiB
secret_f = secret("k")
any_f = 42
table_f = { a = 1 }
enum_f = "a"
arr_f = [1, 2]
nested = { inner = "x" }
"#;

#[test]
fn all_type_forms_parse_and_extract() {
    let module = parse_ok(ALL_TYPES);
    let names: Vec<(String, String)> = root_schema(&module);
    let keys: Vec<&str> = names.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "int_f", "float_f", "str_f", "bool_f", "null_f", "dur_f", "bytes_f", "secret_f",
            "any_f", "table_f", "enum_f", "arr_f", "nested"
        ],
    );
    assert_eq!(names[5].1, "Duration");
    assert_eq!(names[10].1, "Enum(a, b)");
    assert_eq!(names[11].1, "Array");
    // Data evaluates; the schema never exports.
    let v = tree(&module);
    assert_eq!(v["int_f"], 5);
    assert!(v.get("schema").is_none(), "schema must not export: {v}");
}

#[test]
fn three_schema_forms_agree() {
    let inline = "schema = { port = Int(min = 1) }\nport = 1\n";
    let brace = "schema { port = Int(min = 1) }\nport = 1\n";
    let indent = "schema:\n  port = Int(min = 1)\nport = 1\n";
    let mut shapes = Vec::new();
    for src in [inline, brace, indent] {
        let module = parse_ok(src);
        shapes.push(root_schema(&module));
        assert_eq!(tree(&module)["port"], 1);
    }
    assert_eq!(shapes[0], shapes[1]);
    assert_eq!(shapes[1], shapes[2]);
}

#[test]
fn nested_schema_attaches_to_its_table() {
    let src = "[server]\nschema: {\n  host = String(min_len = 1),\n}\nhost = \"h\"\n";
    let module = parse_ok(src);
    // The root schema is an open parent created for the nested table.
    let root = module.schema.expect("open parent expected");
    let (fields, closed) = match module.get(root).expect("schema node") {
        Node::SchemaTable { fields, closed, .. } => (fields.clone(), *closed),
        other => panic!("schema table expected, got {other:?}"),
    };
    assert!(!closed, "synthesized parent stays open");
    assert!(
        fields.contains_key("server"),
        "nested schema keyed by table"
    );
    let v = tree(&module);
    assert_eq!(v["server"]["host"], "h");
    assert!(
        v["server"].get("schema").is_none(),
        "nested schema must not export: {v}"
    );
}

#[test]
fn string_schema_stays_ordinary_data() {
    // `schema` is contextual: a non-table value is data, never schema.
    let module = parse_ok("schema = \"v1\"\n");
    assert!(module.schema.is_none(), "string schema must stay data");
    assert_eq!(tree(&module)["schema"], "v1");
}

#[test]
fn closed_flag_parses_and_renders() {
    let module = parse_ok("schema = { closed = true, port = Int }\nport = 1\n");
    let id = module.schema.expect("schema expected");
    assert!(matches!(
        module.get(id),
        Some(Node::SchemaTable { closed: true, .. })
    ));
    let out = format(&module, Style::Brace);
    assert!(out.contains("closed = true"), "flag must render:\n{out}");
}

#[test]
fn fmt_preserves_schema_and_data_in_both_styles() {
    for style in [Style::Brace, Style::Indent] {
        let module = parse_ok(ALL_TYPES);
        let once = format(&module, style);
        assert!(once.contains("schema"), "schema must emit:\n{once}");
        assert!(
            once.contains("Int(min = 1, max = 10, default = 5)"),
            "args must render:\n{once}"
        );
        let again = format(&parse_ok(&once), style);
        assert_eq!(once, again, "schema fmt must be idempotent");
        // Reparsed document evaluates to the same tree, schema-free.
        let v = tree(&parse_ok(&once));
        assert_eq!(v["int_f"], 5);
        assert!(v.get("schema").is_none());
    }
}

#[test]
fn unknown_type_is_e020() {
    let d = e020("schema = { port = Frobnicate }\nport = 1\n");
    assert!(d.span.is_some(), "E020 must carry a span");
}

#[test]
fn bare_array_and_enum_are_e020() {
    e020("schema = { xs = Array }\nxs = []\n");
    e020("schema = { mode = Enum }\nmode = \"a\"\n");
}

#[test]
fn wrong_argument_for_type_is_e020() {
    e020("schema = { name = String(min = 1) }\nname = \"a\"\n");
    e020("schema = { flag = Bool(min = 1) }\nflag = true\n");
}

#[test]
fn empty_enum_is_e020() {
    e020("schema = { mode = Enum() }\nmode = \"a\"\n");
}

#[test]
fn non_boolean_closed_is_e020() {
    e020("schema = { closed = \"yes\", port = Int }\nport = 1\n");
}

#[test]
fn schema_inside_profile_is_e020() {
    e020("profile \"p\" { schema = { port = Int } }\n");
}

#[test]
fn non_type_entry_is_e020() {
    e020("schema = { port = 8080 }\nport = 1\n");
}

#[test]
fn bare_table_type_name_renders_open() {
    assert_eq!(ActiveType::Table { closed: false }.name(), "Table");
    assert_eq!(ActiveType::Table { closed: true }.name(), "Table(closed)");
}

#[test]
fn table_call_form_is_e020() {
    for src in [
        "schema = { t = Table() }\n",
        "schema = { t = Table(default = 1) }\n",
    ] {
        let diags = parse(src, "schema").expect_err("Table() must fail");
        assert_eq!(
            diags.iter().filter(|d| d.code == "E020").count(),
            1,
            "exactly one E020 expected: {diags:?}"
        );
    }
}

#[test]
fn unclosed_schema_brace_is_e001() {
    for src in [
        "schema = { port = Int",
        "schema { port = Int",
        "schema = { srv = Table { host = String }",
    ] {
        let diags = parse(src, "schema").expect_err("unclosed schema must fail");
        assert!(
            diags.iter().any(|d| d.code == "E001"),
            "E001 expected: {diags:?}"
        );
    }
    // Indentation form ends naturally at EOF: no error.
    parse_ok("schema:\n  port = Int\nport = 1\n");
}

#[test]
fn schema_nesting_depth_is_bounded() {
    // 10-deep nesting parses.
    let ok: String =
        "schema = { x = ".to_string() + &"Array<".repeat(10) + "Int" + &">".repeat(10) + " }\n";
    parse_ok(&ok);
    // Past the 64-level limit: E004, no hang, no panic.
    let deep: String =
        "schema = { x = ".to_string() + &"Array<".repeat(70) + "Int" + &">".repeat(70) + " }\n";
    let diags = parse(&deep, "schema").expect_err("over-deep schema must fail");
    assert!(
        diags.iter().any(|d| d.code == "E004"),
        "E004 expected: {diags:?}"
    );
    // Nested Table forms count toward the same budget.
    let deep_tables: String = "schema = { x = ".to_string()
        + &"Table { a = ".repeat(70)
        + "Int"
        + &"}".repeat(70)
        + " }\n";
    let diags = parse(&deep_tables, "schema").expect_err("over-deep tables must fail");
    assert!(
        diags.iter().any(|d| d.code == "E004"),
        "E004 expected: {diags:?}"
    );
}

#[test]
fn over_deep_expression_chain_diagnostics_are_bounded() {
    // 300 unclosed brackets must fail closed with E004, but the
    // diagnostic tail must be bounded. Uncapped, a 6KB file emits
    // thousands of identical errors (CLI stderr flood, LSP publish
    // storm per keystroke, agent-log bloat). The root-cause E004 is
    // first in emission order and always retained; only the
    // pathological tail is dropped. Occurrence preservation below
    // the cap is pinned by active-wasm's envelope repeats test.
    let src = format!("value = {}", "[".repeat(300));
    let diags = parse(&src, "depth-flood").expect_err("over-deep input must fail");
    assert!(diags.iter().any(|d| d.code == "E004"), "E004 expected");
    assert_eq!(diags[0].code, "E004", "root cause first");
    assert!(
        diags.len() <= 1024,
        "diagnostic output bounded, got {}",
        diags.len()
    );
}

#[test]
fn lexer_error_volume_is_bounded() {
    // 5000 invalid bytes must fail closed, but the diagnostic tail
    // must be bounded just like parser errors: one E005 per byte
    // would turn a 100KB file into megabytes of errors.
    let src = "v = 1\n".to_string() + &"\u{0}".repeat(5000);
    let diags = parse(&src, "lex-flood").expect_err("invalid bytes must fail");
    assert!(diags.iter().any(|d| d.code == "E005"), "E005 expected");
    assert_eq!(diags[0].code, "E005", "root cause first");
    assert!(
        diags.len() <= 1024,
        "lexer diagnostic output bounded, got {}",
        diags.len()
    );
}

#[test]
fn empty_schema_round_trips_in_both_styles() {
    let module = parse_ok("schema = {}\nport = 1\n");
    for style in [Style::Brace, Style::Indent] {
        let once = format(&module, style);
        assert!(
            once.contains("schema = {}"),
            "empty schema stays inline:\n{once}"
        );
        let again = format(&parse_ok(&once), style);
        assert_eq!(once, again, "must be idempotent");
    }
}

#[test]
fn included_schema_extracts_and_never_exports() {
    let module = parse_file(
        &fixture("main_include_schema.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    let names: Vec<String> = root_schema(&module).into_iter().map(|(k, _)| k).collect();
    assert_eq!(names, ["port"]);
    let v = tree(&module);
    assert_eq!(v["port"], 8080);
    assert_eq!(v["mode"], "dev");
    assert!(v.get("schema").is_none(), "schema must not export: {v}");
}

#[test]
fn local_and_included_schemas_union() {
    let module = parse_file(
        &fixture("main_union_schema.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    let mut names: Vec<String> = root_schema(&module).into_iter().map(|(k, _)| k).collect();
    names.sort();
    assert_eq!(names, ["host", "port", "retries"]);
    let v = tree(&module);
    assert_eq!(v["port"], 8080);
    assert_eq!(v["host"], "example.com");
}

#[test]
fn aliased_include_schema_nests_without_exporting() {
    let module = parse_file(
        &fixture("main_alias_schema.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    let v = tree(&module);
    assert_eq!(v["lib"]["port"], 8080);
    assert!(
        v["lib"].get("schema").is_none(),
        "aliased schema must not export: {v}"
    );
}

#[test]
fn profile_include_schema_is_e020() {
    for name in [
        "main_profile_schema.active",
        "main_profile_schema_alias.active",
    ] {
        let diags = parse_file(&fixture(name), ParseOptions::default())
            .expect_err(&format!("{name} must fail"));
        let d = diags
            .iter()
            .find(|d| d.code == "E020")
            .unwrap_or_else(|| panic!("E020 expected: {diags:?}"));
        assert!(d.span.is_some(), "E020 must carry a span");
    }
}

#[test]
fn later_include_cascades_over_earlier_schema() {
    // lib_cas_a says mode = String; lib_cas_b says mode = Int.
    // Last include wins, like any other key.
    let module = parse_file(
        &fixture("main_cascade_schema.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    assert_eq!(
        root_schema(&module),
        [("mode".to_string(), "Int".to_string())]
    );
    let v = tree(&module);
    assert_eq!(v["mode"], 1);
}

#[test]
fn local_schema_beats_included_schema() {
    let module = parse_file(
        &fixture("main_localwins_schema.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    assert_eq!(
        root_schema(&module),
        [("mode".to_string(), "Bool".to_string())]
    );
    let v = tree(&module);
    assert_eq!(v["mode"], true);
}

#[test]
fn schema_inside_array_elements_is_e020() {
    // Without this ban the schema would evade extraction (which
    // descends tables only) and leak `"schema": null` into exports.
    for src in [
        "servers = [{ schema = { port = Int }, port = 1 }]\n",
        "x = [[{ schema = { a = Int } }]]\n",
    ] {
        let diags = parse(src, "schema").expect_err("array schema must fail");
        assert!(
            diags.iter().any(|d| d.code == "E020"),
            "E020 expected: {diags:?}"
        );
    }
}

#[test]
fn schema_in_plain_inline_table_still_extracts() {
    // Table-scope metadata stays allowed: it extracts under its table
    // and never exports.
    let module = parse_ok("point = { schema = { x = Int }, x = 1 }\n");
    let v = tree(&module);
    assert_eq!(v["point"]["x"], 1);
    assert!(
        v["point"].get("schema").is_none(),
        "inline schema must not export: {v}"
    );
}
