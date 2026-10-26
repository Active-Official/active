//! Language server tests (C-043, spec/lsp.md §7).
//!
//! Handler unit tests (no I/O) over parsed fixtures, plus one stdio
//! end-to-end test driving the real binary through initialize →
//! didOpen → hover/definition → shutdown.

use active_ls::analysis;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};

fn untitled(name: &str) -> lsp_types::Url {
    lsp_types::Url::parse(&format!("untitled:{name}")).expect("uri")
}

fn open_untitled(name: &str, text: &str) -> analysis::Doc {
    analysis::open_doc(untitled(name), text.to_string(), 1, None)
}

fn no_lookup() -> impl Fn(&str) -> Option<String> {
    |_: &str| None
}

// ------------------------------------------------------------------
// Hover.
// ------------------------------------------------------------------

#[test]
fn hover_shows_type_value_origin_and_intent() {
    let doc = open_untitled(
        "hover",
        "# @intent: \"Public port\"\n# @constraint: value >= 1\nport = 8080\n",
    );
    let hover = analysis::hover_at(&doc, 2, 1).expect("hover");
    let lsp_types::HoverContents::Markup(content) = hover.contents else {
        panic!("markup expected");
    };
    assert!(content.value.contains("**port**"), "{}", content.value);
    assert!(content.value.contains("8080"), "{}", content.value);
    assert!(content.value.contains("Public port"), "{}", content.value);
    assert!(content.value.contains("value >= 1"), "{}", content.value);
}

#[test]
fn hover_redacts_secrets() {
    let doc = open_untitled("secret", "token = secret(\"s3cr3t-hover\")\n");
    let hover = analysis::hover_at(&doc, 0, 1).expect("hover");
    let lsp_types::HoverContents::Markup(content) = hover.contents else {
        panic!("markup expected");
    };
    assert!(!content.value.contains("s3cr3t-hover"), "{}", content.value);
    assert!(content.value.contains("***SECRET***"), "{}", content.value);
}

#[test]
fn hover_on_unknown_word_is_none() {
    // A comment carries no symbol and owns no value: still nothing.
    // (Cursor positions on literals now resolve to the owning key by
    // design — spec/lsp.md §2 — so this uses a comment, not `1`.)
    let doc = open_untitled("none", "port = 1\n# just a note\n");
    assert!(analysis::hover_at(&doc, 1, 4).is_none());
}

// ------------------------------------------------------------------
// Definition.
// ------------------------------------------------------------------

#[test]
fn definition_resolves_let_bindings() {
    let doc = open_untitled("def", "let limit = 5\nport = limit\n");
    // `limit` use on line 1 (0-based).
    let locations = analysis::definition_at(&doc, &no_lookup(), 1, 7);
    assert_eq!(locations.len(), 1);
    let range = locations[0].range;
    assert_eq!((range.start.line, range.start.character), (0, 4));
}

#[test]
fn definition_resolves_keys() {
    let doc = open_untitled("defkey", "[server]\nhost = \"h\"\nurl = host\n");
    let locations = analysis::definition_at(&doc, &no_lookup(), 2, 6);
    assert_eq!(locations.len(), 1);
    // `host` defined on line 1.
    assert_eq!(locations[0].range.start.line, 1);
}

// ------------------------------------------------------------------
// References.
// ------------------------------------------------------------------

#[test]
fn references_cover_interpolations_and_patches() {
    let doc = open_untitled(
        "refs",
        "let base = \"x\"\nurl = \"https://${base}/v1\"\n@patch(\"url\", base)\n",
    );
    // References of `base` (declaration excluded).
    let locations = analysis::references_at(&doc, &no_lookup(), 0, 5, false);
    assert_eq!(locations.len(), 2, "{locations:?}");
    // Declaration included on request.
    let locations = analysis::references_at(&doc, &no_lookup(), 0, 5, true);
    assert_eq!(locations.len(), 3, "{locations:?}");
    // Sorted deterministically by position.
    let lines: Vec<u32> = locations.iter().map(|l| l.range.start.line).collect();
    assert_eq!(lines, vec![0, 1, 2]);
}

// ------------------------------------------------------------------
// Rename.
// ------------------------------------------------------------------

#[test]
fn rename_produces_edits_and_refusals() {
    let doc = open_untitled("rename", "let limit = 5\nport = limit\n");
    let edit = analysis::rename_at(&doc, &no_lookup(), 0, 5, "cap").expect("rename must succeed");
    let changes = edit.changes.expect("changes");
    assert_eq!(changes.len(), 1);
    let edits = &changes[&untitled("rename")];
    assert_eq!(edits.len(), 2, "declaration + use");
    assert!(edits.iter().all(|e| e.new_text == "cap"));
    // Taken name refused.
    assert!(analysis::rename_at(&doc, &no_lookup(), 0, 5, "port").is_err());
    // Invalid identifier refused.
    assert!(analysis::rename_at(&doc, &no_lookup(), 0, 5, "has space").is_err());
    // Immutable paths refused.
    let doc = open_untitled("imm", "# @ai_immutable: true\nport = 1\n");
    assert!(analysis::rename_at(&doc, &no_lookup(), 1, 1, "p2").is_err());
}

// ------------------------------------------------------------------
// Semantic tokens.
// ------------------------------------------------------------------

#[test]
fn semantic_tokens_encode_and_start_at_zero() {
    let absolute = analysis::semantic_tokens_absolute("let port = 8080\n# note\n");
    assert!(!absolute.is_empty());
    let encoded = analysis::encode_semantic_tokens(&absolute);
    assert!(!encoded.is_empty());
    // First token has zero deltas.
    assert_eq!(encoded[0].delta_line, 0);
    assert_eq!(encoded[0].delta_start, 0);
    // A comment token is present (type index 3).
    assert!(encoded.iter().any(|t| t.token_type == 3));
    // Legend is contractual: 7 standard types plus the four custom
    // Active classes (secret, mode, contentAddressed, schema).
    assert_eq!(analysis::token_legend().token_types.len(), 11);
}

// ------------------------------------------------------------------
// Review regressions (C-043 Copilot review round 1).
// Each test pins one confirmed finding so it cannot regress.
// ------------------------------------------------------------------

#[test]
fn scope_resolution_prefers_innermost_sibling_over_let() {
    // `shared` exists as a let and as a `server` key: the use inside
    // `server` must resolve to the sibling, not the let.
    let doc = open_untitled(
        "scope",
        "let shared = \"outer\"\nserver {\n  shared = \"inner\"\n  url = shared\n}\n",
    );
    // `shared` use on line 3 (0-based), column of the value.
    let locations = analysis::definition_at(&doc, &no_lookup(), 3, 9);
    assert_eq!(locations.len(), 1, "{locations:?}");
    // Inner definition on line 2, not the let on line 0.
    assert_eq!(locations[0].range.start.line, 2, "{locations:?}");
}

#[test]
fn rename_refuses_nested_sibling_but_allows_outer_names() {
    let doc = open_untitled("nested", "[server]\nhost = \"a\"\nport = 1\n");
    // `port` is a sibling of `host` inside [server]: refused.
    assert!(
        analysis::rename_at(&doc, &no_lookup(), 1, 1, "port").is_err(),
        "nested sibling collision must be refused"
    );
    // A name taken only at another scope is fine.
    let doc = open_untitled("scopes", "port = 1\n[server]\nhost = \"a\"\n");
    assert!(
        analysis::rename_at(&doc, &no_lookup(), 2, 1, "port").is_ok(),
        "cross-scope rename must be allowed"
    );
}

#[test]
fn patch_value_strings_are_not_path_references() {
    // Only the first positional argument of @patch is a path: the
    // `"base"` value string must not count as a use of `base`.
    let doc = open_untitled(
        "patchval",
        "let base = \"x\"\nurl = \"u\"\n@patch(\"url\", \"base\")\n",
    );
    let refs = analysis::references_at(&doc, &no_lookup(), 0, 5, false);
    assert!(refs.is_empty(), "{refs:?}");
}

#[test]
fn patch_target_is_a_reference_of_the_patched_key() {
    let doc = open_untitled(
        "patchtgt",
        "let base = \"x\"\nurl = \"u\"\n@patch(\"url\", base)\n",
    );
    // Uses of `url` (declaration excluded): the patch target on line 2.
    let refs = analysis::references_at(&doc, &no_lookup(), 1, 1, false);
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].range.start.line, 2, "{refs:?}");
}

#[test]
fn depends_on_elements_are_references() {
    let doc = open_untitled(
        "dep",
        "[server]\ndepends_on = [\"db\"]\nport = 1\n[db]\nhost = \"h\"\n",
    );
    // `db` header on line 3: the depends_on element on line 1 is a use.
    let refs = analysis::references_at(&doc, &no_lookup(), 3, 1, false);
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].range.start.line, 1, "{refs:?}");
}

#[test]
fn hover_on_literal_value_resolves_owning_key() {
    let doc = open_untitled("lit", "port = 8080\n");
    // Cursor inside `8080` (not on an identifier): owner fallback.
    let hover = analysis::hover_at(&doc, 0, 8).expect("literal hover");
    let lsp_types::HoverContents::Markup(content) = hover.contents else {
        panic!("markup expected");
    };
    assert!(content.value.contains("**port**"), "{}", content.value);
    assert!(content.value.contains("8080"), "{}", content.value);
}

