//! Tooling-surface benchmarks: dependency graph export and value
//! inspection (QUALITY.002). Both run through the shipped pure
//! functions over a parsed/evaluated document, so a regression in the
//! tooling paths shows up next to eval.

use active_core::convert::inspect_path_with_profile;
use active_core::graph::to_graph_json;
use active_core::{parse, Engine};
use codspeed_criterion_compat::{criterion_group, criterion_main, Criterion};

fn document() -> String {
    let mut out = String::new();
    for t in 0..40 {
        out.push_str(&format!("[t{t}]\n"));
        out.push_str(&format!("k0 = {t}\n"));
        for k in 1..100 {
            out.push_str(&format!("k{k} = k{} + 1\n", k - 1));
        }
        out.push_str(&format!("label = \"table-{t}\"\n"));
    }
    out
}

fn bench_graph(c: &mut Criterion) {
    let module = parse(&document(), "bench").expect("must parse");
    c.bench_function("graph_json_4k", |b| {
        b.iter(|| to_graph_json(&module));
    });
}

fn bench_inspect(c: &mut Criterion) {
    let module = parse(&document(), "bench").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    c.bench_function("inspect_path_4k", |b| {
        b.iter(|| inspect_path_with_profile(&module, &value, "t7.k50", None).expect("known path"));
    });
}

criterion_group!(benches, bench_graph, bench_inspect);
criterion_main!(benches);
