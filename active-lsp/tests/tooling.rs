//! Tooling-surface tests (LSP.008/009/010, LSP.007): formatting,
//! symbols, completion depth, semantic-token classes, and code-action
//! plans. These drive the shipped analysis functions the server calls.

use active_ls::analysis::{self, Doc};
use lsp_types::{Position, Range, Url};

fn open(text: &str) -> Doc {
    analysis::open_doc(
        Url::parse("file:///tooling.active").expect("url"),
        text.to_string(),
        1,
        None,
    )
}

#[test]
fn formatting_edits_are_canonical_and_idempotent() {
    let doc = open("port=8080\n");
    let edits = analysis::formatting_edits(&doc);
    assert_eq!(edits.len(), 1, "unformatted text needs one edit");
    assert_eq!(edits[0].new_text, "port = 8080\n");

    let canonical = open("port = 8080\n");
    assert!(
        analysis::formatting_edits(&canonical).is_empty(),
        "canonical text yields no edit"
    );
}

#[test]
fn range_formatting_stays_inside_the_selection() {
    let doc = open("[app]\nport=8080\nname = \"x\"\n");
    let range = Range {
        start: Position {
            line: 1,
            character: 0,
        },
        end: Position {
            line: 2,
            character: 8,
        },
    };
    let edits = analysis::range_formatting_edits(&doc, range);
    assert_eq!(edits.len(), 1, "changed hunk inside range");
    for edit in &edits {
        assert!(edit.range.start.line >= 1, "{edit:?}");
        assert!(edit.range.end.line <= 2, "{edit:?}");
        assert!(edit.new_text.contains("port = 8080"), "{edit:?}");
    }
    // A selection far from the hunk yields nothing.
    let elsewhere = Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 0,
            character: 4,
        },
    };
    assert!(analysis::range_formatting_edits(&doc, elsewhere).is_empty());
}

#[test]
fn document_symbols_cover_paths_and_bindings() {
    let doc = open("let limit = 3\n[server]\nhost = \"x\"\nport = limit\n");
    let symbols = analysis::document_symbols(&doc);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    for expected in ["server", "host", "port", "limit"] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    assert!(symbols
        .iter()
        .any(|s| s.container_name.as_deref() == Some("server")));
}

#[test]
fn workspace_symbols_filter_by_query() {
    let a = open("[server]\nhost = \"x\"\n");
    let b = open("[cache]\nttl = 5\n");
    let all = analysis::workspace_symbols([&a, &b].into_iter(), "");
    assert!(all.len() >= 4, "{all:?}");
    let filtered = analysis::workspace_symbols([&a, &b].into_iter(), "host");
    assert!(
        filtered.iter().all(|s| s.name.contains("host")),
        "{filtered:?}"
    );
    assert!(!filtered.is_empty());
}

#[test]
fn completion_offers_units_and_mode_keys() {
    // Unit completion inserts after the digits without replacing them.
    let doc = open("timeout = 30");
    let items = analysis::completion_at(&doc, 0, 12);
    assert!(
        items.iter().any(|i| i.label == "ms"),
        "units offered: {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
    let ms = items.iter().find(|i| i.label == "ms").expect("ms item");
    match ms.text_edit.as_ref() {
        Some(lsp_types::CompletionTextEdit::Edit(edit)) => {
            assert_eq!(edit.range.start.character, 12, "zero-width insertion");
            assert_eq!(edit.range.end.character, 12);
        }
        other => panic!("expected a text edit, got {other:?}"),
    }

    // Root-level metadata keys complete by prefix.
    let doc = open("active_");
    let items = analysis::completion_at(&doc, 0, 7);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"active_mode"), "{labels:?}");
    assert!(labels.contains(&"active_capabilities"), "{labels:?}");
}

