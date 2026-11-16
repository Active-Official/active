//! Evaluator benchmarks (C-055, spec/performance.md §1).
//!
//! Continuous tracking runs through the CodSpeed CI plugin; local
//! runs use `cargo bench`. The 10k document is generated in code
//! (50 tables × 200 chained keys): wide enough for map behavior,
//! chained enough for topological depth. LANG-K-004 adds the wide-flat,
//! >100k-path, large-literal, and heap-peak shapes the suite lacked.

use active_core::{parse, Engine, IncrementalMemo};
use codspeed_criterion_compat::{criterion_group, criterion_main, BatchSize, Criterion};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Heap accounting for the memory bench: live bytes and the high-water
/// mark since the last reset. Portable, unlike RSS probing (LANG-K-004).
struct CountingAlloc;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn note_alloc(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc(layout.size());
        // SAFETY: forwarding to the system allocator with the same layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: pointer and layout came from `alloc` above.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            note_alloc(new_size - layout.size());
        } else {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        // SAFETY: pointer and layout came from this allocator.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

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

/// 10,000 independent root keys: the wide-flat shape (no chains).
fn doc_wide_10k() -> String {
    let mut out = String::new();
    for i in 0..10_000 {
        out.push_str(&format!("key{i} = {i}\n"));
    }
    out
}

/// 100,098 paths, zero secrets: the shape where LANG-K-001 silently
/// replaced 499 benign values with `***SECRET***`.
fn doc_100k_paths() -> String {
    let mut out = String::new();
    for t in 0..498 {
        out.push_str(&format!("[t{t}]\n"));
        out.push_str(&format!("k0 = {t}\n"));
        for k in 1..200 {
            out.push_str(&format!("k{k} = k{} + 1\n", k - 1));
        }
    }
    out
}

/// ~10,000 keys where every tenth carries a 1 KiB string literal.
fn doc_large_literals() -> String {
    let literal = "x".repeat(1024);
    let mut out = String::new();
    for i in 0..10_000 {
        if i % 10 == 0 {
            out.push_str(&format!("key{i} = \"{literal}\"\n"));
        } else {
            out.push_str(&format!("key{i} = {i}\n"));
        }
    }
    out
}

/// Benches measure time themselves (criterion, CodSpeed
/// simulation); the production wall-clock backstop stays off so
/// instrumentation slowdown (valgrind runs 10–50× slower) can never
/// abort a measurement. Count/depth/size budgets keep their shipped
/// defaults — they are deterministic counts, unaffected by slowdown.
fn bench_engine() -> Engine {
    let mut engine = Engine::new();
    // A zero field resolves to the shipped default (LANG-C-015), so the
    // explicit sentinel is the only way to switch the deadline off.
    engine.budget.max_wall_ms = active_core::eval::UNLIMITED;
    engine
}

fn criterion_benchmark(c: &mut Criterion) {
    let src = doc_10k();

    c.bench_function("parse_10k", |b| {
        b.iter(|| parse(&src, "bench").expect("must parse"));
    });

    let module = parse(&src, "bench").expect("must parse");
    c.bench_function("eval_10k", |b| {
        let engine = bench_engine();
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
                let engine = bench_engine();
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
        let engine = bench_engine();
        b.iter(|| engine.evaluate(&module).expect("must evaluate"));
    });

    // Hot-path extensions: formatter, linter, and JSON converter over
    // the same 10k document (spec/performance.md §1).
    let fmt_module = parse(&src, "bench").expect("must parse");
    c.bench_function("fmt_10k_brace", |b| {
        b.iter(|| active_core::fmt::format(&fmt_module, active_core::Style::Brace));
    });

    c.bench_function("lint_10k", |b| {
        b.iter(|| active_core::lint(&fmt_module));
    });

    let value = bench_engine()
        .evaluate(&fmt_module)
        .expect("must evaluate")
        .0;
    c.bench_function("convert_json_10k", |b| {
        b.iter(|| active_core::convert::value_to_pretty_json(&value));
    });

    // LANG-K-004: the wide-flat shape the suite did not measure.
    let wide = doc_wide_10k();
    let wide_module = parse(&wide, "bench").expect("must parse");
    c.bench_function("eval_wide_10k", |b| {
        let engine = bench_engine();
        b.iter(|| engine.evaluate(&wide_module).expect("must evaluate"));
    });

    // LANG-K-004/LANG-K-001: >100k paths, zero secrets. Every sample
    // asserts the fixed behaviour (no truncation, no `***SECRET***`).
    // The sample budget is small because one iteration is a whole
    // 100k-node evaluation plus its export.
    let big_module = parse(&doc_100k_paths(), "bench").expect("must parse");
    let mut big_engine = bench_engine();
    big_engine.budget = active_core::Budget::trusted();
    big_engine.budget.max_wall_ms = active_core::eval::UNLIMITED;
    let mut group = c.benchmark_group("scale");
    group.sample_size(10);
    group.bench_function("eval_100k_paths", |b| {
        b.iter(|| {
            let (value, _) = big_engine.evaluate(&big_module).expect("must evaluate");
            let seeds = active_core::secret::SecretRedactor::default();
            let profile =
                active_core::secret::SecretProfile::for_engine(&big_module, &big_engine, &seeds);
            assert!(!profile.truncated(), "LANG-K-001: path map truncated");
            let redacted = active_core::convert::redact_value(&profile, &value);
            let json = active_core::convert::value_to_pretty_json(&redacted);
            assert!(
                !json.contains("***SECRET***"),
                "LANG-K-001: benign value redacted"
            );
            json.len()
        });
    });
    group.finish();

    // LANG-K-004: re-hash cost when 1 KiB literals are in scope.
    let lits_module = parse(&doc_large_literals(), "bench").expect("must parse");
    c.bench_function("incremental_large_literals", |b| {
        b.iter_batched(
            || {
                let engine = bench_engine();
                let mut memo = IncrementalMemo::default();
                engine
                    .evaluate_incremental(&lits_module, &mut memo)
                    .expect("must evaluate");
                (engine, memo)
            },
            |(engine, mut memo)| {
                engine
                    .evaluate_incremental(&lits_module, &mut memo)
                    .expect("must evaluate")
            },
            BatchSize::SmallInput,
        );
    });

    // LANG-K-004: heap high-water mark of one 10k evaluation. The
    // allocator counter is portable; the bound catches a leak-shaped
    // regression without pretending to be RSS.
    c.bench_function("eval_10k_heap_peak", |b| {
        b.iter(|| {
            let baseline = LIVE.load(Ordering::Relaxed);
            PEAK.store(baseline, Ordering::Relaxed);
            let engine = bench_engine();
            let (value, _) = engine.evaluate(&module).expect("must evaluate");
            let json = active_core::convert::value_to_pretty_json(&value);
            let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
            assert!(
                peak < 64 * 1024 * 1024,
                "10k evaluation peaked at {peak} bytes of heap"
            );
            json.len()
        });
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
