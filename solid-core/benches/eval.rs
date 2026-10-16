//! Evaluator benchmarks (C-055, spec/performance.md §1).
//!
//! Continuous tracking runs through the CodSpeed CI plugin; local
//! runs use `cargo bench`. The 10k document is generated in code
//! (50 tables × 200 chained keys): wide enough for map behavior,
//! chained enough for topological depth.

use codspeed_criterion_compat::{criterion_group, criterion_main, BatchSize, Criterion};
use solid_core::{parse, Engine, IncrementalMemo};

fn doc_10k() -> String {
    let mut out = String::new();
    for t in 0..50 {
        out.push_str(&format!("[t{t}]\n"));
        out.push_str(&format!("k0 = {t}\n"));
        for k in 1..200 {
            out.push_str(&format!("k{k} = k{} + 1\n", k - 1));
        }
    }
    out
}

fn criterion_benchmark(c: &mut Criterion) {
    let src = doc_10k();

    c.bench_function("parse_10k", |b| {
        b.iter(|| parse(&src, "bench").expect("must parse"));
    });

    let module = parse(&src, "bench").expect("must parse");
    c.bench_function("eval_10k", |b| {
        let engine = Engine::new();
        b.iter(|| engine.evaluate(&module).expect("must evaluate"));
    });

    // One leaf changes to a value no other chain holds, so
    // invalidation travels the full depth of its chain (an edit
    // aliasing a sibling chain would mesh with the memo and
    // measure almost no recompute — that case is pinned by the
    // `edit_aliasing_an_existing_value_serves_shared_memo` test).
    let mut edited = src.clone();
    let from = "k0 = 0\n";
    let to = "k0 = 100000\n";
    let pos = edited.find(from).expect("leaf");
    edited.replace_range(pos..pos + from.len(), to);
    let module2 = parse(&edited, "bench").expect("must parse");
    c.bench_function("incremental_leaf_edit", |b| {
        b.iter_batched(
            || {
                let engine = Engine::new();
                let mut memo = IncrementalMemo::default();
                engine
                    .evaluate_incremental(&module, &mut memo)
                    .expect("must evaluate");
                (engine, memo)
            },
            |(engine, mut memo)| {
                engine
                    .evaluate_incremental(&module2, &mut memo)
                    .expect("must evaluate")
            },
            BatchSize::SmallInput,
        );
    });

    c.bench_function("eval_small", |b| {
        let module = parse("a = 1\nb = a\n", "bench").expect("must parse");
        let engine = Engine::new();
        b.iter(|| engine.evaluate(&module).expect("must evaluate"));
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
