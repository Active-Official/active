//! L-P10: schema disjunctions (`OneOf`) and the `requires`
//! biconditional. Drives the real shipped path (`parse` → `Engine`).

use active_core::{parse, Diagnostic, Engine, EvalError, Value};

fn strict_engine() -> Engine {
    let mut engine = Engine::new();
    engine.resilient = false;
    engine
}

fn parse_ok(src: &str) -> active_core::Module {
    parse(src, "disjunction").unwrap_or_else(|diags| panic!("parse failed: {diags:?}"))
}

fn evaluate_strict(src: &str) -> Result<Value, Vec<Diagnostic>> {
    let module = parse_ok(src);
    match strict_engine().evaluate(&module) {
        Ok((value, diags)) => {
            assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
            Ok(value)
        }
        Err(EvalError::ValidationFailed { diagnostics }) => Err(diagnostics),
        Err(error) => panic!("unexpected evaluation error: {error:?}"),
    }
}

fn evaluate_resilient(src: &str) -> (Value, Vec<Diagnostic>) {
    let module = parse_ok(src);
    Engine::new()
        .evaluate(&module)
        .unwrap_or_else(|error| panic!("resilient evaluation failed: {error:?}"))
}

fn json(value: &Value) -> serde_json::Value {
    serde_json::from_str(&active_core::convert::value_to_pretty_json(value)).expect("json")
}

fn codes(diags: &[Diagnostic]) -> Vec<String> {
    let mut out: Vec<String> = diags.iter().map(|d| d.code.clone()).collect();
    out.sort();
    out
}

#[test]
fn one_of_accepts_every_branch() {
    let src = "schema = { port = OneOf(Int(min = 1), String(min_len = 2)) }\nport = 8080\n";
    let value = evaluate_strict(src).expect("the Int branch must accept");
    assert_eq!(json(&value)["port"], 8080);

    let src = "schema = { port = OneOf(Int(min = 1), String(min_len = 2)) }\nport = \"ab\"\n";
    let value = evaluate_strict(src).expect("the String branch must accept");
    assert_eq!(json(&value)["port"], "ab");
}

#[test]
fn one_of_rejects_a_value_no_branch_accepts() {
    let src = "schema = { port = OneOf(Int(min = 1), String(min_len = 2)) }\nport = true\n";
    let diags = evaluate_strict(src).expect_err("no branch accepts a bool");
    assert_eq!(codes(&diags), vec!["T002".to_string()], "{diags:?}");
    let message = &diags[0].message;
    assert!(
        message.contains("OneOf(Int(min = 1), String(min_len = 2))"),
        "{message}"
    );
    assert!(message.contains("branch 1:"), "{message}");
    assert!(message.contains("branch 2:"), "{message}");
}

#[test]
fn one_of_caps_branch_reasons_at_four() {
    let src =
        "schema = { v = OneOf(Int, Bool, Duration, Bytes, Null, Array<Int>) }\nv = \"text\"\n";
    let diags = evaluate_strict(src).expect_err("no branch accepts a string");
    let message = &diags[0].message;
    assert_eq!(message.matches("branch 1:").count(), 1, "{message}");
    assert_eq!(message.matches("branch 4:").count(), 1, "{message}");
    assert!(!message.contains("branch 5:"), "{message}");
    assert!(message.contains("(+2 more branches)"), "{message}");
}

#[test]
fn one_of_supports_nested_table_branches() {
    let src =
        "schema = { server = OneOf(Table { host = String }, String) }\nserver = { host = \"x\" }\n";
    evaluate_strict(src).expect("the table branch must accept");
    let src =
        "schema = { server = OneOf(Table { host = String }, String) }\nserver = \"https://x\"\n";
    evaluate_strict(src).expect("the string branch must accept");
    let src =
        "schema = { server = OneOf(Table { host = String }, String) }\nserver = { host = 1 }\n";
    let diags = evaluate_strict(src).expect_err("neither branch accepts this table");
    assert!(codes(&diags).contains(&"T002".to_string()), "{diags:?}");
}

