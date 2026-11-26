//! Conformance reference runner (C-003): executes conformance/manifest.toml
//! against active-core. Every non-Rust embedding runs the same manifest
//! from the same vectors; codes compare strictly, spans stay advisory
//! (G-017). Hermetic empty ambient: env cases must carry their own defaults.

use active_core::{parse_file, Engine, EvalError, ParseOptions};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("conformance")
}

fn hermetic_engine() -> Engine {
    // This suite is a correctness gate, not the performance gate: a
    // loaded macOS runner tripped the shipped 100 ms wall budget on its
    // first evaluation (a false E029, seen as "0001-service-happy: No
    // such file or directory" because the fatal path looked for a codes
    // file). The budget itself is exercised by the budget suites.
    let mut engine = Engine::new().with_env(HashMap::new());
    engine.budget.max_wall_ms = 60_000;
    engine
}

fn codes_of(diags: &[active_core::Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.clone()).collect()
}

/// Committed-expectation view: sorted, deduplicated. Recovery may repeat a
/// code legitimately (one W011 per duplicate key, up to the record budget);
/// the ordered per-item equality across embeddings is the differential
/// gate's check.
fn norm_codes(codes: &[String]) -> Vec<String> {
    let mut out = codes.to_vec();
    out.sort();
    out.dedup();
    out
}

#[derive(Debug)]
struct Case {
    id: String,
    file: String,
    kind: String,
    profile: Option<String>,
    /// L-P05: evaluate under `--pure` for this case.
    pure: bool,
    /// L-P40: evaluate the cone of this document path (`Demand::Cone`).
    /// `None` keeps the default `Demand::All`.
    path: Option<String>,
    /// L-P40: per-case budget overrides, applied field by field over the
    /// hermetic engine (which holds the 60 s wall budget). A committed
    /// fixture is too small to reach the shipped ceilings, so an
    /// open-ended case needs a tight limit to be a test that can fail.
    budget: Option<BudgetPatch>,
}

/// L-P40: the `budget` inline table of a case. Every field is optional and
/// patches one `active_core::Budget` field; absent fields keep the
/// engine's value. The reference runner is the only consumer; the L-P37
/// dump and the oracle ignore `budget` so they stay in agreement.
#[derive(Debug, Default)]
struct BudgetPatch {
    max_nodes: Option<u64>,
    max_depth: Option<u64>,
    max_array_len: Option<u64>,
    max_keys: Option<u64>,
    max_wall_ms: Option<u64>,
    max_value_size: Option<u64>,
    max_total_value_size: Option<u64>,
}

impl BudgetPatch {
    fn apply(&self, budget: &mut active_core::Budget) {
        macro_rules! patch {
            ($($field:ident),*) => {
                $(if let Some(value) = self.$field {
                    budget.$field = value;
                })*
            };
        }
        patch!(
            max_nodes,
            max_depth,
            max_array_len,
            max_keys,
            max_wall_ms,
            max_value_size,
            max_total_value_size
        );
    }

    fn from_table(table: &toml::value::Table) -> Self {
        let get = |key: &str| {
            table
                .get(key)
                .and_then(|v| v.as_integer())
                .map(|i| i as u64)
        };
        Self {
            max_nodes: get("max_nodes"),
            max_depth: get("max_depth"),
            max_array_len: get("max_array_len"),
            max_keys: get("max_keys"),
            max_wall_ms: get("max_wall_ms"),
            max_value_size: get("max_value_size"),
            max_total_value_size: get("max_total_value_size"),
        }
    }
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
                pure: t.get("pure").and_then(|v| v.as_bool()).unwrap_or(false),
                path: t.get("path").and_then(|v| v.as_str()).map(str::to_string),
                budget: t
                    .get("budget")
                    .and_then(|v| v.as_table())
                    .map(BudgetPatch::from_table),
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

fn span_json(span: Option<&active_core::diag::Span>) -> serde_json::Value {
    match span {
        Some(s) => serde_json::json!({
            "line": s.line,
            "column": s.column,
            "end_line": s.end_line,
            "end_column": s.end_column,
        }),
        None => serde_json::Value::Null,
    }
}

fn records_json(diags: &[active_core::Diagnostic]) -> serde_json::Value {
    serde_json::json!(diags
        .iter()
        .map(|d| serde_json::json!({
            "code": d.code,
            "severity": format!("{:?}", d.severity).to_lowercase(),
            "path": d.path,
            "span": span_json(d.span.as_ref()),
        }))
        .collect::<Vec<_>>())
}

/// L-P37: the ordered corpus projection the reference differential uses
/// instead of the deduplicated committed sidecars. Written only when
/// `ACTIVE_CORPUS_DUMP` names a file; the gate's own assertions are
/// unchanged.
///
/// This dump deliberately ignores the per-case `path` and `budget` fields
/// (it evaluates every case with `Demand::All` and the default budget), so
/// its record matches the reference oracle, which reads the same manifest
/// and also stays full-demand and default-budget. Honoring `path` here
/// would record a cone projection while the oracle records the full
/// document, and honoring `budget` would trip a case the oracle passes —
/// either would fail the L-P37 gate.
#[test]
fn l37_corpus_dump() {
    let Ok(path) = std::env::var("ACTIVE_CORPUS_DUMP") else {
        return;
    };
    let mut file = std::fs::File::create(path).expect("corpus dump writable");
    for case in load_manifest() {
        let mut engine = hermetic_engine();
        engine.profile_override = case.profile.clone();
        engine.pure = case.pure;
        let record = match parse_file(&root().join(&case.file), ParseOptions::default()) {
            Err(diags) => corpus_record(&case.id, false, &diags, None),
            Ok(module) => match engine.evaluate_with_seeds(&module) {
                Ok((value, diags, seeds)) => {
                    let profile =
                        active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds);
                    let redacted = active_core::convert::redact_value(&profile, &value);
                    corpus_record(
                        &case.id,
                        true,
                        &diags,
                        Some((
                            active_core::provenance::content_hash(redacted.as_value()),
                            active_core::convert::value_to_json(redacted.as_value()),
                        )),
                    )
                }
                Err(EvalError::ValidationFailed { diagnostics }) => {
                    corpus_record(&case.id, false, &diagnostics, None)
                }
                Err(error) => {
                    let diag = active_core::Diagnostic {
                        code: error.code().to_string(),
                        severity: active_core::Severity::Error,
                        message: String::new(),
                        path: None,
                        span: error.span().cloned(),
                        repair: None,
                        confidence: 1.0,
                        file: None,
                    };
                    corpus_record(&case.id, false, std::slice::from_ref(&diag), None)
                }
            },
        };
        writeln!(file, "{record}").expect("corpus dump written");
    }
}

