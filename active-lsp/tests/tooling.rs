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
