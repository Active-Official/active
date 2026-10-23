//! Language-server analysis benchmarks (TOOLING QUALITY.003): document
//! open (parse + evaluate + lint), symbols, completion, semantic
//! tokens, and value inspection over a realistic document.

use active_ls::analysis;
use codspeed_criterion_compat::{criterion_group, criterion_main, Criterion};
use lsp_types::Url;

fn document(tables: usize, keys: usize) -> String {
    let mut out = String::new();
    out.push_str("schema = { port = Int(default = 8080), token = String }\n");
    out.push_str("port = 8080\n");
    out.push_str("token = secret(\"bench-secret\")\n");
    for t in 0..tables {
        out.push_str(&format!("[t{t}]\n"));
        for k in 0..keys {
            out.push_str(&format!("k{k} = {t}\n"));
        }
    }
    out
}

fn bench(c: &mut Criterion) {
    let text = document(20, 50);
    let uri = Url::parse("file:///bench.active").expect("url");

    c.bench_function("lsp_open_doc_1k", |b| {
        b.iter(|| analysis::open_doc(uri.clone(), text.clone(), 1, None));
    });

    let doc = analysis::open_doc(uri.clone(), text.clone(), 1, None);
    c.bench_function("lsp_document_symbols_1k", |b| {
        b.iter(|| analysis::document_symbols(&doc));
    });
    c.bench_function("lsp_completion_1k", |b| {
        b.iter(|| analysis::completion_at(&doc, 2, 5));
    });
    c.bench_function("lsp_semantic_tokens_1k", |b| {
        b.iter(|| analysis::semantic_tokens_absolute(&text));
    });
    c.bench_function("inspect_path_1k", |b| {
        let value = doc.value.as_ref().expect("value");
        b.iter(|| active_core::convert::inspect_path(&doc.module, value, "port").expect("inspect"));
    });
    c.bench_function("graph_mermaid_1k", |b| {
        b.iter(|| active_core::graph::to_mermaid(&doc.module));
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
