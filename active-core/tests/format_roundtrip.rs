//! Formatter round-trip tests (C-022).
//!
//! Proves the dual-syntax guarantees from spec/dual-syntax.md §8.1 on the
//! real path: canonical documents round-trip byte-for-byte, formatting is
//! idempotent, comments survive in every mode, and the two styles produce
//! the same tree.

use active_core::fmt::format;
use active_core::{parse, Engine, Style};

fn parse_ok(src: &str) -> active_core::Module {
    parse(src, "fmt").unwrap_or_else(|d| panic!("parse failed: {d:?}"))
}

fn fmt(src: &str, style: Style) -> String {
    format(&parse_ok(src), style)
}

fn tree(src: &str) -> String {
    let module = parse_ok(src);
    let engine = Engine::new();
    let (value, _) = engine.evaluate(&module).expect("must evaluate");
    active_core::convert::value_to_pretty_json(&value)
}

const BRACE_DOC: &str = "[app]\nname = \"svc\"\nport = 8080\n\n[database]\nhost = \"db\"\n";

const INDENT_DOC: &str = "app:\n  name = \"svc\"\n  port = 8080\n\ndatabase:\n  host = \"db\"\n";

#[test]
fn canonical_brace_document_round_trips_byte_for_byte() {
    assert_eq!(fmt(BRACE_DOC, Style::Brace), BRACE_DOC);
    // Preserve mode of a canonical brace doc is the same bytes.
    assert_eq!(fmt(BRACE_DOC, Style::Preserve), BRACE_DOC);
}

#[test]
fn schema_evolution_metadata_round_trips() {
    // PROPOSAL §2.4: version/min_reader/min_writer and removed_in
    // survive formatting (idempotent), so fmt never drops the
    // compatibility contract.
    let src = "schema = {\n  version = \"1.2.0\",\n  min_reader = \"1.0.0\",\n  old = String(removed_in = \"1.3.0\"),\n  port = Int,\n}\nport = 1\n";
    for style in [Style::Brace, Style::Indent, Style::Preserve] {
        let once = fmt(src, style);
        assert!(once.contains("version = \"1.2.0\""), "{once}");
        assert!(once.contains("min_reader = \"1.0.0\""), "{once}");
        assert!(once.contains("removed_in = \"1.3.0\""), "{once}");
        let twice = fmt(&once, style);
        assert_eq!(once, twice, "not idempotent for {style:?}:\n{once}");
    }
}

#[test]
fn canonical_indent_document_round_trips_byte_for_byte() {
    assert_eq!(fmt(INDENT_DOC, Style::Indent), INDENT_DOC);
    assert_eq!(fmt(INDENT_DOC, Style::Preserve), INDENT_DOC);
}

#[test]
fn formatting_is_idempotent() {
    for src in [BRACE_DOC, INDENT_DOC] {
        for style in [Style::Brace, Style::Indent, Style::Preserve] {
            let once = fmt(src, style);
            let twice = fmt(&once, style);
            assert_eq!(once, twice, "not idempotent for {style:?}:\n{once}");
        }
    }
}

#[test]
fn formatting_is_idempotent_on_the_whole_positive_corpus() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/positive");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("positive corpus")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "active"))
        .collect();
    files.sort();
    assert!(files.len() >= 20, "expected the full corpus");
    for f in files {
        let src = std::fs::read_to_string(&f).expect("read");
        let module = parse(&src, "fmt").unwrap_or_else(|d| panic!("{f:?}: {d:?}"));
        for style in [Style::Brace, Style::Indent] {
            let once = format(&module, style);
            let reparsed = parse(&once, "fmt").unwrap_or_else(|d| {
                panic!("{f:?}: formatted output must re-parse ({style:?}): {d:?}\n{once}")
            });
            let twice = format(&reparsed, style);
            assert_eq!(once, twice, "not idempotent for {f:?} ({style:?})");
            // The formatted output must describe the same tree, and must
            // succeed or fail exactly like the original.
            let engine = Engine::new();
            match (engine.evaluate(&module), engine.evaluate(&reparsed)) {
                (Ok((a, _)), Ok((b, _))) => {
                    let (ja, jb) = (
                        active_core::convert::value_to_pretty_json(&a),
                        active_core::convert::value_to_pretty_json(&b),
                    );
                    assert_eq!(
                        ja, jb,
                        "{f:?}: formatted output changed the tree ({style:?})"
                    );
                }
                (Err(_), Err(_)) => {}
                (a, b) => panic!(
                    "{f:?}: formatting changed the evaluation outcome ({style:?}): \
                     original ok={}, formatted ok={}",
                    a.is_ok(),
                    b.is_ok()
                ),
            }
        }
    }
}

