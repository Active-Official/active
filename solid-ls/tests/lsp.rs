//! Language server tests (C-043, spec/lsp.md §7).
//!
//! Handler unit tests (no I/O) over parsed fixtures, plus one stdio
//! end-to-end test driving the real binary through initialize →
//! didOpen → hover/definition → shutdown.

use solid_ls::analysis;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};

fn untitled(name: &str) -> lsp_types::Url {
    lsp_types::Url::parse(&format!("untitled:{name}")).expect("uri")
}

fn open_untitled(name: &str, text: &str) -> analysis::Doc {
    analysis::open_doc(untitled(name), text.to_string(), 1)
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
    assert_eq!(analysis::token_legend().token_types.len(), 7);
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
        "lspmissing.solid",
        "include \"does-not-exist-xyz.solid\"\nport = 1\n",
    );
    let text = std::fs::read_to_string(uri.to_file_path().expect("file uri")).expect("read back");
    let doc = analysis::open_doc(uri, text, 1);
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
    let dir = std::env::temp_dir().join("solid-ls-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    let uri = lsp_types::Url::from_file_path(&path).expect("file uri");
    (uri, path.to_string_lossy().into_owned())
}

#[test]
fn definition_jumps_across_includes() {
    let (_lib_uri, _lib_path) = temp_fixture("lsplib2.solid", "host = \"db\"\n");
    let (main_uri, _main_path) =
        temp_fixture("lspmain2.solid", "include \"lsplib2.solid\"\nurl = host\n");
    let text =
        std::fs::read_to_string(main_uri.to_file_path().expect("file uri")).expect("read back");
    let doc = analysis::open_doc(main_uri.clone(), text, 1);
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
        locations[0].uri.path().ends_with("lsplib2.solid"),
        "{locations:?}"
    );
    assert_eq!(locations[0].range.start.line, 0, "{locations:?}");
    // References with declaration: the lib definition plus the main use.
    let refs = analysis::references_at(&doc, &lookup, 1, 7, true);
    assert_eq!(refs.len(), 2, "{refs:?}");
    assert!(refs.iter().any(|l| l.uri.path().ends_with("lsplib2.solid")));
    assert!(refs
        .iter()
        .any(|l| l.uri.path().ends_with("lspmain2.solid")));
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_solid-ls"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn solid-ls");
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
}

#[test]
fn stdio_end_to_end_initialize_hover_definition_shutdown() {
    let (_uri, path) = temp_fixture("lspe2e.solid", "# @intent: \"E2E port\"\nport = 8080\n");
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
                "languageId": "solid",
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