#[test]
fn unicode_positions_are_utf16() {
    // 🌍 is one character but two UTF-16 code units: the `suffix`
    // reference after it must be UTF-16 addressed.
    let text = "url = \"🌍\" + suffix\nsuffix = \"s\"\n";
    let line0 = text.lines().next().expect("line");
    let byte_idx = line0.find("suffix").expect("suffix");
    let expected_start = line0[..byte_idx].encode_utf16().count() as u32;
    assert_eq!(expected_start, 13, "fixture assumption: {line0:?}");
    let absolute = analysis::semantic_tokens_absolute(text);
    assert!(
        absolute.contains(&(0, expected_start, 6, 5)),
        "variable token must be UTF-16 addressed: {absolute:?}"
    );
    // Offset/position round-trips through the same units.
    let offset = analysis::offset_of(text, 0, expected_start);
    assert_eq!(offset, byte_idx, "offset must land on the token");
    let pos = analysis::position_of(text, offset);
    assert_eq!((pos.line, pos.character), (0, expected_start));
}

#[test]
fn semantic_token_categories_cover_statements() {
    let absolute =
        analysis::semantic_tokens_absolute("let limit = 5\n[server]\nport = 1\n# note\n");
    for expected in [
        (0, 0, 3, 0),  // `let` keyword
        (0, 4, 5, 5),  // binding name after `let` is a variable
        (0, 10, 1, 6), // `=`
        (0, 12, 1, 2), // number
        (1, 0, 1, 6),  // `[`
        (1, 1, 6, 4),  // table name after `[` is a property
        (1, 7, 1, 6),  // `]`
        (2, 0, 4, 4),  // key before `=` is a property
        (2, 7, 1, 2),  // number
        (3, 0, 6, 3),  // comment
    ] {
        assert!(
            absolute.contains(&expected),
            "missing token {expected:?} in {absolute:?}"
        );
    }
}

#[test]
fn missing_include_reports_e017_and_keeps_document() {
    // Includes resolve against the including file, never the server
    // working directory: a missing sibling is E017, not a panic and
    // not a silent CWD read.
    let (uri, _path) = temp_fixture(
        "lspmissing.active",
        "include \"does-not-exist-xyz.active\"\nport = 1\n",
    );
    let text = std::fs::read_to_string(uri.to_file_path().expect("file uri")).expect("read back");
    let doc = analysis::open_doc(uri, text, 1, None);
    assert!(
        doc.parse_diags.iter().any(|d| d.code == "E017"),
        "missing include must be E017: {:?}",
        doc.parse_diags
    );
    assert!(doc.module.root.is_some(), "document must survive the error");
    assert!(
        doc.module.resolve_value_path("port").is_some(),
        "keys after the bad include must still parse"
    );
}

// ------------------------------------------------------------------
// Review regressions round 2 (Copilot confirmation review).
// ------------------------------------------------------------------

#[test]
fn comments_and_string_contents_start_no_symbol() {
    let doc = open_untitled(
        "prose",
        "port = 1\n# rename port to something\nurl = \"port lives here\"\n",
    );
    // `port` inside the comment: no hover, no definition.
    assert!(analysis::hover_at(&doc, 1, 10).is_none());
    assert!(analysis::definition_at(&doc, &no_lookup(), 1, 10).is_empty());
    // `port` inside the string is not a symbol: definition falls back
    // to the owning key (hover does the same — it is a value
    // position).
    let definitions = analysis::definition_at(&doc, &no_lookup(), 2, 8);
    assert_eq!(definitions.len(), 1, "{definitions:?}");
    assert_eq!(definitions[0].range.start.line, 2, "{definitions:?}");
    let hover = analysis::hover_at(&doc, 2, 8).expect("owner hover");
    let lsp_types::HoverContents::Markup(content) = hover.contents else {
        panic!("markup expected");
    };
    assert!(content.value.contains("**url**"), "{}", content.value);
}

#[test]
fn definition_works_from_inside_interpolation() {
    let text = "let base = \"x\"\nurl = \"https://${base}/v1\"\n";
    let doc = open_untitled("interp", text);
    let line1 = text.lines().nth(1).expect("line");
    let byte_idx = line1.find("${base}").expect("interp").saturating_add(2);
    let character = line1[..byte_idx].encode_utf16().count() as u32;
    let locations = analysis::definition_at(&doc, &no_lookup(), 1, character);
    assert_eq!(locations.len(), 1, "{locations:?}");
    assert_eq!(locations[0].range.start.line, 0, "{locations:?}");
}

#[test]
fn rename_refuses_reserved_spellings() {
    let doc = open_untitled("reserved", "let limit = 5\nport = limit\n");
    for reserved in [
        "true", "false", "null", "let", "env", "secret", "include", "profile",
    ] {
        assert!(
            analysis::rename_at(&doc, &no_lookup(), 0, 5, reserved).is_err(),
            "{reserved} must be refused"
        );
    }
}

#[test]
fn same_named_keys_in_other_tables_are_not_references() {
    // The patch targets a.port: b.port must not match by leaf name.
    let doc = open_untitled(
        "isolation",
        "[a]\nport = 1\n[b]\nport = 2\n@patch(\"a.port\", 3)\n",
    );
    let refs_a = analysis::references_at(&doc, &no_lookup(), 1, 1, false);
    assert_eq!(refs_a.len(), 1, "{refs_a:?}");
    assert_eq!(refs_a[0].range.start.line, 4, "{refs_a:?}");
    let refs_b = analysis::references_at(&doc, &no_lookup(), 3, 1, false);
    assert!(refs_b.is_empty(), "{refs_b:?}");
    // Same isolation through depends_on elements.
    let doc = open_untitled(
        "depiso",
        "[a]\nport = 1\n[b]\nport = 2\n[c]\ndepends_on = [\"a.port\"]\nx = 1\n",
    );
    let refs_a = analysis::references_at(&doc, &no_lookup(), 1, 1, false);
    assert_eq!(refs_a.len(), 1, "{refs_a:?}");
    assert_eq!(refs_a[0].range.start.line, 5, "{refs_a:?}");
    let refs_b = analysis::references_at(&doc, &no_lookup(), 3, 1, false);
    assert!(refs_b.is_empty(), "{refs_b:?}");
}

#[test]
fn rename_destination_is_not_a_reference() {
    // Only the source path counts: renaming x.a must not touch the
    // "b" destination (which must be a plain name per the grammar).
    let doc = open_untitled(
        "renamedest",
        "[x]\na = 1\n[y]\na = 2\n@rename(\"x.a\", \"b\")\n",
    );
    let refs = analysis::references_at(&doc, &no_lookup(), 1, 0, false);
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].range.start.line, 4, "{refs:?}");
    // The other table's same-named key stays out of it.
    let refs = analysis::references_at(&doc, &no_lookup(), 3, 0, false);
    assert!(refs.is_empty(), "{refs:?}");
    let edit = analysis::rename_at(&doc, &no_lookup(), 1, 0, "z").expect("rename must succeed");
    let changes = edit.changes.expect("changes");
    let all: Vec<_> = changes.values().flatten().collect();
    assert_eq!(all.len(), 2, "declaration + source: {all:?}");
    assert!(all.iter().all(|e| e.new_text == "z"));
}

#[test]
fn same_line_duplicate_keys_resolve_to_the_right_declaration() {
    // Two single-line blocks share line 0: column-precise extents
    // keep each cursor in its own block, and the anchored search
    // points at the right `host`.
    let text = "server { host = \"a\" } client { host = \"b\" }\n";
    let doc = open_untitled("dup", text);
    let line0 = text.lines().next().expect("line");
    let first = line0.find("host").expect("first host");
    let second = line0.rfind("host").expect("second host");
    assert_ne!(first, second);
    let first_u16 = line0[..first].encode_utf16().count() as u32;
    let second_u16 = line0[..second].encode_utf16().count() as u32;
    let locations = analysis::definition_at(&doc, &no_lookup(), 0, second_u16);
    assert_eq!(locations.len(), 1, "{locations:?}");
    assert_eq!(locations[0].range.start.line, 0, "{locations:?}");
    assert_eq!(
        locations[0].range.start.character, second_u16,
        "must point at the second host: {locations:?}"
    );
    let locations = analysis::definition_at(&doc, &no_lookup(), 0, first_u16);
    assert_eq!(locations.len(), 1, "{locations:?}");
    assert_eq!(
        locations[0].range.start.character, first_u16,
        "must point at the first host: {locations:?}"
    );
}

