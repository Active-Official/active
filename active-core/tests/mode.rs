//! Mode flag (PROPOSAL §0.2, CHECKLIST Phase M).
//!
//! Config is the default and stays 1.0-compatible; Data Mode is
//! explicit opt-in via `active_mode = "data"` and adds progressive
//! budget warnings while hard ceilings never move.

use active_core::{parse, Engine};

#[test]
fn patches_and_profiles_cannot_rewrite_document_metadata() {
    // The exported mode marker must match the mode the host resolved.
    for src in [
        "active_mode = \"config\"\n@patch(\"active_mode\", \"data\")\nx = 1\n",
        "active_mode = \"config\"\n@remove(\"active_mode\")\nx = 1\n",
        "active_capabilities = [\"check\"]\n@patch(\"active_capabilities\", [\"eval\"])\nx = 1\n",
        "active_mode = \"config\"\nprofile \"p\" { active_mode = \"data\" }\nactive_profile = \"p\"\nx = 1\n",
        "active_profile = \"a\"\nprofile \"a\" { x = 1 }\nprofile \"b\" { x = 2 }\n@patch(\"active_profile\", \"b\")\n",
    ] {
        let module = parse(src, "mode-metadata").expect("must parse");
        let err = Engine::new()
            .evaluate(&module)
            .expect_err("metadata rewrite must fail closed");
        assert_eq!(err.code(), "E019", "{src}: {err}");
    }
}

fn mode_of(src: &str) -> active_core::Mode {
    parse(src, "mode").expect("must parse").mode
}

#[test]
fn default_mode_is_config() {
    assert_eq!(mode_of("port = 8080\n"), active_core::Mode::Config);
}

#[test]
fn data_marker_selects_data_mode() {
    assert_eq!(
        mode_of("active_mode = \"data\"\nport = 8080\n"),
        active_core::Mode::Data
    );
    assert_eq!(
        mode_of("active_mode = \"config\"\nport = 8080\n"),
        active_core::Mode::Config
    );
}

#[test]
fn unknown_mode_is_e024() {
    let (_, errors) = active_core::parse_recovering(
        "active_mode = \"turbo\"\nport = 8080\n",
        "mode",
        active_core::ParseOptions::strict(),
    );
    assert!(errors.iter().any(|d| d.code == "E024"), "{errors:?}");
}

#[test]
fn non_string_mode_is_e024() {
    let (_, errors) = active_core::parse_recovering(
        "active_mode = 42\nport = 8080\n",
        "mode",
        active_core::ParseOptions::strict(),
    );
    assert!(errors.iter().any(|d| d.code == "E024"), "{errors:?}");
}

#[test]
fn marker_key_stays_visible_data() {
    // The opt-in is self-describing: the marker evaluates as data.
    let module = parse("active_mode = \"data\"\nport = 8080\n", "mode").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_json(&value);
    assert_eq!(json["active_mode"], "data");
    assert_eq!(json["port"], 8080);
}

#[test]
fn included_marker_neither_sets_nor_breaks() {
    // Includes cannot set the mode: a merged active_mode from an
    // include is ignored entirely (no E024, no flip). The outer
    // document stays Config.
    let dir = std::env::temp_dir().join("active-mode-include");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("inc.active"), "active_mode = \"data\"\nx = 1\n").expect("write inc");
    let main = dir.join("main.active");
    std::fs::write(&main, "include \"inc.active\"\nport = 2\n").expect("write main");
    let module = active_core::parse_file(&main, active_core::ParseOptions::default())
        .unwrap_or_else(|diags| panic!("must parse: {diags:?}"));
    assert_eq!(module.mode, active_core::Mode::Config);
}

#[test]
fn nested_active_mode_stays_an_ordinary_key() {
    // The root-only exemption is precise: a nested `active_mode` in a
    // closed table fails like any other unexpected key — never
    // silently exempt, never a mode flip.
    let module = parse(
        "schema = { outer = Table { host = String, closed = true } }\n[outer]\nactive_mode = \"data\"\nhost = \"x\"\n",
        "mode",
    )
    .expect("must parse");
    assert_eq!(module.mode, active_core::Mode::Config);
    let mut engine = Engine::new();
    engine.resilient = false;
    let err = engine
        .evaluate(&module)
        .expect_err("nested marker must fail");
    assert!(
        err.code() == "T005" || err.code() == "T002",
        "unexpected code: {}",
        err.code()
    );
}

