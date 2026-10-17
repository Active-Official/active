//! Documentation-site sample tests (C-060/C-061).
//!
//! Every `.active` file under `docs-site/src/` executes on the real
//! shipped path (`parse` → `Engine::evaluate`) with per-file value
//! assertions, and every ```active fence in the tutorial chapters
//! evaluates too: book samples that fail would teach lies, so the
//! book cannot rot. The engine runs on an EMPTY env snapshot —
//! samples must be deterministic by their `?:` defaults, never by
//! the ambient machine.

use active_core::{parse, Engine};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn site_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs-site")
        .join("src")
}

fn hermetic_engine() -> Engine {
    // Empty snapshot: ambient variables cannot leak into samples
    // (twelve-factor relies on defaults; Engine::new() alone would
    // snapshot the real ambient and flake).
    Engine::new().with_env(HashMap::new())
}

fn site_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![site_root()];
    while let Some(dir) = dirs.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|x| x == "active") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn eval_file(path: &Path) -> serde_json::Value {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let module = parse(&src, &name).unwrap_or_else(|diags| panic!("{name}: {diags:?}"));
    let (value, diags) = hermetic_engine()
        .evaluate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert!(diags.is_empty(), "{name}: {diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    serde_json::from_str(&json).expect("valid json")
}

/// Every ```active fence in the tutorial chapters evaluates cleanly
/// (same hermetic engine). Fences are full documents by author
/// contract — a fragment would fail here, forcing it into a .active
/// file or out of the prose.
fn tutorial_fences() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let dir = site_root().join("tutorial");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("tutorial dir: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).expect("readable");
        let mut rest = text.as_str();
        while let Some(start) = rest.find("```active") {
            let after_open = &rest[start + "```active".len()..];
            let Some(end) = after_open.find("```") else {
                panic!("{name}: unclosed ```active fence");
            };
            // Info strings (```active,skip) opt out: only exact ```active
            // fences execute. Every executable fence is a full document.
            if after_open.starts_with('\n') || after_open.starts_with("\r\n") {
                let code = after_open[..end].trim().to_string();
                out.push((format!("{name}#{}", out.len()), code));
            }
            rest = &after_open[end + "```".len()..];
        }
    }
    out
}

#[test]
fn every_site_sample_evaluates_cleanly() {
    let files = site_files();
    assert!(
        files.len() >= 8,
        "expected the cookbook + migration samples"
    );
    for path in &files {
        eval_file(path);
    }
}

#[test]
fn every_tutorial_fence_evaluates_cleanly() {
    let fences = tutorial_fences();
    assert!(fences.len() >= 6, "tutorial prose must stay executable");
    for (label, code) in &fences {
        let module = parse(code, label).unwrap_or_else(|diags| panic!("{label}: {diags:?}"));
        let (_, diags) = hermetic_engine()
            .evaluate(&module)
            .unwrap_or_else(|e| panic!("{label}: {e:?}"));
        assert!(diags.is_empty(), "{label}: {diags:?}");
    }
}

#[test]
fn multi_region_profile_override() {
    // The book shows active_profile; the override path (CLI
    // --profile) must agree: us-east values, not eu-west.
    let path = site_root().join("cookbook").join("multi-region.active");
    let src = std::fs::read_to_string(&path).expect("readable");
    let module = parse(&src, "multi-region").expect("must parse");
    let mut engine = hermetic_engine();
    engine.profile_override = Some("us-east".to_string());
    let (value, diags) = engine.evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(v["server"]["host"], "us-east.example.com");
    assert_eq!(v["limits"]["rps"], 1000);
}

#[test]
fn kubernetes_sample() {
    let v = eval_file(&site_root().join("cookbook").join("kubernetes.active"));
    assert_eq!(v["replicas"], 3);
    assert_eq!(v["resources"]["cpu_millicores"], 500);
    assert_eq!(v["resources"]["memory"], 1073741824);
    assert_eq!(v["health"]["readiness"]["ms"], 30000);
}

#[test]
fn feature_flags_sample() {
    let v = eval_file(&site_root().join("cookbook").join("feature-flags.active"));
    assert_eq!(v["flags"]["rollout_pct"], 25);
    assert_eq!(v["flags"]["dark_launch"], false);
}

#[test]
fn multi_region_selects_eu_west() {
    let v = eval_file(&site_root().join("cookbook").join("multi-region.active"));
    assert_eq!(v["server"]["host"], "eu-west.example.com");
    assert_eq!(v["limits"]["rps"], 500);
}

#[test]
fn ai_prompts_evaluate_to_strings() {
    let v = eval_file(&site_root().join("cookbook").join("ai-prompts.active"));
    assert_eq!(v["entry"], "v1.0.0 ready");
    assert!(v["prompt"]
        .as_str()
        .is_some_and(|s| s.contains("release assistant")));
    assert!(v["policy"]
        .as_str()
        .is_some_and(|s| s.contains("Never invent")));
    assert!(v["changelog"]
        .as_str()
        .is_some_and(|s| s.contains("## Release 1.0.0")));
}

#[test]
fn twelve_factor_uses_defaults() {
    let v = eval_file(&site_root().join("cookbook").join("twelve-factor.active"));
    assert_eq!(v["server"]["host"], "localhost");
    assert_eq!(v["logging"]["level"], "info");
    assert_eq!(v["database"]["pool"], 5);
    assert!(v["database"]["url"]
        .as_str()
        .is_some_and(|s| s.contains("postgres")));
}

#[test]
fn migration_samples() {
    for name in ["toml.active", "yaml.active", "json.active"] {
        let v = eval_file(&site_root().join("migration").join(name));
        assert_eq!(v["server"]["port"], 8080, "{name}");
    }
}

#[test]
fn terraform_vars_sample() {
    let v = eval_file(&site_root().join("cookbook").join("terraform-vars.active"));
    assert_eq!(v["region"], "eu-west-1");
    assert_eq!(v["instance_count"], 3);
    assert_eq!(v["instance_type"], "t3.small");
}

#[test]
fn ci_pipeline_sample() {
    let v = eval_file(&site_root().join("cookbook").join("ci-pipeline.active"));
    assert_eq!(v["build"]["steps"], 4);
    assert_eq!(v["matrix"][1]["os"], "windows-latest");
}