#[test]
fn nested_same_line_tables_resolve_to_their_own_names() {
    // Table nodes span their name token: definition of a table name
    // must point at that name, not at an earlier duplicate.
    let text = "left { server { x = 1 } } right { server { x = 2 } }\n";
    let doc = open_untitled("nesteddup", text);
    let line0 = text.lines().next().expect("line");
    let first = line0.find("server").expect("first server");
    let second = line0.rfind("server").expect("second server");
    assert_ne!(first, second);
    let first_u16 = line0[..first].encode_utf16().count() as u32;
    let second_u16 = line0[..second].encode_utf16().count() as u32;
    for (cursor, expected) in [(first_u16, first_u16), (second_u16, second_u16)] {
        let locations = analysis::definition_at(&doc, &no_lookup(), 0, cursor);
        assert_eq!(locations.len(), 1, "{locations:?}");
        assert_eq!(
            locations[0].range.start.character, expected,
            "cursor {cursor}: {locations:?}"
        );
    }
}

#[test]
fn definition_on_literal_falls_back_to_owning_key() {
    let doc = open_untitled("litdef", "port = 8080\n");
    let locations = analysis::definition_at(&doc, &no_lookup(), 0, 8);
    assert_eq!(locations.len(), 1, "{locations:?}");
    assert_eq!(locations[0].range.start.line, 0, "{locations:?}");
    assert_eq!(
        (
            locations[0].range.start.character,
            locations[0].range.end.character
        ),
        (0, 4),
        "{locations:?}"
    );
}

#[test]
fn hover_falls_back_to_value_kind_without_schema() {
    let doc = open_untitled("kind", "port = 8080\nname = \"svc\"\n");
    let hover = analysis::hover_at(&doc, 0, 1).expect("hover");
    let lsp_types::HoverContents::Markup(content) = hover.contents else {
        panic!("markup expected");
    };
    assert!(
        content.value.contains("**port**: `Int`"),
        "{}",
        content.value
    );
    let hover = analysis::hover_at(&doc, 1, 1).expect("hover");
    let lsp_types::HoverContents::Markup(content) = hover.contents else {
        panic!("markup expected");
    };
    assert!(
        content.value.contains("**name**: `String`"),
        "{}",
        content.value
    );
}

// ------------------------------------------------------------------
// Cross-file includes (unit level, temp files on disk).
// ------------------------------------------------------------------

fn temp_fixture(name: &str, contents: &str) -> (lsp_types::Url, String) {
    let dir = std::env::temp_dir().join("active-lsp-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    let uri = lsp_types::Url::from_file_path(&path).expect("file uri");
    (uri, path.to_string_lossy().into_owned())
}

#[test]
fn definition_jumps_across_includes() {
    let (_lib_uri, _lib_path) = temp_fixture("lsplib2.active", "host = \"db\"\n");
    let (main_uri, _main_path) = temp_fixture(
        "lspmain2.active",
        "include \"lsplib2.active\"\nurl = host\n",
    );
    let text =
        std::fs::read_to_string(main_uri.to_file_path().expect("file uri")).expect("read back");
    let doc = analysis::open_doc(main_uri.clone(), text, 1, None);
    // The merged module indexes the included key with its file.
    let found = doc
        .module
        .value_paths()
        .iter()
        .any(|(id, path)| path == "host" && doc.module.node_files.contains_key(id));
    assert!(found, "included key must be indexed");
    // And definition from the use lands in the included file. URIs
    // compare by file name: include resolution canonicalizes paths
    // (case/short-name sensitive on Windows), so exact-URI equality
    // is environment-dependent.
    let lookup = |path: &str| std::fs::read_to_string(path).ok();
    let locations = analysis::definition_at(&doc, &lookup, 1, 7);
    assert_eq!(locations.len(), 1, "{locations:?}");
    assert!(
        locations[0].uri.path().ends_with("lsplib2.active"),
        "{locations:?}"
    );
    assert_eq!(locations[0].range.start.line, 0, "{locations:?}");
    // References with declaration: the lib definition plus the main use.
    let refs = analysis::references_at(&doc, &lookup, 1, 7, true);
    assert_eq!(refs.len(), 2, "{refs:?}");
    assert!(refs
        .iter()
        .any(|l| l.uri.path().ends_with("lsplib2.active")));
    assert!(refs
        .iter()
        .any(|l| l.uri.path().ends_with("lspmain2.active")));
}

// ------------------------------------------------------------------
// C-044: completions, diagnostics, inlay hints.
// ------------------------------------------------------------------

fn completion_labels(doc: &analysis::Doc, line: u32, character: u32) -> Vec<(String, String)> {
    analysis::completion_at(doc, line, character)
        .into_iter()
        .map(|item| (item.label, item.detail.unwrap_or_default()))
        .collect()
}

#[test]
fn completion_suggests_lets_keywords_and_schema_fields() {
    // Prefix-filtered lets.
    let doc = open_untitled("cmp", "let limit = 5\nport = li\n");
    let labels = completion_labels(&doc, 1, 9);
    assert!(
        labels.iter().any(|(label, _)| label == "limit"),
        "{labels:?}"
    );
    assert!(
        labels.iter().all(|(label, _)| label.starts_with("li")),
        "{labels:?}"
    );
    // Empty prefix completes everything, sorted.
    let labels = completion_labels(&doc, 1, 0);
    for keyword in ["true", "false", "null"] {
        assert!(
            labels.iter().any(|(label, _)| label == keyword),
            "{labels:?}"
        );
    }
    let names: Vec<&str> = labels.iter().map(|(label, _)| label.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "completion must be alphabetical");
    // Schema fields carry their type as detail.
    let doc = open_untitled("cmpschema", "schema = { port = Int(min = 1) }\nport = 1\n");
    let labels = completion_labels(&doc, 1, 0);
    assert!(
        labels
            .iter()
            .any(|(label, detail)| label == "port" && detail == "Int"),
        "{labels:?}"
    );
    // Lets complete inside table scopes.
    let doc = open_untitled("cmpscope", "let limit = 5\n[server]\nurl = lim\n");
    let labels = completion_labels(&doc, 2, 9);
    assert!(
        labels.iter().any(|(label, _)| label == "limit"),
        "{labels:?}"
    );
}

#[test]
fn completion_stays_in_open_and_empty_tables() {
    // Empty table: the cursor past the header still completes the
    // table's schema fields, not the root's.
    let doc = open_untitled(
        "cmpempty",
        "schema = { server = Table { host = String, port = Int } }\n[server]\n",
    );
    let labels = completion_labels(&doc, 2, 0);
    assert!(
        labels.iter().any(|(label, _)| label == "host"),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|(label, _)| label == "port"),
        "{labels:?}"
    );
    // Fresh line after the last field: same table.
    let doc = open_untitled(
        "cmpafter",
        "schema = { server = Table { host = String, port = Int } }\n[server]\nhost = \"a\"\npo\n",
    );
    let labels = completion_labels(&doc, 3, 2);
    assert!(
        labels.iter().any(|(label, _)| label == "port"),
        "{labels:?}"
    );
    // Past a brace block's close: back at the root (the root schema
    // offers `server`; the table schema's `host` must not appear).
    let doc = open_untitled(
        "cmpclosed",
        "schema = { server = Table { host = String } }\nserver {\n  host = \"a\"\n}\n",
    );
    let labels = completion_labels(&doc, 4, 0);
    assert!(
        labels.iter().any(|(label, _)| label == "server"),
        "{labels:?}"
    );
    assert!(
        !labels.iter().any(|(label, _)| label == "host"),
        "{labels:?}"
    );
}

#[test]
fn inline_table_scopes_end_at_their_close() {
    // `cfg` is an inline table on line 0: its scope must not extend
    // to end-of-file, or the `port` use below would resolve to the
    // inline key instead of the root sibling.
    let doc = open_untitled("inlinescope", "cfg = { port = 1 }\nport = 2\nurl = port\n");
    let locations = analysis::definition_at(&doc, &no_lookup(), 2, 7);
    assert_eq!(locations.len(), 1, "{locations:?}");
    assert_eq!(locations[0].range.start.line, 1, "{locations:?}");
    assert_eq!(locations[0].range.start.character, 0, "{locations:?}");
}

#[test]
fn unclosed_brace_tables_keep_their_trailing_lines() {
    // No matching `}`: the header-style fallback keeps fresh lines
    // in the table instead of dropping to the root.
    let doc = open_untitled(
        "unclosed",
        "schema = { server = Table { host = String } }\nserver {\n  host = \"a\"\n\n",
    );
    let labels = completion_labels(&doc, 3, 0);
    assert!(
        labels.iter().any(|(label, _)| label == "host"),
        "{labels:?}"
    );
}

fn hint_labels(doc: &analysis::Doc) -> Vec<(u32, u32, String)> {
    analysis::inlay_hints(doc, None)
        .into_iter()
        .map(|hint| {
            let label = match hint.label {
                lsp_types::InlayHintLabel::String(s) => s,
                lsp_types::InlayHintLabel::LabelParts(parts) => {
                    parts.into_iter().map(|part| part.value).collect()
                }
            };
            (hint.position.line, hint.position.character, label)
        })
        .collect()
}