#[test]
fn one_of_default_fills_only_when_missing_and_fits_a_branch() {
    let src = "schema = { port = OneOf(Int(min = 1), String, default = 8080) }\n";
    // Strict keeps explicitness: the key is still required.
    let diags = evaluate_strict(src).expect_err("strict requires explicit values");
    assert!(codes(&diags).contains(&"T004".to_string()), "{diags:?}");
    // Resilient fills the weakest disjunct.
    let (value, diags) = evaluate_resilient(src);
    assert_eq!(json(&value)["port"], 8080);
    assert!(
        diags.iter().any(|d| d.code == "W005"),
        "the fill must be recorded: {diags:?}"
    );
    // A source value wins over the default and is validated normally.
    let src = "schema = { port = OneOf(Int(min = 1), String, default = 8080) }\nport = \"x\"\n";
    let value = evaluate_strict(src).expect("the source value is the String branch");
    assert_eq!(json(&value)["port"], "x");
}

#[test]
fn a_default_that_fits_no_branch_is_a_schema_error() {
    // 5 is an Int type but violates the Int branch's bound, and it is
    // not a String: the default satisfies no branch.
    let src = "schema = { port = OneOf(Int(min = 10), String, default = 5) }\n";
    let diags = evaluate_strict(src).expect_err("the bad default is a schema defect");
    assert!(
        diags.iter().any(|d| d.code == "T002"
            && d.message
                .contains("schema default does not satisfy any branch")),
        "{diags:?}"
    );
    let (_, diags) = evaluate_resilient(src);
    // Resilient fills the default and validates it like a written value:
    // the fill reports the defect (one T002) and records W005. The eager
    // schema-defect check is skipped in that case so the same defect is
    // not reported twice (M4).
    let t002 = diags.iter().filter(|d| d.code == "T002").count();
    assert_eq!(t002, 1, "{diags:?}");
    assert!(
        diags.iter().any(|d| d.code == "W005"),
        "the fill is still recorded: {diags:?}"
    );
}

#[test]
fn one_of_coercion_needs_exactly_one_coercible_branch() {
    // "5" is not a Bool, so only the Int branch is coercible: resilient
    // applies it.
    let src = "schema = { v = OneOf(Int, Bool) }\nv = \"5\"\n";
    let (value, _) = evaluate_resilient(src);
    assert_eq!(json(&value)["v"], 5);
    // Two coercible branches are ambiguous: no branch is chosen.
    let src = "schema = { v = OneOf(Int, Float) }\nv = \"5\"\n";
    let module = parse_ok(src);
    let (_, diags) = Engine::new().evaluate(&module).expect("resilient");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "T002" && d.message.contains("more than one branch")),
        "{diags:?}"
    );
}

#[test]
fn requires_is_a_biconditional() {
    let schema = "schema = { mode = String, replicas = Int(requires = \"mode\") }\n";
    // Row 1: trigger enabled, field present.
    evaluate_strict(&format!("{schema}mode = \"on\"\nreplicas = 2\n"))
        .expect("enabled trigger with the field is valid");
    // Row 2: trigger enabled, field absent.
    let diags = evaluate_strict(&format!("{schema}mode = \"on\"\n"))
        .expect_err("enabled trigger requires the field");
    assert!(codes(&diags).contains(&"T004".to_string()), "{diags:?}");
    // Row 3: trigger disabled, field absent (B-001).
    evaluate_strict(&format!("{schema}mode = \"\"\n")).expect("disabled trigger excuses absence");
    // Row 4: trigger disabled, field present (new in L-P10).
    let schema = "schema = { on = Bool, replicas = Int(requires = \"on\") }\n";
    let diags = evaluate_strict(&format!("{schema}on = false\nreplicas = 2\n"))
        .expect_err("a present field with a disabled trigger is T010");
    assert_eq!(codes(&diags), vec!["T010".to_string()], "{diags:?}");
    assert!(
        diags[0]
            .message
            .contains("requires trigger 'on' is disabled"),
        "{:?}",
        diags[0].message
    );
}

