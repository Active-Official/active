//! Dual-syntax tests (C-021).
//!
//! Proves the C-020 guarantee on the real path: brace style, indentation
//! style, colon key-values, and mixed documents that describe the same tree
//! lower to the identical IR. Also pins the diagnostics introduced by the
//! dual-syntax design (E006, E007, E008, E010).

use solid_core::{parse, parse_recovering, Engine, ParseOptions};

fn tree(src: &str) -> String {
    let module = parse(src, "dual").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    let engine = Engine::new();
    let (value, diags) = engine
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("evaluation failed: {e}"));
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    solid_core::convert::value_to_pretty_json(&value)
}

fn codes(src: &str) -> Vec<String> {
    match parse(src, "dual") {
        Ok(_) => Vec::new(),
        Err(diags) => diags.into_iter().map(|d| d.code).collect(),
    }
}

/// The recovered tree for input containing errors (tooling path).
fn recovered(src: &str) -> (String, Vec<String>) {
    let (module, errors) = parse_recovering(src, "dual", ParseOptions { resilient: false });
    let engine = Engine::new();
    let json = match engine.evaluate(&module) {
        Ok((value, _)) => solid_core::convert::value_to_pretty_json(&value),
        Err(e) => format!("EVAL_ERROR: {e}"),
    };
    (json, errors.into_iter().map(|d| d.code).collect())
}

#[test]
fn brace_and_indentation_styles_lower_to_identical_ir() {
    let brace = "[app]\nname = \"svc\"\nport = 8080\n\n[app2]\nname = \"svc2\"\n";
    let indent = "app:\n  name = \"svc\"\n  port = 8080\n\napp2:\n  name = \"svc2\"\n";
    assert_eq!(
        tree(brace),
        tree(indent),
        "styles must produce the same tree"
    );
}

#[test]
fn colon_key_values_match_equals_form() {
    let eq = "app:\n  name = \"svc\"\n  port = 8080\n";
    let colon = "app:\n  name: \"svc\"\n  port: 8080\n";
    assert_eq!(tree(eq), tree(colon));
}

#[test]
fn brace_table_blocks_match_header_style() {
    let headers = "[app]\nname = \"svc\"\nport = 8080\n";
    let blocks = "app {\n  name = \"svc\"\n  port = 8080\n}\n";
    assert_eq!(tree(headers), tree(blocks));
}

#[test]
fn empty_brace_block_is_an_empty_table() {
    assert_eq!(tree("app { }\n"), tree("[app]\n"));
}

#[test]
fn mixed_styles_in_one_document() {
    let mixed = "app:\n  name = \"svc\"\n\n[database]\nhost = \"db\"\n\ncache {\n  size = 512\n}\n";
    let json = tree(mixed);
    assert!(json.contains("\"name\": \"svc\""), "{json}");
    assert!(json.contains("\"host\": \"db\""), "{json}");
    assert!(json.contains("\"size\": 512"), "{json}");
}

#[test]
fn nested_indentation_blocks() {
    let src = "outer:\n  middle:\n    inner = 42\n";
    let json = tree(src);
    assert!(json.contains("\"inner\": 42"), "{json}");
    // A real equivalence check: the brace-block form of the same tree.
    let braced = "outer {\n  middle {\n    inner = 42\n  }\n}\n";
    assert_eq!(json, tree(braced), "nested blocks must match nested bodies");
}

#[test]
fn indentation_is_cosmetic_inside_brackets() {
    // Multi-line arrays and inline tables suppress measurement entirely.
    let flat = "hosts = [\n{ name = \"a\" },\n{ name = \"b\" }\n]\n";
    let indented = "hosts = [\n    { name = \"a\" },\n        { name = \"b\" }\n]\n";
    assert_eq!(tree(flat), tree(indented));
}

#[test]
fn blank_lines_and_comments_do_not_change_the_tree() {
    let plain = "app:\n  name = \"svc\"\n";
    let noisy = "# leading comment\napp:\n\n  name = \"svc\"  # trailing\n\n# after\n";
    assert_eq!(tree(plain), tree(noisy));
}

#[test]
fn tab_indentation_is_e006() {
    let diags = codes("app:\n\tname = \"x\"\n");
    assert!(
        diags.contains(&"E006".to_string()),
        "expected E006, got {diags:?}"
    );
}

