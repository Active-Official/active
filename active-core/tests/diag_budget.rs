//! Shared diagnostic-budget regression tests (LANG-A-001, LANG-A-901).
//!
//! Errors and healing diagnostics share ONE 1024-record budget per parse:
//! a comma-less array, a duplicate-key pile, and include breadth are the
//! same flood through different doors. When the budget trips, the
//! retained set stays within the bound and carries exactly one visible
//! truncation notice (W014) — never a silent drop.
//!
//! Every test drives the shipped parser/engine entry points.

use active_core::{parse, Engine};

/// Total records the parser retained across both channels.
fn retained(src: &str) -> (Vec<String>, usize) {
    let module = parse(src, "budget").expect("recovering parse must succeed");
    let mut codes: Vec<String> = module.healing.iter().map(|d| d.code.clone()).collect();
    let total = module.healing.len();
    codes.sort();
    (codes, total)
}

#[test]
fn comma_less_array_healing_is_bounded() {
    // 3000 elements with no separators: one H003 per element before the
    // fix (2999 records through Engine::evaluate).
    let mut src = String::from("x = [");
    for i in 0..3000 {
        if i > 0 {
            src.push(' ');
        }
        src.push_str(&i.to_string());
    }
    src.push_str("]\n");
    let (codes, total) = retained(&src);
    assert!(total <= 1024, "healing records bounded, got {total}");
    assert_eq!(
        codes.iter().filter(|c| *c == "W014").count(),
        1,
        "exactly one truncation notice"
    );
    assert!(codes.iter().any(|c| c == "H003"), "genuine healing kept");
}

#[test]
fn duplicate_key_healing_is_bounded() {
    // 3000 redefinitions of the same key: 2999 W011 before the fix.
    let mut src = String::new();
    for i in 0..3000 {
        src.push_str(&format!("k = {i}\n"));
    }
    let (codes, total) = retained(&src);
    assert!(total <= 1024, "healing records bounded, got {total}");
    assert_eq!(
        codes.iter().filter(|c| *c == "W014").count(),
        1,
        "exactly one truncation notice"
    );
    assert!(codes.iter().any(|c| c == "W011"), "genuine warning kept");
}

#[test]
fn truncation_notice_is_last_and_deterministic() {
    let mut src = String::new();
    for i in 0..1200 {
        src.push_str(&format!("k = {i}\n"));
    }
    let first = parse(&src, "budget").expect("parse");
    let second = parse(&src, "budget").expect("parse");
    let a: Vec<String> = first
        .healing
        .iter()
        .map(|d| format!("{}@{:?}", d.code, d.span))
        .collect();
    let b: Vec<String> = second
        .healing
        .iter()
        .map(|d| format!("{}@{:?}", d.code, d.span))
        .collect();
    assert_eq!(a, b, "same input yields the same retained set");
    assert_eq!(
        first.healing.last().map(|d| d.code.as_str()),
        Some("W014"),
        "the notice closes the retained list"
    );
}

#[test]
fn include_healing_breadth_is_bounded_through_the_shared_choke_point() {
    // LANG-A-901: many includes each carrying one unclosed string merge
    // through the shared healing choke point. The include RESOLUTION budget
    // (LANG-C-002, 256) bounds breadth before the record budget matters, so
    // this fixture stays inside it and asserts the merge path itself; the
    // 1024-record cap is exercised by the duplicate-key fixture above.
    let dir = std::env::temp_dir().join(format!("active_heal_breadth_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut main = String::from("v = 1\n");
    for i in 0..240 {
        let name = format!("h{i}.active");
        std::fs::write(dir.join(&name), format!("s{i} = \"unclosed-{i}\n")).expect("write");
        main.push_str(&format!("include \"{name}\"\n"));
    }
    let path = dir.join("main.active");
    std::fs::write(&path, main).expect("write main");
    let module = active_core::parse_file(&path, active_core::ParseOptions::default())
        .expect("recovering parse must succeed inside the include budget");
    let total = module.healing.len();
    assert!(total <= 1024, "include breadth bounded, got {total}");
    assert!(
        module.healing.iter().any(|d| d.code == "H001"),
        "genuine include healing kept"
    );
    assert_eq!(
        module.healing.iter().filter(|d| d.code == "W014").count(),
        0,
        "240 records is under the 1024 budget, so nothing is truncated"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn engine_evaluation_surfaces_the_bounded_set() {
    // The CLI path adds `module.healing` to evaluation diagnostics
    // verbatim; the bounded set must reach that surface unchanged.
    let mut src = String::new();
    for i in 0..3000 {
        src.push_str(&format!("k = {i}\n"));
    }
    let module = parse(&src, "budget").expect("parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("evaluate");
    assert!(diags.len() <= 1024, "evaluated diagnostics bounded");
    assert!(diags.iter().any(|d| d.code == "W014"));
}