#[test]
fn inlay_hints_show_types_and_normalized_values() {
    let doc = open_untitled("hints", "port = 8080\nwait = 5s\nlet limit = 5\n");
    let hints = hint_labels(&doc);
    assert!(
        hints
            .iter()
            .any(|(line, _, label)| *line == 0 && label == ": Int"),
        "{hints:?}"
    );
    assert!(
        hints
            .iter()
            .any(|(line, _, label)| *line == 1 && label == ": Duration = 5000 ms"),
        "{hints:?}"
    );
    assert!(
        hints
            .iter()
            .any(|(line, _, label)| *line == 2 && label == ": Int"),
        "{hints:?}"
    );
    // Byte quantities evaluate to integers: the hint recovers the
    // source unit for the normalized form.
    let doc = open_untitled("hintbytes", "size = 5MiB\n");
    let hints = hint_labels(&doc);
    assert!(
        hints
            .iter()
            .any(|(_, _, label)| label == ": Int = 5242880 MiB"),
        "{hints:?}"
    );
    // Range filtering withholds outside hints.
    let range = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 0,
            character: 20,
        },
    };
    let hints = analysis::inlay_hints(&doc, Some(range));
    assert!(!hints.is_empty());
    assert!(hints.iter().all(|hint| hint.position.line == 0));
}

#[test]
fn inlay_hints_redact_secrets() {
    let doc = open_untitled("hintsecret", "token = secret(\"s3cr3t-hint\")\n");
    let hints = hint_labels(&doc);
    assert!(
        hints.iter().any(|(_, _, label)| label == ": Secret"),
        "{hints:?}"
    );
    assert!(
        hints
            .iter()
            .all(|(_, _, label)| !label.contains("s3cr3t-hint")),
        "{hints:?}"
    );
}

#[test]
fn diagnostics_cover_parse_eval_and_lint_with_stable_codes() {
    // Parse errors surface with E-codes.
    let doc = open_untitled("diagparse", "[server\nport = 1\n");
    let diags = analysis::diagnostics_for(&doc);
    assert!(!diags.is_empty(), "parse errors must publish");
    assert!(diags.iter().any(|d| match &d.code {
        Some(lsp_types::NumberOrString::String(code)) => code.starts_with('E'),
        _ => false,
    }));
    // Hard evaluation failures (cycles) convert to S-diagnostics.
    let doc = open_untitled("diagcycle", "[a]\nx = y\ny = x\n");
    let diags = analysis::diagnostics_for(&doc);
    let codes: Vec<String> = diags
        .iter()
        .filter_map(|d| match &d.code {
            Some(lsp_types::NumberOrString::String(code)) => Some(code.clone()),
            _ => None,
        })
        .collect();
    assert!(codes.iter().any(|c| c == "S001"), "{codes:?}");
    assert!(
        diags
            .iter()
            .any(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)),
        "{diags:?}"
    );
    // Lint findings ride along.
    let doc = open_untitled("diaglint", "let unused = 1\nport = 2\n");
    let diags = analysis::diagnostics_for(&doc);
    assert!(
        diags.iter().any(|d| match &d.code {
            Some(lsp_types::NumberOrString::String(code)) => code.starts_with('L'),
            _ => false,
        }),
        "{diags:?}"
    );
    // Deterministic order: sorted by position.
    let positions: Vec<(u32, u32)> = diags
        .iter()
        .map(|d| (d.range.start.line, d.range.start.character))
        .collect();
    let mut sorted = positions.clone();
    sorted.sort();
    assert_eq!(positions, sorted);
    // Every diagnostic carries the source tag.
    assert!(diags.iter().all(|d| d.source.as_deref() == Some("active")));
}

#[test]
fn diagnostics_never_contain_secrets() {
    let doc = open_untitled(
        "diagsecret",
        "token = secret(\"s3cr3t-diag\")\nurl = \"${token}\"\n",
    );
    let diags = analysis::diagnostics_for(&doc);
    assert!(!diags.is_empty(), "secret misuse must surface: {diags:?}");
    for diag in &diags {
        assert!(!diag.message.contains("s3cr3t-diag"), "{diag:?}");
    }
}

#[test]
fn healing_notices_publish_exactly_once() {
    // An unclosed string heals (auto-apply tier) and records one
    // H-notice: evaluation merges the healing records, so the
    // publisher must not append them again.
    let doc = open_untitled("diagheal", "port = \"abc\n");
    let diags = analysis::diagnostics_for(&doc);
    let healing: Vec<_> = diags
        .iter()
        .filter(|d| match &d.code {
            Some(lsp_types::NumberOrString::String(code)) => code.starts_with('H'),
            _ => false,
        })
        .collect();
    assert_eq!(healing.len(), 1, "{diags:?}");
}

#[test]
fn include_diagnostics_publish_under_their_own_file() {
    // The library has a syntax error on its own line 0: it must
    // publish under the library URI with library coordinates, never
    // mis-ranged against the main document.
    let (_lib_uri, _lib_path) = temp_fixture("lspdiaglib.active", "[broken\n");
    let (main_uri, _main_path) = temp_fixture(
        "lspdiagmain.active",
        "include \"lspdiaglib.active\"\nport = 1\n",
    );
    let text =
        std::fs::read_to_string(main_uri.to_file_path().expect("file uri")).expect("read back");
    let doc = analysis::open_doc(main_uri.clone(), text, 1, None);
    let lookup = |path: &str| std::fs::read_to_string(path).ok();
    let groups = analysis::diagnostics_by_file(&doc, &lookup);
    assert!(groups.len() >= 2, "{groups:?}");
    let lib_group = groups
        .iter()
        .find(|(uri, _)| uri.path().ends_with("lspdiaglib.active"))
        .expect("library group: {groups:?}");
    assert!(
        lib_group.1.iter().any(|d| match &d.code {
            Some(lsp_types::NumberOrString::String(code)) => code.starts_with('E'),
            _ => false,
        }),
        "{lib_group:?}"
    );
    assert!(
        lib_group.1.iter().all(|d| d.range.start.line == 0),
        "library coordinates: {lib_group:?}"
    );
    // The main document's own group survives alongside.
    assert!(
        groups
            .iter()
            .any(|(uri, _)| uri.path().ends_with("lspdiagmain.active")),
        "{groups:?}"
    );
}

#[test]
fn buffer_matching_tolerates_uri_case_differences() {
    use std::collections::HashMap;
    let (uri, path) = temp_fixture("lspcase.active", "port = 1\n");
    let doc = analysis::open_doc(uri.clone(), "port = 1\n".to_string(), 7, None);
    let mut open = HashMap::new();
    open.insert(uri.to_string(), doc);
    assert_eq!(
        analysis::lookup_text(&open, &path),
        Some("port = 1\n".to_string())
    );
    // Canonical stamps (Windows case/short-names) may differ
    // textually from the client's URI: the live buffer still wins
    // over disk, for both text and version.
    let shifted = path.to_uppercase();
    if shifted != path {
        assert_eq!(
            analysis::lookup_text(&open, &shifted),
            Some("port = 1\n".to_string())
        );
        let uri_shifted = lsp_types::Url::from_file_path(&shifted).expect("uri");
        assert_eq!(analysis::open_version(&open, &uri_shifted), Some(7));
    }
    // Unknown files have no version (never another file's).
    let unknown = lsp_types::Url::parse("untitled:nowhere").expect("uri");
    assert_eq!(analysis::open_version(&open, &unknown), None);
}

// ------------------------------------------------------------------
// C-045: code actions.
// ------------------------------------------------------------------

/// Apply text edits (range → replacement) back-to-front so offsets
/// stay valid, for round-trip assertions on action edits.
fn apply_edits(text: &str, edits: &[(lsp_types::Range, String)]) -> String {
    let mut ordered: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|(range, new_text)| {
            (
                analysis::offset_of(text, range.start.line, range.start.character),
                analysis::offset_of(text, range.end.line, range.end.character),
                new_text.as_str(),
            )
        })
        .collect();
    ordered.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut out = text.to_string();
    for (start, end, new_text) in ordered {
        out.replace_range(start..end, new_text);
    }
    out
}

#[test]
fn healing_action_applies_the_suggested_repair() {
    let doc = open_untitled("healact", "port = \"abc\n");
    let everything = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 2,
            character: 0,
        },
    };
    let actions = analysis::code_actions(&doc, everything);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Apply healing (H"))
        .expect("healing action: {actions:?}");
    let edits: Vec<(lsp_types::Range, String)> = action
        .edit
        .as_ref()
        .expect("edit")
        .changes
        .as_ref()
        .expect("changes")
        .values()
        .flatten()
        .map(|edit| (edit.range, edit.new_text.clone()))
        .collect();
    assert_eq!(edits.len(), 1);
    // Round trip: the repaired document heals clean.
    let fixed = apply_edits(&doc.text, &edits);
    let reopened = open_untitled("healact2", &fixed);
    let diags = analysis::diagnostics_for(&reopened);
    assert!(
        diags.iter().all(|d| match &d.code {
            Some(lsp_types::NumberOrString::String(code)) => !code.starts_with('H'),
            _ => true,
        }),
        "{diags:?}"
    );
}

