//! Formatter crash regression (libFuzzer, formatter target, 2026-11-15).
//!
//! The fuzzer found an input whose formatted output did not re-parse
//! (an unnamed `Ref` rendered as `<?>`) and, once that was fixed, whose
//! formatting was not idempotent (a reference to a table rendered as a
//! scalar expression on the first pass and as a block on the second).
//! Both must hold for every parseable input: output re-parses and
//! formatting reaches a fixed point.

use active_core::{fmt, parse, Style};

const CRASH: &str = "ye=o[o]yye=o[o]ye=o#[o]=e=o[o]=?v";

fn check(src: &str, style: Style) {
    let module = parse(src, "fmt-crash").expect("input parses");
    let once = fmt::format(&module, style);
    assert!(
        !once.contains("<?>"),
        "no unparseable placeholder may be emitted ({style:?}):\n{once}"
    );
    let reparsed = parse(&once, "fmt-crash")
        .unwrap_or_else(|d| panic!("formatted output must re-parse ({style:?}):\n{once}\n{d:?}"));
    let twice = fmt::format(&reparsed, style);
    assert_eq!(once, twice, "formatter must be idempotent ({style:?})");
}

#[test]
fn the_fuzz_crash_input_round_trips_in_both_styles() {
    check(CRASH, Style::Brace);
    check(CRASH, Style::Indent);
}

#[test]
fn references_to_tables_render_as_blocks() {
    // The minimal shape behind the crash: a key whose value is a
    // reference to a table.
    let src = "t = { a = 1 }\nr = t\n";
    let module = parse(src, "fmt-crash").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-crash").expect("re-parse");
        assert_eq!(once, fmt::format(&reparsed, style), "{style:?}:\n{once}");
    }
}

#[test]
fn an_aliased_empty_table_renders_inline_and_stays_idempotent() {
    // Fuzz seed crash-f438d19: `ye` resolves to an EMPTY table through a
    // Ref. The empty-vs-block decision read the Ref node (not a table), so
    // pass one emitted `ye {\n}`; that re-parses as an empty table node
    // whose inline form is `ye = {}`, so pass two differed.
    let src = "e=o[o]+++++[o]ye=o[oo]";
    let module = parse(src, "fmt-alias-empty").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-alias-empty").expect("re-parse");
        let twice = fmt::format(&reparsed, style);
        assert_eq!(once, twice, "{style:?} not idempotent:\n{once}");
        assert!(
            !once.contains("ye {\n}"),
            "{style:?} must emit the inline empty form: {once}"
        );
    }

    // The same shape by hand: an alias to an empty table.
    let src = "empty = {}\nalias = empty\n";
    let module = parse(src, "fmt-alias-empty-2").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-alias-empty-2").expect("re-parse");
        assert_eq!(once, fmt::format(&reparsed, style), "{style:?}:\n{once}");
    }
}

#[test]
fn a_parent_trailing_comment_hoists_into_an_aliased_last_table() {
    // Fuzz seed crash-8e989b62: a block's trailing comment must land inside
    // the nested block the parser will re-attach it to. The last entry was
    // an ALIAS to a table, so the hoist (which looked at the raw node) did
    // not fire, the comment stayed after the closing brace, and the next
    // parse moved it inside - a non-idempotent rewrite.
    let src = "ye=o[o]yye=o[o]ye=o\n\n\n\n#[o}=e\n\n=\n";
    let module = parse(src, "fmt-hoist").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-hoist").expect("re-parse");
        let twice = fmt::format(&reparsed, style);
        assert_eq!(once, twice, "{style:?} not idempotent:\n{once}");
    }

    // The minimal hand-written shape: a block whose last entry aliases a
    // table, with the block's trailing comment after it.
    let src = "[a]\nx = 1\n[b]\nc = { d = 1 }\ne = c\n# trailing\n";
    let module = parse(src, "fmt-hoist-2").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-hoist-2").expect("re-parse");
        assert_eq!(once, fmt::format(&reparsed, style), "{style:?}:\n{once}");
    }
}