#[test]
fn comment_placement_is_exact() {
    // Placement follows spec/dual-syntax.md §7.2: a comment between the last
    // key of a table and the next table header has no following statement in
    // that block, so it is that block's trailing comment.
    let src = "# document header\n[app]\n# leading on name\nname = \"svc\" # trailing\n# before database\n\n[database]\nhost = \"db\"\n";
    let out = fmt(src, Style::Brace);
    assert_eq!(out, src, "canonical comment placement must round-trip");
    for needle in [
        "# document header",
        "# leading on name",
        "# trailing",
        "# before database",
    ] {
        assert_eq!(out.matches(needle).count(), 1, "{needle:?} in:\n{out}");
    }
    // Placement is stable under re-formatting.
    assert_eq!(
        fmt(&out, Style::Brace),
        out,
        "comment placement must be idempotent"
    );
}

#[test]
fn trailing_comment_stays_on_a_table_header_line() {
    let out = fmt("[app] # the app table\nname = \"svc\"\n", Style::Brace);
    assert!(
        out.starts_with("[app] # the app table"),
        "header comment must stay on the header line:\n{out}"
    );
}

#[test]
fn trailing_comments_survive_on_every_header_form() {
    // Indentation-style header.
    let indent_src = "app: # the app block\n  name = \"svc\"\n";
    let indent_out = fmt(indent_src, Style::Indent);
    assert!(
        indent_out.starts_with("app: # the app block"),
        "indent header comment lost:\n{indent_out}"
    );

    // Nested brace header.
    let nested_src = "outer {\n  inner { # the inner block\n    a = 1\n  }\n}\n";
    let nested_out = fmt(nested_src, Style::Brace);
    assert!(
        nested_out.contains("inner { # the inner block"),
        "nested header comment lost:\n{nested_out}"
    );

    // Empty table emitted as an inline value in indentation style.
    let empty_src = "[outer]\nempty = {} # the empty table\n";
    let empty_out = fmt(empty_src, Style::Indent);
    assert!(
        empty_out.contains("empty = {} # the empty table"),
        "empty-table comment lost:\n{empty_out}"
    );
}

#[test]
fn duplicate_key_comments_are_not_dropped() {
    // The IR keeps the last statement for a duplicate key; neither comment
    // may vanish.
    let out = fmt("[app]\nx = 1 # first\nx = 2 # second\n", Style::Brace);
    assert!(out.contains("# first"), "{out}");
    assert!(out.contains("# second"), "{out}");
}

#[test]
fn empty_table_keeps_its_trailing_comment_in_indent_style() {
    let src = "outer:\n  empty = {}\n  # belongs to outer\n";
    let out = fmt(src, Style::Indent);
    assert!(
        out.contains("# belongs to outer"),
        "block comment must survive the empty-table path:\n{out}"
    );
}

#[test]
fn large_and_small_floats_re_parse() {
    let src = "[nums]\nbig = 100000000000000000000.0\ntiny = 0.00000000000000000001\nplain = 2.0\n";
    let module = parse_ok(src);
    let out = format(&module, Style::Brace);
    let reparsed = parse(&out, "fmt")
        .unwrap_or_else(|d| panic!("formatted floats must re-parse: {d:?}\n{out}"));
    let engine = Engine::new();
    let (a, _) = engine.evaluate(&module).expect("original evaluates");
    let (b, _) = engine.evaluate(&reparsed).expect("formatted evaluates");
    assert_eq!(
        active_core::convert::value_to_pretty_json(&a),
        active_core::convert::value_to_pretty_json(&b),
        "float values must survive formatting:\n{out}"
    );
}

#[test]
fn env_default_operand_is_parenthesized() {
    // `(env.X ?: 1) + 2` must keep its grouping: the default swallows the
    // rest of the expression otherwise.
    let src = "[calc]\nv = (env.ACTIVE_FMT_TEST ?: 1) + 2\n";
    let module = parse_ok(src);
    let out = format(&module, Style::Brace);
    assert!(
        out.contains("(env.ACTIVE_FMT_TEST ?: 1) + 2"),
        "env default operand must be parenthesized:\n{out}"
    );
    let reparsed = parse(&out, "fmt").expect("must re-parse");
    let engine = Engine::new();
    let (a, _) = engine.evaluate(&module).expect("original evaluates");
    let (b, _) = engine.evaluate(&reparsed).expect("formatted evaluates");
    assert_eq!(
        active_core::convert::value_to_pretty_json(&a),
        active_core::convert::value_to_pretty_json(&b)
    );
}