#[test]
fn extract_action_inserts_binding_and_rewrites_use() {
    let doc = open_untitled("extract", "port = 8080\n");
    let select = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 7,
        },
        end: lsp_types::Position {
            line: 0,
            character: 11,
        },
    };
    let actions = analysis::code_actions(&doc, select);
    let action = actions
        .iter()
        .find(|action| action.title == "Extract to let `extracted`")
        .expect("extract action: {actions:?}");
    let edits: Vec<(lsp_types::Range, String)> = action
        .edit
        .as_ref()
        .expect("edit")
        .changes
        .as_ref()
        .expect("changes")
        .values()
        .flatten()
        .map(|edit| (edit.range, edit.new_text.clone()))
        .collect();
    assert_eq!(edits.len(), 2);
    let rewritten = apply_edits(&doc.text, &edits);
    assert_eq!(
        rewritten, "let extracted = 8080\nport = extracted\n",
        "{rewritten:?}"
    );
    // The rewritten document evaluates to the same value.
    let reopened = open_untitled("extract2", &rewritten);
    let hover = analysis::hover_at(&reopened, 1, 1).expect("hover");
    let lsp_types::HoverContents::Markup(content) = hover.contents else {
        panic!("markup expected");
    };
    assert!(content.value.contains("8080"), "{}", content.value);
}

#[test]
fn convert_actions_offer_only_changing_directions() {
    let doc = open_untitled("conv", "[server]\nport = 1\n");
    let everything = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 3,
            character: 0,
        },
    };
    let actions = analysis::code_actions(&doc, everything);
    let titles: Vec<&str> = actions.iter().map(|action| action.title.as_str()).collect();
    assert!(titles.contains(&"Convert to indent style"), "{titles:?}");
    assert!(
        !titles.iter().any(|title| title.contains("brace")),
        "{titles:?}"
    );
    let action = actions
        .iter()
        .find(|action| action.title == "Convert to indent style")
        .expect("convert action");
    let edit = action
        .edit
        .as_ref()
        .expect("edit")
        .changes
        .as_ref()
        .expect("changes")
        .values()
        .flatten()
        .next()
        .expect("one edit");
    assert!(edit.new_text.contains("server:"), "{}", edit.new_text);
}

#[test]
fn depends_on_action_inserts_the_missing_path() {
    // Empty declared array: the missing path fills it (missing-entry
    // diagnostics fire for declared arrays — empty included).
    let doc = open_untitled(
        "depact",
        "[server]\ndepends_on = []\nport = max\nlet max = 4\n",
    );
    let diags = analysis::diagnostics_for(&doc);
    let missing = diags
        .iter()
        .find(|d| match &d.code {
            Some(lsp_types::NumberOrString::String(code)) => {
                code == "L002" && d.message.contains("missing")
            }
            _ => false,
        })
        .expect("L002 missing: {diags:?}");
    let actions = analysis::code_actions(&doc, missing.range);
    let action = actions
        .iter()
        .find(|action| action.title == "Add depends_on entry for 'max'")
        .expect("depends_on action: {actions:?}");
    let edits: Vec<(lsp_types::Range, String)> = action
        .edit
        .as_ref()
        .expect("edit")
        .changes
        .as_ref()
        .expect("changes")
        .values()
        .flatten()
        .map(|edit| (edit.range, edit.new_text.clone()))
        .collect();
    assert_eq!(edits.len(), 1);
    let fixed = apply_edits(&doc.text, &edits);
    assert!(fixed.contains("depends_on = [\"max\"]"), "{fixed:?}");
    let reopened = open_untitled("depact2", &fixed);
    let diags = analysis::diagnostics_for(&reopened);
    assert!(
        diags
            .iter()
            .all(|d| !d.message.contains("missing depends_on")),
        "{diags:?}"
    );
}

#[test]
fn depends_on_action_creates_a_missing_array() {
    // The creation branch is defensive (missing-entry diagnostics
    // fire for declared arrays), exercised with a synthetic
    // diagnostic on a scope without one.
    let mut doc = open_untitled("depcreate", "[server]\nport = 1\n");
    let table = doc.module.resolve_value_path("server").expect("server");
    let span = doc.module.node_spans.get(&table).cloned().expect("span");
    doc.lint_diags.push(active_core::Diagnostic {
        severity: active_core::Severity::Warning,
        code: "L002".to_string(),
        message: "missing depends_on entry for 'db'".to_string(),
        path: Some("server".to_string()),
        span: Some(span),
        repair: None,
        confidence: 1.0,
        file: None,
    });
    let diags = analysis::diagnostics_for(&doc);
    let missing = diags
        .iter()
        .find(|d| d.message.contains("missing depends_on"))
        .expect("synthetic L002: {diags:?}");
    let actions = analysis::code_actions(&doc, missing.range);
    let action = actions
        .iter()
        .find(|action| action.title == "Add depends_on entry for 'db'")
        .expect("creation action: {actions:?}");
    let edits: Vec<(lsp_types::Range, String)> = action
        .edit
        .as_ref()
        .expect("edit")
        .changes
        .as_ref()
        .expect("changes")
        .values()
        .flatten()
        .map(|edit| (edit.range, edit.new_text.clone()))
        .collect();
    assert_eq!(edits.len(), 1);
    let fixed = apply_edits(&doc.text, &edits);
    assert!(fixed.contains("depends_on = [\"db\"]"), "{fixed:?}");
}

#[test]
fn depends_on_action_prefers_repair_metadata_over_prose() {
    // The machine repair identifies the diagnostic even when the
    // human message changes beyond recognition.
    let mut doc = open_untitled("deprepair", "[server]\ndepends_on = []\nport = 1\n");
    let table = doc.module.resolve_value_path("server").expect("server");
    let span = doc.module.node_spans.get(&table).cloned().expect("span");
    doc.lint_diags.push(active_core::Diagnostic {
        severity: active_core::Severity::Warning,
        code: "L002".to_string(),
        message: "something entirely different".to_string(),
        path: Some("server".to_string()),
        span: Some(span),
        repair: Some(active_core::Repair {
            original: String::new(),
            suggested: "db".to_string(),
            kind: active_core::RepairKind::Other("add_depends_on".to_string()),
            safe: true,
            side_effects: Vec::new(),
            goal: None,
            steps: Vec::new(),
        }),
        confidence: 1.0,
        file: None,
    });
    let everything = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 3,
            character: 0,
        },
    };
    let actions = analysis::code_actions(&doc, everything);
    assert!(
        actions
            .iter()
            .any(|action| action.title == "Add depends_on entry for 'db'"),
        "{actions:?}"
    );
}

#[test]
fn quiet_positions_offer_no_targeted_actions() {
    let doc = open_untitled("quiet", "port = 1\n");
    let point = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 0,
            character: 0,
        },
    };
    let actions = analysis::code_actions(&doc, point);
    assert!(
        actions.iter().all(|action| {
            !action.title.starts_with("Apply healing")
                && !action.title.starts_with("Extract")
                && !action.title.starts_with("Add depends_on")
        }),
        "{actions:?}"
    );
}

#[test]
fn unsafe_high_confidence_repairs_are_withheld() {
    // Confidence alone never presents: an explicitly unsafe repair
    // must not offer, however it scored.
    let mut doc = open_untitled("unsafeheal", "port = 1\n");
    doc.eval_diags.push(active_core::Diagnostic {
        severity: active_core::Severity::Warning,
        code: "H005".to_string(),
        message: "unknown key 'prt', did you mean 'port'?".to_string(),
        path: None,
        span: None,
        repair: Some(active_core::Repair {
            original: "prt".to_string(),
            suggested: "port".to_string(),
            kind: active_core::RepairKind::KeyTypo,
            safe: false,
            side_effects: Vec::new(),
            goal: None,
            steps: Vec::new(),
        }),
        confidence: 0.97,
        file: None,
    });
    let everything = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 2,
            character: 0,
        },
    };
    let actions = analysis::code_actions(&doc, everything);
    assert!(
        actions
            .iter()
            .all(|action| !action.title.starts_with("Apply healing")),
        "{actions:?}"
    );
    // Control: the same repair marked safe is offered.
    doc.eval_diags[0].repair.as_mut().expect("repair").safe = true;
    let actions = analysis::code_actions(&doc, everything);
    assert!(
        actions
            .iter()
            .any(|action| action.title == "Apply healing (H005)"),
        "{actions:?}"
    );
}

#[test]
fn extract_withholds_non_expression_selections() {
    let doc = open_untitled("extractbad", "port = 8080\nurl = \"x\"\n");
    // A multi-statement selection has no single value container.
    let multi = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 1,
            character: 4,
        },
    };
    let actions = analysis::code_actions(&doc, multi);
    assert!(
        actions
            .iter()
            .all(|action| !action.title.starts_with("Extract")),
        "{actions:?}"
    );
    // A partial token (middle of `8080`) matches nothing exactly.
    let partial = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 8,
        },
        end: lsp_types::Position {
            line: 0,
            character: 10,
        },
    };
    let actions = analysis::code_actions(&doc, partial);
    assert!(
        actions
            .iter()
            .all(|action| !action.title.starts_with("Extract")),
        "{actions:?}"
    );
}

