//! Redaction sweep over LSP responses (C-036): completions, inlay
//! hints, diagnostics, code actions, and graph snapshots over a
//! secret-bearing document must never carry the raw value.

use active_ls::analysis;
use lsp_types::Url;

const RAW: &str = "s3cr3t-lsp-sweep";
const DOC: &str = "token = secret(\"s3cr3t-lsp-sweep\")\nport = 8080\nurl = \"https://x\"\n";

fn open() -> analysis::Doc {
    analysis::open_doc(
        Url::parse("untitled:sweep").expect("uri"),
        DOC.to_string(),
        1,
    )
}

fn scrubbed(text: &str, what: &str) {
    assert!(!text.contains(RAW), "{what} leaked raw secret: {text}");
}

#[test]
fn completion_labels_and_details_carry_no_secrets() {
    let doc = open();
    for line in 0..3 {
        for ch in [0, 2, 6] {
            for item in analysis::completion_at(&doc, line, ch) {
                scrubbed(&item.label, "completion label");
                if let Some(detail) = item.detail {
                    scrubbed(&detail, "completion detail");
                }
                if let Some(doc) = item.documentation {
                    scrubbed(&format!("{doc:?}"), "completion docs");
                }
            }
        }
    }
}

#[test]
fn inlay_hints_show_kinds_not_values() {
    let doc = open();
    for hint in analysis::inlay_hints(&doc, None) {
        let label = match &hint.label {
            lsp_types::InlayHintLabel::String(s) => s.clone(),
            lsp_types::InlayHintLabel::LabelParts(parts) => {
                parts.iter().map(|p| p.value.clone()).collect()
            }
        };
        scrubbed(&label, "inlay hint");
    }
}

#[test]
fn diagnostics_carry_no_secrets() {
    // Broken-but-secret doc: diagnostics must describe structure,
    // never values.
    let doc = analysis::open_doc(
        Url::parse("untitled:sweep-broken").expect("uri"),
        "token = secret(\"s3cr3t-lsp-sweep\")\nport = \n".to_string(),
        1,
    );
    for d in analysis::diagnostics_for(&doc) {
        scrubbed(&d.message, "diagnostic");
    }
    // Healthy doc: no diagnostics at all, hence nothing to leak.
    assert!(analysis::diagnostics_for(&open()).is_empty());
}

#[test]
fn code_actions_carry_no_secrets() {
    // Source-to-source actions (reformat) echo the user's own locator
    // text — the same contract as the formatter — so they must equal
    // the SHIPPED formatter output exactly, never anything else.
    // All other action content must be secret-free.
    let doc = open();
    let module = active_core::parse(DOC, "sweep").expect("must parse");
    let formatted: Vec<String> = [
        active_core::fmt::format(&module, active_core::Style::Brace),
        active_core::fmt::format(&module, active_core::Style::Indent),
    ]
    .into_iter()
    .collect();
    let range = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 2,
            character: 20,
        },
    };
    assert!(
        !analysis::code_actions(&doc, range).is_empty(),
        "expected actions for test"
    );
    for action in analysis::code_actions(&doc, range) {
        scrubbed(&action.title, "action title");
        if let Some(edit) = action.edit {
            let changes = edit.changes.unwrap_or_default();
            let texts: Vec<&str> = changes
                .values()
                .flatten()
                .map(|e| e.new_text.as_str())
                .collect();
            for text in texts {
                if text.contains(RAW) {
                    assert!(
                        formatted.iter().any(|f| f == text),
                        "edit text with secret material is not formatter output: {text:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn graph_snapshot_impact_and_why_carry_no_secrets() {
    let doc = open();
    let (graph, dot, json) = analysis::graph_snapshot(&doc);
    scrubbed(&graph, "graph text");
    scrubbed(&dot, "graph dot");
    scrubbed(&serde_json::to_string(&json).expect("json"), "graph json");
    if let Some((label, deps)) = analysis::impact_at(&doc, 0, 1) {
        scrubbed(&label, "impact label");
        for d in deps {
            scrubbed(&d, "impact dep");
        }
    }
    if let Some((label, chain)) = analysis::why_at(&doc, 0, 1) {
        scrubbed(&label, "why label");
        for c in chain {
            scrubbed(&c, "why link");
        }
    }
}