#[test]
fn semantic_tokens_classify_secret_mode_blob_schema() {
    let legend = analysis::token_legend();
    let types: Vec<String> = legend
        .token_types
        .iter()
        .map(|t| t.as_str().to_string())
        .collect();
    for custom in ["secret", "mode", "contentAddressed", "schema"] {
        assert!(types.iter().any(|t| t == custom), "legend lacks {custom}");
    }
    let secret_idx = types.iter().position(|t| t == "secret").unwrap() as u32;
    let mode_idx = types.iter().position(|t| t == "mode").unwrap() as u32;
    let ca_idx = types.iter().position(|t| t == "contentAddressed").unwrap() as u32;
    let schema_idx = types.iter().position(|t| t == "schema").unwrap() as u32;

    let text = "active_mode = \"data\"\nschema = { a = Int }\nlet pw = secret(\"x\")\nid = blob(\"text/plain\", \"hi\")\n";
    let tokens = analysis::semantic_tokens_absolute(text);
    let have = |idx: u32| tokens.iter().any(|t| t.3 == idx);
    assert!(have(mode_idx), "mode marker classified: {tokens:?}");
    assert!(have(schema_idx), "schema classified: {tokens:?}");
    assert!(have(secret_idx), "secret classified: {tokens:?}");
    assert!(have(ca_idx), "blob classified: {tokens:?}");
}

#[test]
fn code_actions_offer_mode_marker_and_repair_plan() {
    let doc = open("port=8080\n");
    let actions = analysis::code_actions(
        &doc,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        },
    );
    let titles: Vec<&str> = actions.iter().map(|a| a.title.as_str()).collect();
    assert!(
        titles.iter().any(|t| t.contains("active_mode")),
        "mode marker action: {titles:?}"
    );

    let broken = open("schema = { port = Int(default = 8080) }\n");
    let plan = analysis::code_actions(
        &broken,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        },
    );
    assert!(
        plan.iter().any(|a| a.title.contains("repair plan")),
        "multi-step plan action: {:?}",
        plan.iter().map(|a| &a.title).collect::<Vec<_>>()
    );
}

#[test]
fn formatting_refuses_broken_documents() {
    let doc = open("port=8080\nbad = = 2\n");
    assert!(
        analysis::formatting_edits(&doc).is_empty(),
        "a document with parse errors must not be reformatted"
    );
    let full = Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 2,
            character: 0,
        },
    };
    assert!(analysis::range_formatting_edits(&doc, full).is_empty());
}