#[test]
fn extract_withholds_scope_relative_references() {
    // `host` resolves inside [server]: hoisting it to a top-level
    // let would rebind the reference.
    let doc = open_untitled("extractscope", "[server]\nhost = \"a\"\nurl = host\n");
    let select = lsp_types::Range {
        start: lsp_types::Position {
            line: 2,
            character: 6,
        },
        end: lsp_types::Position {
            line: 2,
            character: 10,
        },
    };
    let actions = analysis::code_actions(&doc, select);
    assert!(
        actions
            .iter()
            .all(|action| !action.title.starts_with("Extract")),
        "{actions:?}"
    );
    // Control: a root-key reference is document-safe.
    let doc = open_untitled("extractroot", "port = 8080\nurl = port\n");
    let select = lsp_types::Range {
        start: lsp_types::Position {
            line: 1,
            character: 6,
        },
        end: lsp_types::Position {
            line: 1,
            character: 10,
        },
    };
    let actions = analysis::code_actions(&doc, select);
    assert!(
        actions
            .iter()
            .any(|action| action.title.starts_with("Extract to let")),
        "{actions:?}"
    );
}

#[test]
fn extract_handles_non_bmp_characters() {
    // 🌍 is one character but two UTF-16 units and four bytes:
    // character-aware span mapping must still match the whole
    // string exactly.
    let text = "greeting = \"héllo 🌍\"\n";
    let doc = open_untitled("extractemoji", text);
    let line = text.lines().next().expect("line");
    let start_byte = line.find('"').expect("open quote");
    let end_byte = line.rfind('"').expect("close quote") + 1;
    let start = analysis::position_of(text, start_byte);
    let end = analysis::position_of(text, end_byte);
    let select = lsp_types::Range { start, end };
    let actions = analysis::code_actions(&doc, select);
    assert!(
        actions
            .iter()
            .any(|action| action.title.starts_with("Extract to let")),
        "{actions:?}"
    );
}

#[test]
fn extract_inserts_after_top_level_lets_only() {
    // The nested `let` sits past the top-level one: insertion still
    // anchors after the column-1 binding.
    let doc = open_untitled(
        "extractnested",
        "let a = 1\nserver {\n  let b = 2\n}\nport = a\n",
    );
    let select = lsp_types::Range {
        start: lsp_types::Position {
            line: 4,
            character: 7,
        },
        end: lsp_types::Position {
            line: 4,
            character: 8,
        },
    };
    let actions = analysis::code_actions(&doc, select);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Extract to let"))
        .expect("extract action: {actions:?}");
    let insert = action
        .edit
        .as_ref()
        .expect("edit")
        .changes
        .as_ref()
        .expect("changes")
        .values()
        .flatten()
        .find(|edit| edit.range.start == edit.range.end)
        .expect("insert edit");
    // Insert lands at the end of line 0 (after `let a = 1`), not
    // after the nested binding on line 2.
    assert_eq!(insert.range.start.line, 0, "{insert:?}");
}

#[test]
fn convert_range_excludes_crlf_terminators() {
    // No trailing newline: the full-file range must end at the
    // content width, not past a carriage return.
    let doc = open_untitled("crlf", "[server]\r\nport = 1\r");
    let everything = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 2,
            character: 0,
        },
    };
    let actions = analysis::code_actions(&doc, everything);
    let action = actions
        .iter()
        .find(|action| action.title == "Convert to indent style")
        .expect("convert action: {actions:?}");
    let edit = action
        .edit
        .as_ref()
        .expect("edit")
        .changes
        .as_ref()
        .expect("changes")
        .values()
        .flatten()
        .next()
        .expect("one edit");
    assert_eq!(
        (edit.range.end.line, edit.range.end.character),
        (1, 8),
        "{edit:?}"
    );
}

// ------------------------------------------------------------------
// C-049: dependency-graph queries.
// ------------------------------------------------------------------

const GRAPH_DOC: &str = "let base = 1\n[a]\nx = base\ny = x\n";

#[test]
fn impact_and_why_handlers_answer_from_the_cursor() {
    let doc = open_untitled("graph", GRAPH_DOC);
    // `base` use on line 2: impact lists its consumers.
    let (key, paths) = analysis::impact_at(&doc, 2, 6).expect("impact");
    assert_eq!(key, "base");
    assert!(paths.contains(&"a.x".to_string()), "{paths:?}");
    assert!(paths.contains(&"a.y".to_string()), "{paths:?}");
    // `y` definition on line 3: why lists its ingredients.
    let (key, paths) = analysis::why_at(&doc, 3, 0).expect("why");
    assert_eq!(key, "a.y");
    assert!(paths.contains(&"a.x".to_string()), "{paths:?}");
    assert!(paths.contains(&"base".to_string()), "{paths:?}");
    // Comments initiate nothing.
    let doc = open_untitled("graphquiet", "# base\nport = 1\n");
    assert!(analysis::impact_at(&doc, 0, 3).is_none());
    assert!(analysis::why_at(&doc, 0, 3).is_none());
}

#[test]
fn graph_snapshot_covers_all_formats() {
    let doc = open_untitled("graphshot", GRAPH_DOC);
    let (mermaid, dot, json) = analysis::graph_snapshot(&doc);
    assert!(mermaid.starts_with("flowchart TD"), "{mermaid}");
    assert!(dot.starts_with("digraph active {"), "{dot}");
    assert!(
        json.get("nodes").is_some() && json.get("edges").is_some(),
        "{json}"
    );
}

// ------------------------------------------------------------------
// End-to-end: real binary over stdio.
// ------------------------------------------------------------------

struct Framed {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    stdin: std::process::ChildStdin,
    next_id: i64,
}

impl Framed {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_active-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn active-lsp");
        let reader = BufReader::new(child.stdout.take().expect("stdout"));
        let stdin = child.stdin.take().expect("stdin");
        Self {
            child,
            reader,
            stdin,
            next_id: 1,
        }
    }

    fn send(&mut self, body: &str) {
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).expect("write");
        self.stdin.flush().expect("flush");
    }

    fn recv(&mut self) -> serde_json::Value {
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("header line");
            let line = line.trim().to_string();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                content_length = value.trim().parse().expect("length");
            }
        }
        let mut buf = vec![0u8; content_length];
        self.reader.read_exact(&mut buf).expect("body");
        serde_json::from_slice(&buf).expect("json")
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            })
            .to_string(),
        );
        loop {
            let message = self.recv();
            if message.get("id") == Some(&serde_json::json!(id)) {
                return message;
            }
        }
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) {
        self.send(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            })
            .to_string(),
        );
    }

    fn shutdown(&mut self) {
        let id = self.next_id;
        self.next_id += 1;
        self.send(
            &serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "shutdown"}).to_string(),
        );
        let _ = self.recv();
        self.notify("exit", serde_json::json!(null));
        let _ = self.child.wait();
    }

    /// Next server-initiated notification with the given method,
    /// dropping anything else (responses are read by `request`).
    fn recv_notification(&mut self, method: &str) -> serde_json::Value {
        loop {
            let message = self.recv();
            if message.get("method") == Some(&serde_json::json!(method)) {
                return message;
            }
        }
    }
}

#[test]
fn stdio_end_to_end_initialize_hover_definition_shutdown() {
    let (_uri, path) = temp_fixture("lspe2e.active", "# @intent: \"E2E port\"\nport = 8080\n");
    let file_uri = lsp_types::Url::from_file_path(&path).expect("file uri");
    let uri = file_uri.to_string();

    let mut server = Framed::spawn();
    let init = server.request(
        "initialize",
        serde_json::json!({
            "processId": null,
            "capabilities": {},
        }),
    );
    let capabilities = &init["result"]["capabilities"];
    assert_eq!(
        capabilities["hoverProvider"], true,
        "hover advertised: {init}"
    );
    assert!(capabilities.get("definitionProvider").is_some());
    server.notify("initialized", serde_json::json!({}));
    server.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": "active",
                "version": 1,
                "text": "# @intent: \"E2E port\"\nport = 8080\n",
            }
        }),
    );
    // Hover over `port` on line 1: must carry the intent text.
    let hover = server.request(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 1, "character": 1},
        }),
    );
    let contents = hover["result"]["contents"]["value"]
        .as_str()
        .expect("hover contents");
    assert!(contents.contains("E2E port"), "{hover}");
    assert!(!contents.contains("s3cr3t"), "{hover}");
    // Definition of `port` resolves to line 1.
    let definition = server.request(
        "textDocument/definition",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 1, "character": 1},
        }),
    );
    let locations = definition["result"].as_array().expect("locations");
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0]["range"]["start"]["line"], 1);
    server.shutdown();
}

#[test]
fn stdio_unknown_method_returns_method_not_found() {
    let mut server = Framed::spawn();
    let init = server.request(
        "initialize",
        serde_json::json!({
            "processId": null,
            "capabilities": {},
        }),
    );
    assert!(init.get("result").is_some(), "{init}");
    // The LSP handshake requires `initialized` before further
    // requests; anything else is a protocol error and kills the
    // server (verified against lsp-server's initialize()).
    server.notify("initialized", serde_json::json!({}));
    // Unknown requests are refused, never hung.
    let reply = server.request("workspace/unknownMethod", serde_json::json!({}));
    assert_eq!(reply["error"]["code"], -32601, "{reply}");
    server.shutdown();
}