#[test]
fn one_of_round_trips_through_the_formatter() {
    let src = "schema = { port = OneOf(Int(min = 1), String(min_len = 2), default = 8080) }\nport = 8080\n";
    let module = parse_ok(src);
    for style in [active_core::Style::Brace, active_core::Style::Indent] {
        let formatted = active_core::fmt::try_format(&module, style).expect("format");
        assert!(
            formatted.contains("OneOf(Int(min = 1), String(min_len = 2), default = 8080)"),
            "{style:?}:\n{formatted}"
        );
        let reparsed = parse(&formatted, "roundtrip").unwrap_or_else(|d| panic!("{d:?}"));
        let (value, diags) = Engine::new().evaluate(&reparsed).expect("evaluate");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(json(&value)["port"], 8080);
    }
}

#[test]
fn one_of_parses_arguments_and_nested_disjunctions() {
    // A nested OneOf is flattened; its branches keep their constraints.
    let src = "schema = { v = OneOf(OneOf(Int(min = 1), String), Bool) }\nv = \"x\"\n";
    let value = evaluate_strict(src).expect("the flattened String branch accepts");
    assert_eq!(json(&value)["v"], "x");
    // Entries may carry the same named arguments as leaves.
    let src = "schema = { v = OneOf(Int, String, requires = \"mode\", description = \"d\") , mode = String }\nv = 1\nmode = \"on\"\n";
    evaluate_strict(src).expect("arguments are stored on the entry");
}

#[test]
fn the_explain_table_covers_t006_and_t010() {
    // T006 is emitted by annotation constraints but had no explain entry
    // (L-P10 pre-review fix); T010 is minted here.
    let t006 = active_core::convert::explain_code("T006");
    assert!(!t006.is_empty(), "T006 must have an explain entry");
    let t010 = active_core::convert::explain_code("T010");
    assert!(t010.contains("requires"), "{t010}");
    // Unknown codes keep the shipped fallback wording.
    assert!(!active_core::convert::explain_code("T999").is_empty());
}

#[test]
fn a_losing_malformed_branch_does_not_poison_the_winner() {
    // H1: a malformed branch (incomparable bound) records a once-only
    // schema bug. The trial must restore that bookkeeping, or a later
    // trial of the same branch silently skips its own bug and accepts.
    let src = "schema = { v = OneOf(Int(min = \"bad\"), String, default = 5) }\nv = 5\n";
    let diags = evaluate_strict(src).expect_err("the malformed branch must not silently accept");
    assert!(
        diags.iter().any(|d| d.code == "T002" || d.code == "T003"),
        "{diags:?}"
    );
}

#[test]
fn nested_requires_resolves_sibling_and_root_triggers() {
    // Row 4 with a sibling-relative target inside a nested table.
    let src = "schema = { nested = Table { y = Bool, x = Int(requires = \"y\") } }\nnested = { y = false, x = 1 }\n";
    let diags = evaluate_strict(src).expect_err("sibling trigger is disabled");
    assert!(codes(&diags).contains(&"T010".to_string()), "{diags:?}");
    // Row 2/3 with a document-root target referenced from a nested table.
    let src = "schema = { on = Bool, nested = Table { x = Int(requires = \"on\") } }\non = true\nnested = { }\n";
    let diags = evaluate_strict(src).expect_err("root trigger is enabled");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "T004" && d.message.contains("'on'")),
        "{diags:?}"
    );
    let src = "schema = { on = Bool, nested = Table { x = Int(requires = \"on\") } }\non = true\nnested = { x = 1 }\n";
    evaluate_strict(src).expect("enabled root trigger with the field is valid");
    // Row 4 with a document-root target referenced from a nested table.
    let src = "schema = { on = Bool, nested = Table { x = Int(requires = \"on\") } }\non = false\nnested = { x = 1 }\n";
    let diags = evaluate_strict(src).expect_err("root trigger is disabled");
    assert!(codes(&diags).contains(&"T010".to_string()), "{diags:?}");
}