#[test]
fn inconsistent_dedent_is_e007() {
    let diags = codes("app:\n    name = \"x\"\n  port = 1\n");
    assert!(
        diags.contains(&"E007".to_string()),
        "expected E007, got {diags:?}"
    );
}

#[test]
fn header_without_body_is_e008() {
    let diags = codes("app:\n");
    assert!(
        diags.contains(&"E008".to_string()),
        "expected E008, got {diags:?}"
    );
}

#[test]
fn unexpected_indentation_is_e010_without_cascade() {
    let diags = codes("x = 1\n  y = 2\nz = 3\n");
    assert_eq!(
        diags,
        vec!["E010"],
        "exactly one E010, no cascade: {diags:?}"
    );
}

#[test]
fn unclosed_brace_block_is_e001() {
    let diags = codes("app {\n  name = \"x\"\n");
    assert!(
        diags.contains(&"E001".to_string()),
        "expected E001, got {diags:?}"
    );
}

#[test]
fn close_brace_cannot_close_an_indentation_block() {
    // spec/dual-syntax.md §5.3: a `}` at the block's own indentation is a
    // delimiter mismatch (E001) — indentation blocks close only by Dedent.
    let inside = codes("t:\n  a = 1\n  }\n");
    assert!(
        inside.contains(&"E001".to_string()),
        "expected E001 for `}}` inside the block, got {inside:?}"
    );
    // A `}` dedented out to document level is a stray brace (E002): the
    // Dedent legitimately closed the block first.
    let dedented = codes("t:\n  a = 1\n}\n");
    assert!(
        dedented.contains(&"E002".to_string()),
        "expected E002 for a stray `}}` at document level, got {dedented:?}"
    );
    assert!(
        !dedented.contains(&"E001".to_string()),
        "the block must close cleanly at its Dedent: {dedented:?}"
    );
}

#[test]
fn e010_recovery_preserves_enclosing_scope() {
    // A stray indent before the second block must not steal that block's
    // Dedent: `c` must remain a sibling of `a`, not nested inside it.
    let src = "x = 1\n  y = 2\na:\n  b = 1\nc:\n  d = 2\n";
    let (json, errs) = recovered(src);
    assert!(
        errs.contains(&"E010".to_string()),
        "expected E010: {errs:?}"
    );
    let a_start = json.find("\"a\"").expect("a present");
    let c_start = json.find("\"c\"").expect("c present");
    let between = &json[a_start..c_start];
    assert!(
        between.contains('}'),
        "`a`'s table must close before `c` begins: {json}"
    );
}

#[test]
fn e010_inside_a_block_keeps_later_statements_in_that_block() {
    let src = "t:\n  a = 1\n    b = 2\n  c = 3\n";
    let (json, errs) = recovered(src);
    assert!(
        errs.contains(&"E010".to_string()),
        "expected E010: {errs:?}"
    );
    assert!(
        json.contains("\"a\": 1") && json.contains("\"b\": 2") && json.contains("\"c\": 3"),
        "all three keys belong to t: {json}"
    );
}

#[test]
fn structural_nesting_is_bounded() {
    // 200 nested blocks must fail with E011, not overflow the stack.
    let mut src = String::new();
    for level in 0..200 {
        src.push_str(&"  ".repeat(level));
        src.push_str("k:\n");
    }
    src.push_str(&"  ".repeat(200));
    src.push_str("leaf = 1\n");
    let (_, errs) = recovered(&src);
    assert!(
        errs.contains(&"E011".to_string()),
        "expected E011 for over-deep nesting: {errs:?}"
    );
    // A legal depth still parses.
    let mut ok = String::new();
    for level in 0..10 {
        ok.push_str(&"  ".repeat(level));
        ok.push_str("k:\n");
    }
    ok.push_str(&"  ".repeat(10));
    ok.push_str("leaf = 1\n");
    assert!(parse(&ok, "dual").is_ok(), "10 levels are within the limit");
}

#[test]
fn mismatched_pair_does_not_disable_indentation_measurement() {
    // `[` and `{` open; a `]` that does not match `{` must not pop the
    // delimiter stack, so `y = 3` is still parsed at document level.
    let src = "x = [1, {a = 2]\ny = 3\n";
    let (json, errs) = recovered(src);
    assert!(!errs.is_empty(), "malformed delimiters must be diagnosed");
    assert!(
        json.contains("\"y\": 3"),
        "the following statement must still parse: {json}"
    );
}