#[test]
fn stdio_capability_explain_and_debug_bridges() {
    let uri = "untitled:bridges";
    let mut server = Framed::spawn();
    let init = server.request(
        "initialize",
        serde_json::json!({
            "processId": null,
            "capabilities": {},
        }),
    );
    assert!(init.get("result").is_some(), "{init}");
    server.notify("initialized", serde_json::json!({}));

    // Capability handshake: the same descriptor the CLI serves, and
    // an unknown major fails closed instead of answering.
    let descriptor = server.request("active/capability", serde_json::json!({}));
    let tools = descriptor["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert_eq!(tools.len(), 20, "{descriptor}");
    assert!(tools.iter().any(|t| t["name"] == "debug"), "{descriptor}");
    let refused = server.request("active/capability", serde_json::json!({"requireMajor": 99}));
    assert_eq!(refused["error"]["code"], -32602, "{refused}");

    server.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": "active",
                "version": 1,
                "text": "let idle = 1\nport = 8080\n",
            }
        }),
    );

    // Explainer bridge over every live diagnostic.
    let explain = server.request(
        "active/explain",
        serde_json::json!({"textDocument": {"uri": uri}}),
    );
    let items = explain["result"].as_array().expect("explanation array");
    assert!(
        items.iter().any(|e| e["code"] == "L001"),
        "lint finding must be explained: {explain}"
    );
    assert!(
        items
            .iter()
            .all(|e| e["guidance"].as_str().is_some_and(|g| !g.is_empty())),
        "{explain}"
    );

    // Debugger bridge: the same breakpoint selectors as the CLI.
    let steps = server.request(
        "active/debug/steps",
        serde_json::json!({"textDocument": {"uri": uri}, "breakpoint": "path:port"}),
    );
    assert_eq!(steps["result"]["breakpoint"], "path:port", "{steps}");
    assert!(steps["result"]["matched"].is_number(), "{steps}");
    assert!(
        !steps["result"]["steps"]
            .as_array()
            .expect("steps")
            .is_empty(),
        "{steps}"
    );

    // Unknown request methods still refuse.
    let reply = server.request("workspace/unknownMethod", serde_json::json!({}));
    assert_eq!(reply["error"]["code"], -32601, "{reply}");
    server.shutdown();
}

#[test]
fn stdio_publish_completion_hints_and_close() {
    let (_uri, path) = temp_fixture("lspdiag.active", "let limit = 5\n[a]\nx = y\ny = x\n");
    let file_uri = lsp_types::Url::from_file_path(&path).expect("file uri");
    let uri = file_uri.to_string();
    let text = "let limit = 5\n[a]\nx = y\ny = x\n";

    let mut server = Framed::spawn();
    let init = server.request(
        "initialize",
        serde_json::json!({
            "processId": null,
            "capabilities": {},
        }),
    );
    assert!(init.get("result").is_some(), "{init}");
    let capabilities = &init["result"]["capabilities"];
    assert!(capabilities.get("completionProvider").is_some(), "{init}");
    assert!(capabilities.get("inlayHintProvider").is_some(), "{init}");
    server.notify("initialized", serde_json::json!({}));
    server.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": "active",
                "version": 1,
                "text": text,
            }
        }),
    );
    // The cycle publishes as S001 before any request runs.
    let published = server.recv_notification("textDocument/publishDiagnostics");
    let diags = published["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(diags.iter().any(|d| d["code"] == "S001"), "{published}");
    // Completion over the wire: `lim` completes the let.
    let completion = server.request(
        "textDocument/completion",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": 11},
        }),
    );
    let items = completion["result"].as_array().expect("items");
    assert!(
        items.iter().any(|item| item["label"] == "limit"),
        "{completion}"
    );
    // Inlay hints over the wire: types with normalized durations.
    let hints = server.request(
        "textDocument/inlayHint",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 4, "character": 0},
            },
        }),
    );
    let hints = hints["result"].as_array().expect("hints");
    assert!(!hints.is_empty(), "hints expected");
    // didChange re-publishes: the fixed document carries a version
    // and no longer reports the cycle.
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"text": "let limit = 5\nport = limit\n"}],
        }),
    );
    let changed = server.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(changed["params"]["version"], 2, "{changed}");
    assert!(
        !changed["params"]["diagnostics"]
            .as_array()
            .expect("array")
            .iter()
            .any(|d| d["code"] == "S001"),
        "{changed}"
    );
    // didClose clears with an empty publish and no version (stale
    // clears with version 0 may be discarded by clients).
    server.notify(
        "textDocument/didClose",
        serde_json::json!({"textDocument": {"uri": uri}}),
    );
    let cleared = server.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(
        cleared["params"]["diagnostics"],
        serde_json::json!([]),
        "{cleared}"
    );
    assert!(
        cleared["params"].get("version").is_none_or(|v| v.is_null()),
        "close clear must be versionless: {cleared}"
    );
    server.shutdown();
}

#[test]
fn stdio_shared_include_markers_survive_single_close() {
    let (_lib_uri, lib_path) = temp_fixture("lspmultilib.active", "[broken\n");
    let (_a_uri, a_path) = temp_fixture(
        "lspmultia.active",
        "include \"lspmultilib.active\"\nport = 1\n",
    );
    let (_b_uri, b_path) = temp_fixture(
        "lspmultib.active",
        "include \"lspmultilib.active\"\nport = 2\n",
    );
    let lib_uri = lsp_types::Url::from_file_path(&lib_path)
        .expect("file uri")
        .to_string();
    let a_uri = lsp_types::Url::from_file_path(&a_path)
        .expect("file uri")
        .to_string();
    let b_uri = lsp_types::Url::from_file_path(&b_path)
        .expect("file uri")
        .to_string();

    let mut server = Framed::spawn();
    server.request(
        "initialize",
        serde_json::json!({"processId": null, "capabilities": {}}),
    );
    server.notify("initialized", serde_json::json!({}));
    let open = |server: &mut Framed, uri: &str, version: i32, text: &str| {
        server.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {"uri": uri, "languageId": "active", "version": version, "text": text},
            }),
        );
    };
    let collect = |server: &mut Framed, n: usize| -> Vec<serde_json::Value> {
        (0..n)
            .map(|_| server.recv_notification("textDocument/publishDiagnostics"))
            .collect()
    };
    let for_uri = |messages: &[serde_json::Value], name: &str| {
        messages
            .iter()
            .find(|m| {
                m["params"]["uri"]
                    .as_str()
                    .is_some_and(|uri| uri.ends_with(name))
            })
            .unwrap_or_else(|| panic!("no publish for {name}: {messages:?}"))
            .clone()
    };
    let has_error = |message: &serde_json::Value| {
        message["params"]["diagnostics"]
            .as_array()
            .expect("array")
            .iter()
            .any(|d| d["code"] == "E001")
    };

    open(&mut server, &lib_uri, 5, "[broken\n");
    let lib_open = server.recv_notification("textDocument/publishDiagnostics");
    assert!(has_error(&lib_open), "{lib_open}");
    open(
        &mut server,
        &a_uri,
        1,
        "include \"lspmultilib.active\"\nport = 1\n",
    );
    open(
        &mut server,
        &b_uri,
        1,
        "include \"lspmultilib.active\"\nport = 2\n",
    );
    // Four publishes: A+lib, B+lib.
    let mut first = collect(&mut server, 4);
    assert!(
        has_error(&for_uri(&first, "lspmultilib.active")),
        "{first:?}"
    );
    // Closing A clears A but republishes the still-owned library
    // markers (never an empty clear while B owns them).
    server.notify(
        "textDocument/didClose",
        serde_json::json!({"textDocument": {"uri": a_uri}}),
    );
    first = collect(&mut server, 2);
    let cleared_a = for_uri(&first, "lspmultia.active");
    assert_eq!(
        cleared_a["params"]["diagnostics"],
        serde_json::json!([]),
        "{first:?}"
    );
    let kept = for_uri(&first, "lspmultilib.active");
    assert!(
        has_error(&kept),
        "library markers must survive A's close: {first:?}"
    );
    // Closing B keeps them too: the library itself is still open and
    // republishes its own state.
    server.notify(
        "textDocument/didClose",
        serde_json::json!({"textDocument": {"uri": b_uri}}),
    );
    first = collect(&mut server, 2);
    let kept = for_uri(&first, "lspmultilib.active");
    assert!(
        has_error(&kept),
        "library markers must survive B's close: {first:?}"
    );
    // Closing the library finally clears it, versionless.
    server.notify(
        "textDocument/didClose",
        serde_json::json!({"textDocument": {"uri": lib_uri}}),
    );
    let cleared = server.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(cleared["params"]["uri"], lib_uri, "{cleared}");
    assert_eq!(
        cleared["params"]["diagnostics"],
        serde_json::json!([]),
        "{cleared}"
    );
    server.shutdown();
}