#[test]
fn long_left_associative_chains_do_not_overflow() {
    // 20k terms: the tree is deep on the left spine, so rendering must be
    // iterative.
    let mut src = String::from("[calc]\nv = 1");
    for _ in 0..20_000 {
        src.push_str(" + 1");
    }
    src.push('\n');
    let module = parse_ok(&src);
    let out = format(&module, Style::Brace);
    assert!(out.contains("+ 1"), "chain must be rendered");
    parse(&out, "fmt").expect("formatted chain must re-parse");
}

#[test]
fn comments_survive_every_mode() {
    let src = "# document header\napp:\n  # leading on name\n  name = \"svc\"  # trailing\n  port = 8080\n  # trailing block\n\n# before database\n[database]\nhost = \"db\"\n";
    let module = parse_ok(src);
    assert_eq!(module.trivia.len(), 5, "all five comments must be recorded");
    for style in [Style::Brace, Style::Indent, Style::Preserve] {
        let out = format(&module, style);
        for needle in [
            "# document header",
            "# leading on name",
            "# trailing",
            "# trailing block",
            "# before database",
        ] {
            assert!(out.contains(needle), "{needle:?} lost in {style:?}:\n{out}");
        }
    }
}

#[test]
fn annotations_are_preserved_like_comments() {
    let src =
        "[app]\n# @intent: application identity\nname = \"svc\"\n# @ai_immutable\nport = 8080\n";
    let out = fmt(src, Style::Indent);
    assert!(out.contains("# @intent: application identity"), "{out}");
    assert!(out.contains("# @ai_immutable"), "{out}");
}

#[test]
fn styles_agree_on_the_tree_and_produce_each_other() {
    // Formatting the indentation document as brace style must produce the
    // canonical brace document, and vice versa.
    assert_eq!(fmt(INDENT_DOC, Style::Brace), BRACE_DOC);
    assert_eq!(fmt(BRACE_DOC, Style::Indent), INDENT_DOC);
    assert_eq!(tree(BRACE_DOC), tree(INDENT_DOC));
}

#[test]
fn preserve_mode_follows_the_source_style() {
    let from_indent = fmt(INDENT_DOC, Style::Preserve);
    assert!(from_indent.contains("app:"), "{from_indent}");
    let from_brace = fmt(BRACE_DOC, Style::Preserve);
    assert!(from_brace.contains("[app]"), "{from_brace}");
}

#[test]
fn let_bindings_and_expressions_round_trip() {
    let src = "let region = env.AWS_REGION ?: \"us-east-1\"\nlet timeout = 5s\n\n[app]\nregion = region\nurl = \"https://api.${region}.example.com\"\nwindow = timeout * 2\nflag = !false\nratio = 1.5\n";
    let module = parse_ok(src);
    for style in [Style::Brace, Style::Indent] {
        let once = format(&module, style);
        let reparsed = parse(&once, "fmt")
            .unwrap_or_else(|d| panic!("must re-parse ({style:?}): {d:?}\n{once}"));
        let twice = format(&reparsed, style);
        assert_eq!(once, twice, "not idempotent ({style:?})");
    }
}

#[test]
fn precedence_needs_no_extra_parentheses_but_keeps_meaning() {
    let src = "[calc]\na = 2 + 3 * 4\nb = (2 + 3) * 4\nc = 10 - 2 - 3\n";
    let module = parse_ok(src);
    let out = format(&module, Style::Brace);
    let reparsed = parse(&out, "fmt").expect("formatted output must re-parse");
    let engine = Engine::new();
    let (a, _) = engine.evaluate(&module).expect("evaluate original");
    let (b, _) = engine.evaluate(&reparsed).expect("evaluate formatted");
    assert_eq!(
        active_core::convert::value_to_pretty_json(&a),
        active_core::convert::value_to_pretty_json(&b),
        "parenthesization must preserve meaning:\n{out}"
    );
    assert!(
        out.contains("(2 + 3) * 4"),
        "explicit grouping must survive:\n{out}"
    );
}

