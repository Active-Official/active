//! Conformance reference runner (C-003): executes conformance/manifest.toml
//! against active-core. Every non-Rust embedding runs the same manifest
//! from the same vectors; codes compare strictly, spans stay advisory
//! (G-017). Hermetic empty ambient: env cases must carry their own defaults.

use active_core::{parse_file, Engine, EvalError, ParseOptions};
use std::collections::HashMap;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("conformance")
}

fn hermetic_engine() -> Engine {
    Engine::new().with_env(HashMap::new())
}

fn codes_of(diags: &[active_core::Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.clone()).collect()
}

#[derive(Debug)]
struct Case {
    id: String,
    file: String,
    kind: String,
    profile: Option<String>,
}

fn load_manifest() -> Vec<Case> {
    let text = std::fs::read_to_string(root().join("manifest.toml")).expect("manifest readable");
    let manifest: toml::Value = text.parse().expect("manifest parses");
    manifest
        .get("cases")
        .and_then(|c| c.as_array())
        .expect("cases array")
        .iter()
        .map(|c| {
            let t = c.as_table().expect("case table");
            let str_field = |k: &str| {
                t.get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("case missing {k}"))
                    .to_string()
            };
            Case {
                id: str_field("id"),
                file: str_field("file"),
                kind: str_field("kind"),
                profile: t
                    .get("profile")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            }
        })
        .collect()
}

fn expected_json(id: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(root().join("expected").join(format!("{id}.json")))
        .unwrap_or_else(|e| panic!("{id}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn expected_codes(id: &str) -> Vec<String> {
    let text = std::fs::read_to_string(root().join("errors").join(format!("{id}.json")))
        .unwrap_or_else(|e| panic!("{id}: {e}"));
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{id}: {e}"));
    v.get("codes")
        .and_then(|c| c.as_array())
        .expect("codes array")
        .iter()
        .map(|c| c.as_str().expect("code string").to_string())
        .collect()
}

#[test]
fn conformance_manifest_is_green() {
    let cases = load_manifest();
    assert!(cases.len() >= 13, "tranche must hold: {}", cases.len());
    for case in &cases {
        let path = root().join(&case.file);
        let module = match parse_file(&path, ParseOptions::default()) {
            Ok(m) => m,
            // Parse-time refusals (E004 depth, E011 blocks): codes compare
            // as a set; recovery may emit the same code twice.
            Err(diags) => {
                let mut got: Vec<String> = diags.iter().map(|d| d.code.clone()).collect();
                got.sort();
                got.dedup();
                assert_eq!(
                    got,
                    expected_codes(&case.id),
                    "{}: parse-refused codes",
                    case.id
                );
                assert_eq!(case.kind, "refused", "{}: kind", case.id);
                continue;
            }
        };
        let mut engine = hermetic_engine();
        engine.profile_override = case.profile.clone();
        match engine.evaluate(&module) {
            Ok((value, diags)) => {
                let got_codes = codes_of(&diags);
                if case.kind == "refused" {
                    // Strict-collected violations (T003/T005/E004/E023) arrive
                    // as Ok-plus-diagnostics; the CLI turns them into exit
                    // codes, embeddings compare the code set.
                    assert_eq!(
                        got_codes,
                        expected_codes(&case.id),
                        "{}: refused codes",
                        case.id
                    );
                    assert!(!got_codes.is_empty(), "{}: refused needs codes", case.id);
                    continue;
                }
                let want_codes = if case.kind == "clean" {
                    Vec::new()
                } else {
                    expected_codes(&case.id)
                };
                assert_eq!(
                    got_codes, want_codes,
                    "{}: code set diverged (spans advisory, codes strict)",
                    case.id
                );
                let json = active_core::convert::value_to_pretty_json(&value);
                let got: serde_json::Value = serde_json::from_str(&json).expect("valid json");
                assert_eq!(got, expected_json(&case.id), "{}: value diverged", case.id);
            }
            Err(EvalError::ValidationFailed { diagnostics }) => {
                let got: Vec<String> = diagnostics.iter().map(|d| d.code.clone()).collect();
                assert_eq!(got, expected_codes(&case.id), "{}: refused codes", case.id);
                assert_eq!(case.kind, "refused", "{}: kind", case.id);
            }
            Err(e) => {
                assert_eq!(
                    vec![e.code().to_string()],
                    expected_codes(&case.id),
                    "{}: refused",
                    case.id
                );
                assert_eq!(case.kind, "refused", "{}: kind", case.id);
            }
        }
    }
}