#[test]
fn formatting_preserves_includes_without_inlining() {
    let dir = std::env::temp_dir().join(format!("active-lsp-fmt-inc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("inc.active"), "host = \"included\"\nport = 8080\n").expect("include");
    let main = dir.join("main.active");
    let text = "include \"inc.active\"\nlocal=1\n";
    std::fs::write(&main, text).expect("main");
    let uri = Url::from_file_path(&main).expect("file url");
    let doc = analysis::open_doc(uri, text.to_string(), 1, None);
    let edits = analysis::formatting_edits(&doc);
    assert_eq!(edits.len(), 1, "unformatted local line needs one edit");
    let formatted = &edits[0].new_text;
    assert!(
        formatted.contains("include \"inc.active\""),
        "include statement preserved: {formatted:?}"
    );
    assert!(
        !formatted.contains("host") && !formatted.contains("8080"),
        "included keys must not be inlined into the buffer: {formatted:?}"
    );
    let range = Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 1,
            character: 0,
        },
    };
    for edit in analysis::range_formatting_edits(&doc, range) {
        assert!(
            !edit.new_text.contains("host") && !edit.new_text.contains("8080"),
            "range formatting inlined includes: {:?}",
            edit.new_text
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn repair_action_uses_included_schema_and_stays_in_the_buffer() {
    let dir = std::env::temp_dir().join(format!("active-lsp-repair-inc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("schema.active"),
        "schema = { port = Int(default = 8080) }\n",
    )
    .expect("include");
    let main = dir.join("main.active");
    let text = "include \"schema.active\"\nname = \"x\"\n";
    std::fs::write(&main, text).expect("main");
    let uri = Url::from_file_path(&main).expect("file url");
    let doc = analysis::open_doc(uri, text.to_string(), 1, None);
    let actions = analysis::code_actions(
        &doc,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        },
    );
    let repair = actions
        .iter()
        .find(|action| action.title.contains("repair plan"))
        .unwrap_or_else(|| panic!("no repair plan from an included schema: {actions:?}"));
    let edit = repair
        .edit
        .as_ref()
        .and_then(|workspace| workspace.changes.as_ref())
        .and_then(|changes| changes.values().next())
        .and_then(|edits| edits.first())
        .expect("repair edit");
    assert!(
        edit.new_text.contains("@patch(\"port\", 8080)"),
        "included schema must drive the plan: {:?}",
        edit.new_text
    );
    assert!(
        !edit.new_text.contains("schema = {"),
        "included schema must not be copied into the buffer: {:?}",
        edit.new_text
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn untitled_buffers_do_not_read_includes_from_disk() {
    let doc = analysis::open_doc(
        Url::parse("untitled:hermetic").expect("url"),
        "include \"definitely-not-here.active\"\nlocal = 1\n".to_string(),
        1,
        None,
    );
    assert!(
        doc.parse_diags.is_empty(),
        "an untitled include must not be loaded: {:?}",
        doc.parse_diags
    );
    let value = doc.value.as_ref().expect("value");
    let active_core::Value::Table(map) = value else {
        panic!("expected a table, got {value:?}");
    };
    assert!(matches!(map.get("local"), Some(active_core::Value::Int(1))));
    assert!(!map.contains_key("included_key"), "{map:?}");
}

#[test]
fn analysis_and_repair_ignore_ambient_environment() {
    // H3 regression: the server engine is hermetic. Setting a process
    // variable must not change the evaluated tree or a repair action.
    std::env::set_var("ACTIVE_LSP_REPAIR_PROBE", "from-ambient");
    let doc = open("p = env.ACTIVE_LSP_REPAIR_PROBE\nname = p ?: \"fallback\"\n");
    let value = doc.value.as_ref().expect("evaluated value");
    let active_core::Value::Table(map) = value else {
        panic!("expected a table, got {value:?}");
    };
    assert!(
        matches!(map.get("p"), Some(active_core::Value::Null)),
        "ambient environment reached analysis: {map:?}"
    );
    let actions = analysis::code_actions(
        &doc,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        },
    );
    for action in &actions {
        let rendered = format!("{action:?}");
        assert!(
            !rendered.contains("from-ambient"),
            "repair action used ambient state: {rendered}"
        );
    }
    std::env::remove_var("ACTIVE_LSP_REPAIR_PROBE");
}

#[test]
fn inlay_hints_withhold_material_values() {
    // A plain number used as a secret locator must not publish its
    // normalized value in a hint.
    let doc = open("let x = 42\nk = secret(x)\nq = 1ms * x\n");
    let hints = analysis::inlay_hints(&doc, None);
    for hint in &hints {
        let label = format!("{:?}", hint.label);
        assert!(!label.contains("42"), "inlay hint leaked material: {label}");
    }
    assert!(!hints.is_empty(), "hints expected for a valid document");
}

#[test]
fn document_symbols_exclude_included_files() {
    let dir = std::env::temp_dir().join(format!("active-lsp-symbols-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("inc.active"), "host = \"included\"\n").expect("include");
    let main = dir.join("main.active");
    let text = "include \"inc.active\"\nport = 1\n";
    std::fs::write(&main, text).expect("main");
    let uri = Url::from_file_path(&main).expect("file url");
    let doc = analysis::open_doc(uri, text.to_string(), 1, None);
    let symbols = analysis::document_symbols(&doc);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"port"), "{names:?}");
    assert!(
        !names.contains(&"host"),
        "included symbols must not be attributed to the open document: {names:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mode_marker_action_uses_exact_key() {
    let lookalike = open("active_mode_extra = 1\n");
    let actions = analysis::code_actions(
        &lookalike,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        },
    );
    assert!(
        actions.iter().any(|a| a.title.contains("active_mode")),
        "lookalike key must not suppress the marker action"
    );
    let real = open("active_mode = \"data\"\n");
    let actions = analysis::code_actions(
        &real,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        },
    );
    assert!(!actions.iter().any(|a| a.title.contains("Set active_mode")));

    // A nested key with the same name is data, not a root marker.
    let nested = open("[server]\nactive_mode = \"data\"\n");
    let actions = analysis::code_actions(
        &nested,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        },
    );
    assert!(
        actions.iter().any(|a| a.title.contains("active_mode")),
        "nested marker must not suppress the root action"
    );
}

#[test]
fn style_conversion_is_withheld_for_broken_documents() {
    let doc = open("port=8080\nbad = = 2\n");
    let actions = analysis::code_actions(
        &doc,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 6,
            },
        },
    );
    assert!(
        !actions.iter().any(|a| a.title.starts_with("Convert to")),
        "style conversion must not rewrite a broken buffer: {:?}",
        actions.iter().map(|a| &a.title).collect::<Vec<_>>()
    );
}