#[test]
fn a_degenerate_one_of_is_refused_and_never_installed() {
    // M1: `OneOf(Int)` is E020 and must not become a silent single type.
    use active_core::ParseOptions;
    let (module, diags) = active_core::parser::parse_recovering(
        "schema = { v = OneOf(Int) }\nv = \"x\"\n",
        "degenerate",
        ParseOptions::default(),
    );
    assert!(diags.iter().any(|d| d.code == "E020"), "{diags:?}");
    let schema = module.schema.expect("schema node");
    let field = match module.get(schema) {
        Some(active_core::ir::Node::SchemaTable { fields, .. }) => fields["v"],
        other => panic!("unexpected schema node: {other:?}"),
    };
    match module.get(field) {
        Some(active_core::ir::Node::SchemaField { field }) => {
            assert!(field.one_of.is_empty(), "degenerate OneOf must not install");
        }
        other => panic!("unexpected field node: {other:?}"),
    }
}

#[test]
fn duplicate_leaf_branches_are_one_branch() {
    // M3: `OneOf(Int, Int)` must coerce like a single Int, not report an
    // ambiguity between two identical branches.
    let src = "schema = { v = OneOf(Int, Int) }\nv = \"5\"\n";
    let (value, diags) = evaluate_resilient(src);
    assert_eq!(json(&value)["v"], 5, "{diags:?}");
    assert!(
        !diags
            .iter()
            .any(|d| d.message.contains("more than one branch")),
        "{diags:?}"
    );
    // Structural branches are distinct even when both are tables.
    let src = "schema = { v = OneOf(Table { a = Int }, Table { b = Int }) }\nv = { b = 1 }\n";
    evaluate_strict(src).expect("the second table branch must accept");
}

#[test]
fn a_bad_default_reports_once() {
    // M4: a missing key under resilient mode is validated by the fill;
    // the eager check must not add a second T002 for the same defect.
    let src = "schema = { v = OneOf(Int(min = 10), String, default = 5) }\n";
    let (_, diags) = evaluate_resilient(src);
    let t002 = diags.iter().filter(|d| d.code == "T002").count();
    assert_eq!(t002, 1, "{diags:?}");
}

#[test]
fn a_computed_requires_target_enforces_t010() {
    // Round-2 critical: the target is the evaluated `requires`
    // argument, not just a literal.
    let mut env = std::collections::HashMap::new();
    env.insert("T".to_string(), "mode".to_string());
    let engine = Engine::new().with_env(env);
    let module =
        parse_ok("schema = { mode = String, v = Int(requires = env.T) }\nmode = \"\"\nv = 1\n");
    let mut strict = engine.clone();
    strict.resilient = false;
    let diags = match strict.evaluate(&module) {
        Err(EvalError::ValidationFailed { diagnostics }) => diagnostics,
        Ok((_, diags)) => diags,
        Err(error) => panic!("unexpected: {error:?}"),
    };
    assert!(diags.iter().any(|d| d.code == "T010"), "{diags:?}");
    // The evaluated target is host content: T010 must not echo it.
    assert!(
        !diags.iter().any(|d| d.message.contains("'mode'")),
        "the computed target must not be echoed: {diags:?}"
    );
}

#[test]
fn distinct_pattern_branches_are_not_deduped() {
    let src = "schema = { v = OneOf(String(pattern = \"^a+$\"), String(pattern = \"^b+$\")) }\nv = \"bbb\"\n";
    let value = evaluate_strict(src).expect("the second pattern branch must accept");
    assert_eq!(json(&value)["v"], "bbb");
}

