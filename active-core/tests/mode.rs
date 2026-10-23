//! Mode flag (PROPOSAL §0.2, CHECKLIST Phase M).
//!
//! Config is the default and stays 1.0-compatible; Data Mode is
//! explicit opt-in via `active_mode = "data"` and adds progressive
//! budget warnings while hard ceilings never move.

use active_core::{parse, Engine};

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