#[test]
fn stdio_code_action_over_wire() {
    let (_uri, path) = temp_fixture("lspaction.active", "port = \"abc\n");
    let file_uri = lsp_types::Url::from_file_path(&path).expect("file uri");
    let uri = file_uri.to_string();

    let mut server = Framed::spawn();
    server.request(
        "initialize",
        serde_json::json!({"processId": null, "capabilities": {}}),
    );
    server.notify("initialized", serde_json::json!({}));
    server.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": "active",
                "version": 1,
                "text": "port = \"abc\n",
            }
        }),
    );
    // Drain the open publish before requesting.
    let _ = server.recv_notification("textDocument/publishDiagnostics");
    let actions = server.request(
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 1, "character": 0},
            },
            "context": {"diagnostics": []},
        }),
    );
    let actions = actions["result"].as_array().expect("actions");
    assert!(
        actions.iter().any(|action| action["title"]
            .as_str()
            .is_some_and(|title| title.starts_with("Apply healing (H"))),
        "{actions:?}"
    );
    server.shutdown();
}

#[test]
fn stdio_graph_methods_over_wire() {
    let (_uri, path) = temp_fixture("lspgraph.active", GRAPH_DOC);
    let file_uri = lsp_types::Url::from_file_path(&path).expect("file uri");
    let uri = file_uri.to_string();

    let mut server = Framed::spawn();
    server.request(
        "initialize",
        serde_json::json!({"processId": null, "capabilities": {}}),
    );
    server.notify("initialized", serde_json::json!({}));
    server.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": "active",
                "version": 1,
                "text": GRAPH_DOC,
            }
        }),
    );
    let _ = server.recv_notification("textDocument/publishDiagnostics");
    let graph = server.request(
        "active/graph",
        serde_json::json!({"textDocument": {"uri": uri}}),
    );
    assert!(
        graph["result"]["mermaid"]
            .as_str()
            .is_some_and(|m| m.starts_with("flowchart TD")),
        "{graph}"
    );
    assert!(
        graph["result"]["dot"]
            .as_str()
            .is_some_and(|d| d.starts_with("digraph active {")),
        "{graph}"
    );
    assert!(graph["result"]["json"]["nodes"].is_array(), "{graph}");
    let impact = server.request(
        "active/impact",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 2, "character": 6},
        }),
    );
    assert_eq!(impact["result"]["key"], "base", "{impact}");
    assert!(
        impact["result"]["paths"]
            .as_array()
            .expect("paths")
            .iter()
            .any(|p| p == "a.y"),
        "{impact}"
    );
    let why = server.request(
        "active/why",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 3, "character": 0},
        }),
    );
    assert_eq!(why["result"]["key"], "a.y", "{why}");
    // Malformed params are InvalidParams, not hangs.
    let bad = server.request("active/impact", serde_json::json!({}));
    assert_eq!(bad["error"]["code"], -32602, "{bad}");
    server.shutdown();
}

#[test]
fn stdio_stale_include_groups_withdrawn() {
    let (_lib_uri, lib_path) = temp_fixture("lspstalelib.active", "[broken\n");
    let (_main_uri, main_path) = temp_fixture(
        "lspstalemain.active",
        "include \"lspstalelib.active\"\nport = 1\n",
    );
    let lib_uri = lsp_types::Url::from_file_path(&lib_path)
        .expect("file uri")
        .to_string();
    let main_uri = lsp_types::Url::from_file_path(&main_path)
        .expect("file uri")
        .to_string();

    let mut server = Framed::spawn();
    let init = server.request(
        "initialize",
        serde_json::json!({
            "processId": null,
            "capabilities": {},
        }),
    );
    assert!(init.get("result").is_some(), "{init}");
    server.notify("initialized", serde_json::json!({}));
    // Open the library itself: its group carries the library's own
    // version, and proves the E-code is real before main opens.
    server.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": lib_uri,
                "languageId": "active",
                "version": 5,
                "text": "[broken\n",
            }
        }),
    );
    let lib_open = server.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(lib_open["params"]["uri"], lib_uri, "{lib_open}");
    assert_eq!(lib_open["params"]["version"], 5, "{lib_open}");
    // Open main: two groups — main at version 1, the library still
    // at its own version 5 (never the parent's).
    server.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": main_uri,
                "languageId": "active",
                "version": 1,
                "text": "include \"lspstalelib.active\"\nport = 1\n",
            }
        }),
    );
    let first = server.recv_notification("textDocument/publishDiagnostics");
    let second = server.recv_notification("textDocument/publishDiagnostics");
    // Canonical URIs again: match groups by file name.
    let is_lib = |message: &serde_json::Value| {
        message["params"]["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("lspstalelib.active"))
    };
    let is_main = |message: &serde_json::Value| {
        message["params"]["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("lspstalemain.active"))
    };
    assert!(
        (is_lib(&first) && is_main(&second)) || (is_main(&first) && is_lib(&second)),
        "{first} {second}"
    );
    let lib_group = if is_lib(&first) { &first } else { &second };
    assert_eq!(lib_group["params"]["version"], 5, "{lib_group}");
    // Remove the include: the library is still open itself, so its
    // markers are preserved (republished from the remaining owner),
    // never withdrawn while an owner remains.
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": main_uri, "version": 2},
            "contentChanges": [{"text": "port = 1\n"}],
        }),
    );
    let mut saw_withdraw = false;
    for _ in 0..2 {
        let message = server.recv_notification("textDocument/publishDiagnostics");
        // Server include URIs are canonical (8.3 short-names on
        // Windows CI); match by file name like `for_uri` above.
        let is_lib = message["params"]["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("lspstalelib.active"));
        if is_lib {
            // The library is still open itself: its markers are
            // preserved (republished from the remaining owner),
            // never withdrawn while an owner remains.
            assert!(
                !message["params"]["diagnostics"]
                    .as_array()
                    .expect("array")
                    .is_empty(),
                "open library markers must survive: {message}"
            );
            saw_withdraw = true;
        }
    }
    assert!(saw_withdraw, "library group must be preserved");
    // Closing the library withdraws its group: no open document owns
    // it anymore, so it clears (versionless).
    server.notify(
        "textDocument/didClose",
        serde_json::json!({"textDocument": {"uri": lib_uri}}),
    );
    let withdrawn = server.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(withdrawn["params"]["uri"], lib_uri, "{withdrawn}");
    assert_eq!(
        withdrawn["params"]["diagnostics"],
        serde_json::json!([]),
        "{withdrawn}"
    );
    // Re-add the include while the library is closed: its group
    // republishes from disk, versionless (no open buffer owns it).
    // Removing it again clears the group the same way.
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": main_uri, "version": 3},
            "contentChanges": [{"text": "include \"lspstalelib.active\"\nport = 1\n"}],
        }),
    );
    let mut saw_disk = false;
    for _ in 0..2 {
        let message = server.recv_notification("textDocument/publishDiagnostics");
        // Server include URIs are canonical (8.3 short-names on
        // Windows CI); match by file name like `for_uri` above.
        let is_lib = message["params"]["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("lspstalelib.active"));
        if is_lib {
            assert!(
                !message["params"]["diagnostics"]
                    .as_array()
                    .expect("array")
                    .is_empty(),
                "{message}"
            );
            assert!(
                message["params"].get("version").is_none_or(|v| v.is_null()),
                "disk-derived group must be versionless: {message}"
            );
            saw_disk = true;
        }
    }
    assert!(saw_disk, "library group must republish from disk");
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": main_uri, "version": 4},
            "contentChanges": [{"text": "port = 1\n"}],
        }),
    );
    let mut saw_clear = false;
    for _ in 0..2 {
        let message = server.recv_notification("textDocument/publishDiagnostics");
        // Server include URIs are canonical (8.3 short-names on
        // Windows CI); match by file name like `for_uri` above.
        let is_lib = message["params"]["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("lspstalelib.active"));
        if is_lib {
            assert_eq!(
                message["params"]["diagnostics"],
                serde_json::json!([]),
                "{message}"
            );
            saw_clear = true;
        }
    }
    assert!(saw_clear, "unowned library group must clear");
    server.notify(
        "textDocument/didClose",
        serde_json::json!({"textDocument": {"uri": main_uri}}),
    );
    let cleared = server.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(cleared["params"]["uri"], main_uri, "{cleared}");
    assert_eq!(
        cleared["params"]["diagnostics"],
        serde_json::json!([]),
        "{cleared}"
    );
    server.shutdown();
}

#[test]
fn carried_memo_turns_reanalysis_into_hits() {
    // PE.003: re-analyzing an unchanged document with the previous
    // revision's memo must hit instead of re-walking the arena.
    let (uri, _path) = temp_fixture("lspmemo.active", "port = 1\n");
    let text = std::fs::read_to_string(uri.to_file_path().expect("uri")).expect("read back");
    let first = analysis::open_doc(uri.clone(), text.clone(), 1, None);
    assert_eq!(first.memo.last_memo_hits, 0);
    let second = analysis::open_doc(uri, text, 2, Some(first.memo));
    assert!(
        second.memo.last_memo_hits > 0,
        "carried memo must hit on unchanged content"
    );
}