fn corpus_record(
    id: &str,
    ok: bool,
    diags: &[active_core::Diagnostic],
    extra: Option<(String, serde_json::Value)>,
) -> serde_json::Value {
    let codes = diags.iter().map(|d| d.code.clone()).collect::<Vec<_>>();
    let messages = diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>();
    let mut record = serde_json::json!({
        "id": id,
        "ok": ok,
        "codes": codes,
        "messages": messages,
        "diag_records": records_json(diags),
    });
    match extra {
        Some((hash, value)) => {
            record["content_hash"] = serde_json::Value::String(hash);
            record["value"] = value;
        }
        None => {
            record["content_hash"] = serde_json::Value::Null;
            record["value"] = serde_json::Value::Null;
        }
    }
    record
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
                let got: Vec<String> = diags.iter().map(|d| d.code.clone()).collect();
                assert_eq!(
                    norm_codes(&got),
                    norm_codes(&expected_codes(&case.id)),
                    "{}: parse-refused codes",
                    case.id
                );
                assert_eq!(case.kind, "refused", "{}: kind", case.id);
                continue;
            }
        };
        let mut engine = hermetic_engine();
        engine.profile_override = case.profile.clone();
        engine.pure = case.pure;
        if let Some(patch) = &case.budget {
            patch.apply(&mut engine.budget);
        }
        // L-P40: a `path` case evaluates the cone of that document path;
        // otherwise the default whole-document demand holds. Both arms
        // share the `(value, diagnostics)` shape, so the assertions below
        // are demand-agnostic.
        let result = match &case.path {
            Some(path) => engine
                .evaluate_demanded(&module, &active_core::Demand::Cone(path.clone()))
                .map(|outcome| (outcome.value, outcome.diagnostics)),
            None => engine.evaluate(&module),
        };
        match result {
            Ok((value, diags)) => {
                let got_codes = codes_of(&diags);
                if case.kind == "refused" {
                    // DEC-004 (LANG-G-002): the core suite still evaluates
                    // resiliently - it is the reference diagnostic source - but
                    // the embeddings refuse an error-severity diagnostic, so a
                    // refused case has no value in any embedding and carries
                    // only its codes.
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
                // Committed expectations are the SET view (sorted, deduped),
                // matching the harness's norm_codes rule: recovery may
                // legitimately repeat a code (depth on open+close, one W011
                // per duplicate key up to the record budget). Ordered
                // per-item equality across embeddings is the differential
                // gate's job (conformance/harness/diff_vectors.py).
                assert_eq!(
                    norm_codes(&got_codes),
                    norm_codes(&want_codes),
                    "{}: code set diverged (spans advisory, codes strict)",
                    case.id
                );
                let json = active_core::convert::value_to_pretty_json(&value);
                let got: serde_json::Value = serde_json::from_str(&json).expect("valid json");
                assert_eq!(got, expected_json(&case.id), "{}: value diverged", case.id);
            }
            Err(EvalError::ValidationFailed { diagnostics }) => {
                let got: Vec<String> = diagnostics.iter().map(|d| d.code.clone()).collect();
                assert_eq!(
                    norm_codes(&got),
                    norm_codes(&expected_codes(&case.id)),
                    "{}: refused codes",
                    case.id
                );
                assert_eq!(case.kind, "refused", "{}: kind", case.id);
            }
            Err(e) => {
                // A clean case that fails fatally is a test failure with
                // its own name, not a missing-codes-file mystery.
                assert_ne!(
                    case.kind,
                    "clean",
                    "{}: a clean case failed fatally ({})",
                    case.id,
                    e.code()
                );
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