#[test]
fn a_header_form_schema_is_metadata_and_formats_stably() {
    // Fuzz seed crash-504a8155: `[schema]` used to parse as a DATA table
    // named schema. Indent style renders such a table with schema syntax
    // (`schema = {}` / `schema:`), so the next parse saw a schema block and
    // the output moved to the canonical schema position - non-idempotent,
    // and a meaning change. The grammar makes `schema` contextual on a
    // table value (spec/grammar.md section 2.10), so the header form is a
    // schema block in every style.
    let src = "prompte=o[schema]";
    let module = parse(src, "fmt-hdr-schema").expect("parse");
    assert!(
        module.schema.is_some(),
        "a header-form schema is schema metadata"
    );
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-hdr-schema").expect("re-parse");
        assert_eq!(once, fmt::format(&reparsed, style), "{style:?}:\n{once}");
    }
}

#[test]
fn repeated_patch_comments_are_emitted_once_each() {
    // Local random search: comments attach by qualified key, and two
    // `@remove("a")` statements share one. Re-reading the bucket emitted
    // the same comment once per patch, and the next pass doubled it again.
    // Input found by the search at 371066.
    let src = "# cx@remove(\"a\")\n@remove(\"a\")\n@remove(\"a\")\n";
    let module = parse(src, "fmt-patch-dup").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-patch-dup").expect("re-parse");
        let twice = fmt::format(&reparsed, style);
        assert_eq!(once, twice, "{style:?} not idempotent:\n{once}");
        assert_eq!(
            once.matches("# cx").count(),
            1,
            "{style:?} emits the comment once: {once:?}"
        );
    }
}

#[test]
fn a_reference_to_a_schema_node_renders_by_name() {
    // Local random search (seed 33): an expression that resolved to the
    // schema node had no name in the formatter's index, so the Ref arm fell
    // through to `<?>` and the output did not re-parse at all.
    let src = "t1schemat = schema+Int(min = 1)[schema    ]";
    let module = parse(src, "fmt-schema-ref").expect("parse");
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        assert!(
            !once.contains("<?>"),
            "{style:?} must not emit an unparseable placeholder: {once:?}"
        );
        let reparsed = parse(&once, "fmt-schema-ref").expect("re-parse");
        assert_eq!(once, fmt::format(&reparsed, style), "{style:?}:\n{once}");
    }
}

#[test]
fn duplicate_key_comments_keep_their_boundary() {
    // LANG-D-002: the surviving statement is the LAST duplicate, so its own
    // trailing comment stays on the line and earlier duplicates' comments
    // become leading lines above it. Concatenating (`x = 2 # first #
    // second`) would make two independent comments indistinguishable.
    //
    // The CLI refuses duplicate-key documents outright (LANG-D-001: W011 is
    // a repair), so this form is pinned at the library level - the path the
    // language server and programmatic callers use.
    let src = "[app]\nx = 1 # first\nx = 2 # second\n";
    let module = parse(src, "fmt-dup-comments").expect("parse");
    assert_eq!(
        fmt::format(&module, Style::Brace),
        "[app]\n# first\nx = 2 # second\n"
    );
    assert_eq!(
        fmt::format(&module, Style::Indent),
        "app:\n  # first\n  x = 2 # second\n"
    );
    for style in [Style::Brace, Style::Indent] {
        let once = fmt::format(&module, style);
        let reparsed = parse(&once, "fmt-dup-comments").expect("re-parse");
        assert_eq!(once, fmt::format(&reparsed, style), "{style:?}");
    }
}

#[test]
fn root_scalars_hoist_above_tables_in_both_styles() {
    // LANG-D-003: hoisting is required for brace style (a `key = value`
    // line after a header would be absorbed) and applied to indent style for
    // uniformity. spec/formatter.md section 1 states it, and the corpus
    // files tests/fmt/{brace,indent}/root-scalar-after-table.active pin the
    // canonical bytes.
    let src = "t:\n  a = 1\ntop = 2\n";
    let module = parse(src, "fmt-hoist").expect("parse");
    assert_eq!(
        fmt::format(&module, Style::Indent),
        "top = 2\n\nt:\n  a = 1\n"
    );
    let brace_src = "t { a = 1 }\ntop = 2\n";
    let module = parse(brace_src, "fmt-hoist-brace").expect("parse");
    assert_eq!(
        fmt::format(&module, Style::Brace),
        "top = 2\n\n[t]\na = 1\n"
    );
}
