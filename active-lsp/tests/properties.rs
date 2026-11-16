//! Property-style invariants for tooling analysis (TOOLING QUALITY.002):
//! generated documents never panic any analysis entry point, semantic
//! tokens stay inside their lines, and secret material never reaches
//! hover or inspection output. Deterministic LCG, no external deps.

use active_ls::analysis;
use lsp_types::Url;

fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed
}

fn generate(seed: u64) -> String {
    let mut state = seed;
    let count = 5 + (next(&mut state) % 15) as usize;
    let mut out = String::from("schema = { port = Int(min = 1), token = String }\n");
    out.push_str("port = 8080\n");
    for index in 0..count {
        match next(&mut state) % 5 {
            0 => out.push_str(&format!("k{index} = {}\n", next(&mut state) % 1000)),
            1 => out.push_str(&format!("k{index} = \"text-{index}\"\n")),
            2 => out.push_str(&format!("k{index} = secret(\"hunter2-{index}\")\n")),
            3 => out.push_str(&format!("k{index} = true\n")),
            _ => {
                out.push_str(&format!("[t{index}]\n"));
                out.push_str(&format!("inner = {}\n", next(&mut state) % 10));
            }
        }
    }
    out
}

fn open(text: &str) -> analysis::Doc {
    analysis::open_doc(
        Url::parse("file:///property.active").expect("url"),
        text.to_string(),
        1,
        None,
    )
}

#[test]
fn analysis_entry_points_are_total_over_generated_documents() {
    for seed in 1..=25u64 {
        let text = generate(seed);
        let doc = open(&text);
        let symbols = analysis::document_symbols(&doc);
        assert!(
            symbols.len() >= 2,
            "seed {seed}: expected root paths, got {symbols:?}"
        );
        // Every symbol range must be inside the document.
        let line_count = text.split('\n').count() as u32;
        for symbol in &symbols {
            assert!(
                symbol.location.range.start.line < line_count,
                "seed {seed}: symbol outside document: {symbol:?}"
            );
        }
        let _ = analysis::completion_at(&doc, 0, 0);
        let _ = analysis::inlay_hints(&doc, None);
        let _ = analysis::code_actions(
            &doc,
            lsp_types::Range {
                start: lsp_types::Position {
                    line: 0,
                    character: 0,
                },
                end: lsp_types::Position {
                    line: 0,
                    character: 0,
                },
            },
        );
        let _ = analysis::diagnostics_for(&doc);

        // Semantic tokens stay within their lines.
        for (line, start, length, _) in analysis::semantic_tokens_absolute(&text) {
            let line_text = text.split('\n').nth(line as usize).unwrap_or("");
            let width: u32 = line_text.encode_utf16().count() as u32;
            assert!(
                start <= width && start.saturating_add(length) <= width,
                "seed {seed}: token out of bounds on line {line}"
            );
        }
    }
}

#[test]
fn hover_and_inspection_never_reveal_generated_secrets() {
    for seed in 1..=15u64 {
        let text = generate(seed);
        let doc = open(&text);
        let line_count = text.split('\n').count();
        for line in 0..line_count {
            if let Some(hover) = analysis::hover_at(&doc, line as u32, 0) {
                let rendered = format!("{hover:?}");
                assert!(
                    !rendered.contains("hunter2"),
                    "seed {seed}: hover leaked a secret on line {line}"
                );
            }
        }
        if let Some(value) = doc.value.as_ref() {
            for symbol in analysis::document_symbols(&doc) {
                // Inspection is a publishing surface too: when the path
                // resolves, its report must not carry generated material.
                if let Ok(report) =
                    active_core::convert::inspect_path(&doc.module, value, &symbol.name)
                {
                    let rendered = report.to_string();
                    assert!(
                        !rendered.contains("hunter2"),
                        "seed {seed}: inspection leaked for {}: {rendered}",
                        symbol.name
                    );
                }
            }
        }
    }
}

#[test]
fn hover_redacts_secret_locator_and_interpolation() {
    let text = "x = \"hunter2-hover\"\nmsg = \"prefix ${x}\"\nk = secret(x)\n";
    let doc = open(text);
    for line in 0..3 {
        if let Some(hover) = analysis::hover_at(&doc, line, 0) {
            let rendered = format!("{hover:?}");
            assert!(
                !rendered.contains("hunter2"),
                "hover leaked a secret on line {line}: {rendered}"
            );
        }
    }
    let hover = analysis::hover_at(&doc, 1, 0).expect("hover on msg");
    let rendered = format!("{hover:?}");
    assert!(
        rendered.contains("SECRET"),
        "interpolated secret must render the marker: {rendered}"
    );
}

#[test]
fn publishing_many_diagnostics_is_linear_in_the_diagnostics() {
    // LANG-C-005 follow-up: every duplicate key heals to a W011 warning,
    // so a generated config can publish tens of thousands of diagnostics.
    // Conversion used to re-split the whole document per position (2.8 s
    // for 20k warnings on 120 KB, 80 s for a 3 MB buffer). The cached line
    // index makes this a few milliseconds; the bound is deliberately far
    // above that so machine speed cannot make the test flap, while a
    // return to per-diagnostic splitting (2.8 s) fails it.
    let mut src = String::new();
    for _ in 0..20_000 {
        src.push_str("k = 1\n");
    }
    let uri = Url::parse("file:///many-diagnostics.active").expect("uri");
    let doc = analysis::open_doc(uri, src, 1, None);
    // Best of three: a cold page cache or a loaded CI runner can spike one
    // call; the regression this guards (per-diagnostic text splitting) is
    // uniform across runs.
    let mut elapsed = std::time::Duration::MAX;
    let mut published = 0usize;
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let groups = analysis::diagnostics_by_file(&doc, &|_: &str| None);
        elapsed = elapsed.min(started.elapsed());
        published = groups.iter().map(|(_, diags)| diags.len()).sum();
    }
    // Parse diagnostics are capped (1024 records), so the count is bounded
    // while the TEXT is not: conversion cost was 1024 x text length, which
    // is the quadratic term this test guards.
    assert!(
        (1_000..=1_024).contains(&published),
        "duplicates must publish up to the diagnostic cap, got {published}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "publishing {published} diagnostics took {elapsed:?} at best; the line index regressed"
    );
}