#[test]
fn flattening_cannot_exceed_the_branch_cap() {
    use active_core::ParseOptions;
    let mut outer = String::new();
    for group in 0..2 {
        outer.push_str("OneOf(");
        for index in 0..17 {
            if index > 0 {
                outer.push_str(", ");
            }
            outer.push_str(&format!("Int(min = {})", group * 17 + index));
        }
        outer.push_str("), ");
    }
    outer.pop();
    outer.pop();
    let src = format!("schema = {{ v = OneOf({outer}) }}\nv = 1\n");
    let (module, diags) =
        active_core::parser::parse_recovering(&src, "flatten-cap", ParseOptions::default());
    assert!(
        diags
            .iter()
            .any(|d| d.code == "E020" && d.message.contains("at most 32 branches")),
        "{diags:?}"
    );
    let schema = module.schema.expect("schema");
    let field = match module.get(schema) {
        Some(active_core::ir::Node::SchemaTable { fields, .. }) => fields["v"],
        other => panic!("{other:?}"),
    };
    match module.get(field) {
        Some(active_core::ir::Node::SchemaField { field }) => {
            assert!(
                field.one_of.is_empty(),
                "the over-cap list must not install"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_excused_field_does_not_fail_on_its_unused_bad_default() {
    // Round-2 low: strict + disabled trigger excused the field, but the
    // eager default check still failed the document.
    let src = "schema = { on = Bool, v = OneOf(Int(min = 10), String, default = 5, requires = \"on\") }\non = false\n";
    evaluate_strict(src).expect("the excused field's default is never used");
}

#[test]
fn an_excused_field_is_not_filled_in_resilient_mode() {
    // Round-3 high: filling a default on a disabled trigger would create
    // a present-while-disabled tree the biconditional forbids.
    let src = "schema = { on = Bool, v = OneOf(Int, String, default = 1, requires = \"on\") }\non = false\n";
    let (value, diags) = evaluate_resilient(src);
    assert!(json(&value).get("v").is_none(), "{diags:?}");
    assert!(
        diags
            .iter()
            .all(|d| d.code != "W005" && d.code != "T004" && d.code != "T010"),
        "{diags:?}"
    );
}

#[test]
fn a_dangling_requires_target_is_a_schema_typo() {
    // Round-3 medium: a literal target that names no path must not
    // silently switch conditionality off.
    let src = "schema = { v = Int(requires = \"mod\") }\nv = 1\n";
    let diags = evaluate_strict(src).expect_err("dangling target is a schema bug");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "T003" && d.message.contains("does not name a schema path")),
        "{diags:?}"
    );
    let src = "schema = { v = Int(requires = \"mod\") }\n";
    let diags = evaluate_strict(src).expect_err("the typo is reported either way");
    assert!(codes(&diags).contains(&"T003".to_string()), "{diags:?}");
}

#[test]
fn a_branch_level_requires_or_default_is_e020() {
    use active_core::ParseOptions;
    for (name, source) in [
        (
            "default",
            "schema = { v = OneOf(Int(default = 1), String) }\nv = 1\n",
        ),
        (
            "requires",
            "schema = { v = OneOf(Int(requires = \"x\"), String) }\nv = 1\n",
        ),
    ] {
        let (_, diags) =
            active_core::parser::parse_recovering(source, "branch-arg", ParseOptions::default());
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E020" && d.message.contains("OneOf branch")),
            "{name}: {diags:?}"
        );
    }
}

#[test]
fn an_ambiguous_default_is_refused() {
    // Round-3 low: the eager check must mirror validate_one_of's
    // ambiguity rule, not stop at the first side-effect win.
    let src = "schema = { v = OneOf(Int, Float, default = \"5\") }\n";
    let (_, diags) = evaluate_resilient(src);
    assert!(
        diags.iter().any(|d| d.code == "T002"),
        "two coercible branches cannot pick one: {diags:?}"
    );
}

#[test]
fn duplicate_table_branches_are_one_branch() {
    // Round-3 low: structural duplicates dedupe by content, so a value
    // does not become "ambiguous".
    let src = "schema = { v = OneOf(Table { a = Int }, Table { a = Int }) }\nv = { a = 1 }\n";
    evaluate_strict(src).expect("the duplicate table branches accept once");
}
