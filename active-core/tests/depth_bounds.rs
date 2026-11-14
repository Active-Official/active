//! Depth-budget boundary and recovery tests (LANG-A-003, A-004, A-006).
//!
//! Counting rule: the documented budget (64) is the number of *usable*
//! nesting levels. 64 parses, 65 trips — for parentheses, unary chains,
//! brace blocks, and schema wrappers alike. A single depth failure stays
//! a single root-cause diagnostic: recovery skips the offending balanced
//! construct instead of bubbling one error per enclosing level, and it
//! never fabricates a column of null elements.

use active_core::{parse, parse_recovering};

fn codes(src: &str) -> Vec<String> {
    match parse(src, "depth") {
        Ok(m) => m.healing.iter().map(|d| d.code.clone()).collect(),
        Err(diags) => diags.iter().map(|d| d.code.clone()).collect(),
    }
}

#[test]
fn parentheses_use_the_full_documented_budget() {
    let ok = format!("x = {}1{}\n", "(".repeat(64), ")".repeat(64));
    assert_eq!(codes(&ok), Vec::<String>::new(), "64 levels parse");
    let over = format!("x = {}1{}\n", "(".repeat(65), ")".repeat(65));
    assert_eq!(codes(&over), vec!["E004".to_string()], "65 trips once");
}

#[test]
fn unary_chains_use_the_full_documented_budget() {
    let ok = format!("x = {}true\n", "!".repeat(64));
    assert_eq!(codes(&ok), Vec::<String>::new());
    let over = format!("x = {}true\n", "!".repeat(65));
    assert_eq!(codes(&over), vec!["E004".to_string()]);
}

#[test]
fn brace_blocks_use_the_full_documented_budget() {
    let ok = "t0 {\n".repeat(64) + "x = 1\n" + &"}\n".repeat(64);
    assert_eq!(codes(&ok), Vec::<String>::new(), "64 nested blocks parse");
    let over = "t0 {\n".repeat(65) + "x = 1\n" + &"}\n".repeat(65);
    assert_eq!(codes(&over), vec!["E011".to_string()], "65 trips once");
}

#[test]
fn schema_wrappers_use_the_full_documented_budget() {
    let ok = format!(
        "schema = {{ x = {}Int{} }}\n",
        "Array<".repeat(64),
        ">".repeat(64)
    );
    assert!(!codes(&ok).iter().any(|c| c == "E004"), "64 wrappers parse");
    let over = format!(
        "schema = {{ x = {}Int{} }}\n",
        "Array<".repeat(65),
        ">".repeat(65)
    );
    assert!(codes(&over).iter().any(|c| c == "E004"), "65 trips");
}

#[test]
fn one_depth_failure_is_one_diagnostic() {
    // Before the recovery fix, 70 parentheses produced 1 E004 plus 65
    // E001 follow-ons; 70-deep arrays produced 6 E004.
    let parens = format!("x = {}1{}\n", "(".repeat(70), ")".repeat(70));
    assert_eq!(codes(&parens), vec!["E004".to_string()]);
    let arrays = format!("x = {}1{}\n", "[".repeat(70), "]".repeat(70));
    assert_eq!(codes(&arrays), vec!["E004".to_string()]);
    // 70 brace blocks: the leaf is dropped with ONE E011 naming the bound.
    let blocks = "t0 {\n".repeat(70) + "x = 1\n" + &"}\n".repeat(70);
    assert_eq!(codes(&blocks), vec!["E011".to_string()]);
}

#[test]
fn over_deep_array_recovery_does_not_fabricate_elements() {
    // The recovering parse (LSP, formatter, tooling) must not invent a
    // column of nulls: the guarded construct is replaced by one null at
    // the deep end, never N-63 nulls.
    let src = format!("x = {}1{}\n", "[".repeat(70), "]".repeat(70));
    let (module, diags) = parse_recovering(&src, "depth", Default::default());
    assert!(
        diags.iter().filter(|d| d.code == "E004").count() == 1,
        "one root-cause E004 in recovery"
    );
    // Count Literal(Null) nodes: the guard's placeholder plus at most the
    // few the enclosing recovery arms create — never ~70.
    let nulls = module
        .arena
        .iter()
        .filter(|n| matches!(n, active_core::Node::Literal(active_core::Value::Null)))
        .count();
    assert!(nulls <= 3, "recovery fabricated {nulls} null nodes");
}
