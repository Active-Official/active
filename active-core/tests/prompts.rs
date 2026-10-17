//! Prompt-pack self-test (C-053): deterministic, CI-gated.
//!
//! The LLM validation (C-054) cannot run in CI — models are
//! nondeterministic and backends are local. What CI *can* pin:
//! the pack version is consistent everywhere, and every ```active
//! fence shipped in the prompts parses cleanly (an agent copying
//! a few-shot must never receive broken syntax).

use std::path::{Path, PathBuf};

fn prompts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("prompts")
}

fn extract_active_fences(markdown: &str) -> Vec<String> {
    // Info line must be exactly `active`: inline mentions like
    // "```active fenced block" in prose are skipped one ```` ``` ````
    // at a time so they can never swallow a real fence.
    let mut fences = Vec::new();
    let mut rest = markdown;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let (info, body) = match after.find('\n') {
            Some(i) => (&after[..i], &after[i + 1..]),
            None => break,
        };
        if !info.trim().eq_ignore_ascii_case("active") {
            rest = body;
            continue;
        }
        let end = match body.find("\n```") {
            Some(i) => i,
            None => break,
        };
        fences.push(body[..end].to_string());
        rest = &body[end + 4..];
    }
    fences
}

#[test]
fn pack_version_is_consistent() {
    let version = std::fs::read_to_string(prompts_dir().join("VERSION"))
        .expect("VERSION")
        .trim()
        .to_string();
    assert!(!version.is_empty());
    let mut files = Vec::new();
    for entry in std::fs::read_dir(prompts_dir().join("v1")).expect("v1") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|ext| ext == "md") {
            files.push(path);
        }
    }
    assert!(!files.is_empty(), "prompt files expected");
    for path in files {
        let text = std::fs::read_to_string(&path).expect("read prompt");
        assert!(
            text.contains(&format!("prompt-pack v{version}")),
            "{path:?} must carry the pack version"
        );
    }
}

#[test]
fn every_shipped_fence_parses() {
    let mut total = 0;
    for entry in std::fs::read_dir(prompts_dir().join("v1")).expect("v1") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|ext| ext != "md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read prompt");
        for fence in extract_active_fences(&text) {
            total += 1;
            let module = active_core::parse(&fence, "prompts")
                .unwrap_or_else(|d| panic!("fence in {path:?} must parse: {d:?}"));
            // Few-shots must also evaluate (no dangling references).
            active_core::Engine::new()
                .evaluate(&module)
                .unwrap_or_else(|e| panic!("fence in {path:?} must evaluate: {e}"));
        }
    }
    assert!(total >= 5, "expected several fences, found {total}");
}