#[test]
fn schema_evolution_metadata_survives_formatting() {
    // An empty schema with min_reader/min_writer (no version) must not
    // collapse to `{}` and lose its contract.
    let src = "schema = { min_reader = \"1.0.0\", min_writer = \"1.1.0\" }\n";
    let module = parse_ok(src);
    for style in [Style::Brace, Style::Indent, Style::Preserve] {
        let out = format(&module, style);
        assert!(out.contains("min_reader = \"1.0.0\""), "{style:?}: {out}");
        assert!(out.contains("min_writer = \"1.1.0\""), "{style:?}: {out}");
    }
}

#[test]
fn formatter_contract_holds_on_mutated_corpus_inputs() {
    // Deterministic mutation sweep standing in for the formatter fuzz
    // target on hosts where libFuzzer cannot run: the same contract
    // (no panic, output re-parses, formatting is idempotent) over
    // pseudo-random edits of real corpus files.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/positive");
    let mut sources: Vec<String> = std::fs::read_dir(&dir)
        .expect("positive corpus")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "active"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();
    sources.sort();
    assert!(!sources.is_empty());

    // LCG: fixed seed, so a failure reproduces exactly.
    let mut state: u64 = 0x5dce_2a17_9f31_c4b1;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state
    };

    let mut checked = 0;
    for _ in 0..1500 {
        let base = &sources[(next() % sources.len() as u64) as usize];
        let mut lines: Vec<String> = base.lines().map(str::to_string).collect();
        if lines.is_empty() {
            continue;
        }
        match next() % 4 {
            0 => {
                let index = (next() % lines.len() as u64) as usize;
                lines.remove(index);
            }
            1 => {
                let index = (next() % lines.len() as u64) as usize;
                let copy = lines[index].clone();
                lines.insert(index, copy);
            }
            2 => {
                let index = (next() % lines.len() as u64) as usize;
                let line = &mut lines[index];
                if !line.is_empty() {
                    let at = (next() % line.len() as u64) as usize;
                    // Include CR: line-ending artifacts in comments are
                    // where the fuzz target found the idempotence bug.
                    let ch = if next() % 4 == 0 { '\r' } else { ' ' };
                    line.insert(at, ch);
                }
            }
            _ => {
                lines.push("mutated = \"value\"".to_string());
            }
        }
        let mutated = lines.join("\n") + "\n";
        let Ok(module) = parse(&mutated, "fmt-mutation") else {
            continue;
        };
        for style in [Style::Brace, Style::Indent] {
            let once = format(&module, style);
            let reparsed = match parse(&once, "fmt-mutation") {
                Ok(module) => module,
                Err(diagnostics) => panic!(
                    "formatted output must re-parse ({style:?}): {diagnostics:?}\ninput:\n{mutated}\noutput:\n{once}"
                ),
            };
            let twice = format(&reparsed, style);
            assert_eq!(
                once, twice,
                "not idempotent ({style:?})\ninput:\n{mutated}\noutput:\n{once}"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 500,
        "mutation sweep exercised too few inputs: {checked}"
    );
}

#[test]
fn secret_env_names_without_a_bare_form_round_trip() {
    // Found by the mutation sweep: `env.NAME` cannot spell a name with
    // spaces, so the formatter keeps the string form instead of
    // emitting an unquoted name that re-parses shorter.
    let src = "k = secret.env(\"NAME WITH SPACE\")\n";
    let once = fmt(src, Style::Brace);
    assert!(once.contains("secret.env(\"NAME WITH SPACE\")"), "{once}");
    assert_eq!(fmt(&once, Style::Brace), once);
    // Identifier-shaped names still canonicalize to the bare form.
    let bare = fmt("k = secret.env(\"PORT\")\n", Style::Brace);
    assert!(bare.contains("secret(env.PORT)"), "{bare}");
}

#[test]
fn carriage_returns_in_comments_do_not_break_idempotence() {
    // Found by the formatter fuzz target: a lone CR emitted before the
    // formatter's newline re-parses as CRLF and disappears on the next
    // pass. Comments are non-semantic, so CRs are dropped.
    let src = "\r#\r\r\"\"\\\r\r\r";
    let module = parse(src, "cr").expect("parses");
    let once = format(&module, Style::Brace);
    let reparsed = parse(&once, "cr2").expect("formatted output re-parses");
    assert_eq!(format(&reparsed, Style::Brace), once);
    assert!(!once.contains('\r'), "{once:?}");
}