#[test]
fn closed_tables_exempt_the_marker() {
    // The mode marker is metadata, not data: it never trips T005.
    let module = parse(
        "schema = { closed = true, host = String }\nactive_mode = \"data\"\nhost = \"x\"\n",
        "mode",
    )
    .expect("must parse");
    let mut engine = Engine::new();
    engine.resilient = false;
    let (_, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(
        !diags.iter().any(|d| d.code == "T005"),
        "marker must not trip closed tables: {diags:?}"
    );
}

#[test]
fn config_documents_behave_identically() {
    // M.003: no marker, no behavior change — Data and Config produce
    // the same value tree for the same document body.
    let body = "port = 8080\nhost = \"x\"\n";
    let plain = parse(body, "mode").expect("must parse");
    let (v_plain, _) = Engine::new().evaluate(&plain).expect("must evaluate");
    let marked_src = format!("active_mode = \"data\"\n{body}");
    let marked = parse(&marked_src, "mode").expect("must parse");
    assert_eq!(marked.mode, active_core::Mode::Data);
    let (v_marked, _) = Engine::new().evaluate(&marked).expect("must evaluate");
    let mut expected = active_core::convert::value_to_json(&v_plain);
    expected["active_mode"] = serde_json::json!("data");
    assert_eq!(active_core::convert::value_to_json(&v_marked), expected);
}

#[test]
fn data_mode_warns_progressively_but_ceilings_hold() {
    // At 80% of max_nodes the Data document warns (W013) where the
    // Config document stays silent; past 100% both trip X001.
    let mut src = String::from("active_mode = \"data\"\n");
    for i in 0..90 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let module = parse(&src, "mode").expect("must parse");
    let mut engine = Engine::new();
    engine.budget = active_core::Budget {
        max_nodes: 100,
        ..active_core::Budget::default()
    };
    let (_, diags) = engine
        .evaluate(&module)
        .expect("must evaluate under ceiling");
    assert!(
        diags.iter().any(|d| d.code == "W013"),
        "expected progressive warning, got {diags:?}"
    );

    let mut src = String::from("active_mode = \"data\"\n");
    for i in 0..200 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let module = parse(&src, "mode").expect("must parse");
    let mut engine = Engine::new();
    engine.budget = active_core::Budget {
        max_nodes: 100,
        ..active_core::Budget::default()
    };
    let err = engine.evaluate(&module).expect_err("must trip ceiling");
    assert_eq!(err.code(), "X001");
}

#[test]
fn data_mode_warns_on_depth_value_and_total() {
    // W013 covers every countable budget; below 80% it stays silent.
    let mut engine = Engine::new();
    engine.budget = active_core::Budget {
        max_depth: 6,
        max_value_size: 1_000_000,
        max_total_value_size: 1_000_000,
        ..active_core::Budget::default()
    };
    // Deep nesting: depth bound 5 of 6 → W013.
    let mut src = String::from("active_mode = \"data\"\n");
    for i in 0..5 {
        src.push_str(&format!("t{i} = {{\n"));
    }
    src.push_str("v = 1\n");
    for _ in 0..5 {
        src.push_str("}\n");
    }
    let module = parse(&src, "mode").expect("must parse");
    let (_, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(
        diags
            .iter()
            .any(|d| d.code == "W013" && d.message.contains("depth")),
        "expected depth warning, got {diags:?}"
    );
    // Small document, roomy budgets: silence.
    let module = parse("active_mode = \"data\"\nport = 1\n", "mode").expect("must parse");
    let (_, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(
        !diags.iter().any(|d| d.code == "W013"),
        "no warning far below ceilings, got {diags:?}"
    );
}

#[test]
fn memo_separates_modes() {
    // The token includes the mode: a Config evaluation must never
    // serve a Data memo entry (which would wrongly carry W013).
    // Literals stay unique per key so within-eval structural sharing
    // cannot be mistaken for a cross-mode hit.
    let mut src = String::from("active_mode = \"data\"\n");
    for i in 0..90 {
        src.push_str(&format!("k{i} = {}\n", 1000 + i));
    }
    let module = parse(&src, "mode").expect("must parse");
    let mut engine = Engine::new();
    engine.budget = active_core::Budget {
        max_nodes: 110,
        ..active_core::Budget::default()
    };
    let mut memo = active_core::IncrementalMemo::default();
    let (v1, d1) = engine
        .evaluate_incremental(&module, &mut memo)
        .expect("data eval");
    assert!(d1.iter().any(|d| d.code == "W013"), "{d1:?}");
    let after_data = memo.last_memo_hits;
    // Same memo, Config document: the mode difference clears the
    // store — no W013 leaks across modes, and values stay right.
    let mut plain_src = String::from("port = 0\n");
    for i in 0..90 {
        plain_src.push_str(&format!("k{i} = {}\n", 2000 + i));
    }
    let plain = parse(&plain_src, "mode").expect("must parse");
    let (v2, d2) = engine
        .evaluate_incremental(&plain, &mut memo)
        .expect("config eval");
    assert!(
        !d2.iter().any(|d| d.code == "W013"),
        "config eval over a Data memo must stay silent: {d2:?}"
    );
    assert_eq!(memo.last_memo_hits, after_data);
    assert_eq!(
        active_core::convert::value_to_json(&v2)["k89"],
        serde_json::json!(2089)
    );
    let _ = v1;
}
