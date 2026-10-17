//! Research battery, batches 1–2 (docs/research-test-index.md).
//!
//! One test per index row, run ONE BY ONE in RB order. Each test drives
//! shipped entry points (`parse`/`parse_file` → `Engine::evaluate` → JSON
//! export) and asserts diagnostic CLASSES, never message text. Positive
//! control neighbours ride along so fixes cannot break the good case.
//! Rule: red test ⇒ fix the code (spec-gated), never the expectation.

use active_core::{parse, Engine};

fn eval_json(src: &str) -> serde_json::Value {
    let module = parse(src, "research").unwrap_or_else(|d| panic!("parse failed: {d:?}"));
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json")
}

fn codes_of(src: &str) -> Vec<String> {
    match parse(src, "research") {
        Ok(module) => match Engine::new().evaluate(&module) {
            Ok((_, diags)) => diags.into_iter().map(|d| d.code).collect(),
            Err(e) => vec![e.code().to_string()],
        },
        Err(diags) => diags.into_iter().map(|d| d.code).collect(),
    }
}

/// RB-001 — Colon forms are specified key-value pairs.
///
/// REFUSAL (spec-gated, DERIVED-0001 §A, P1-DS-015 §F, HIST-YAML-019 §G).
/// Research demands rejecting `value : 1`, but spec/dual-syntax.md:49 and
/// spec/grammar.md:192 define a same-line `identifier ":" value` pair as
/// legal. Norway protection comes from explicit typing (bare words still
/// reject), not from banning the colon glyph. All four spacing forms
/// evaluate to 1 here; a bodiless `key:` header still rejects (E008).
#[test]
fn rb_001_colon_forms_are_specified_pairs() {
    for src in ["value : 1\n", "value:1\n", "value :1\n", "value: 1\n"] {
        let v = eval_json(src);
        assert_eq!(v["value"], 1, "{src:?} must evaluate as key-value pair");
    }
    // Bodiless header still rejects.
    let codes = codes_of("value:\n");
    assert!(
        codes.iter().any(|c| c == "E008"),
        "E008 expected, got {codes:?}"
    );
    // Positive control: `=` assigns identically.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-002 — Duplicate keys warn, last wins (P1-TI-012 §C, HIST-YAML-004 §G).
/// Rejecting would break existing last-wins behavior; silence is not allowed.
#[test]
fn rb_002_duplicate_key_warns_last_wins() {
    let src = "[server]\nhost = \"a\"\nhost = \"b\"\n";
    let v = eval_json(src);
    assert_eq!(v["server"]["host"], "b");
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "W011"),
        "duplicate key must warn W011, got {codes:?}"
    );
    // Repeated table headers keep last-wins with the same notice.
    let v = eval_json("[server]\nhost = \"a\"\n[server]\nport = 1\n");
    assert_eq!(v["server"]["port"], 1);
    let codes = codes_of("[server]\nhost = \"a\"\n[server]\nport = 1\n");
    assert!(
        codes.iter().any(|c| c == "W011"),
        "redefined header must warn W011, got {codes:?}"
    );
    // Inline tables and `let` redefinitions behave the same.
    assert_eq!(eval_json("x = {a = 1, a = 2}\n")["x"]["a"], 2);
    let codes = codes_of("x = {a = 1, a = 2}\n");
    assert!(
        codes.iter().any(|c| c == "W011"),
        "inline dup must warn W011, got {codes:?}"
    );
    assert_eq!(eval_json("let x = 1\nlet x = 2\ny = x\n")["y"], 2);
    let codes = codes_of("let x = 1\nlet x = 2\ny = x\n");
    assert!(
        codes.iter().any(|c| c == "W011"),
        "redefined let must warn W011, got {codes:?}"
    );
}

/// RB-003 — BOM skipped (HIST-YAML-021 §G).
/// Leading U+FEFF is not content; JSON RFC policy: parsers may ignore it.
#[test]
fn rb_003_bom_skipped() {
    let v = eval_json("\u{FEFF}value = 1\n");
    assert_eq!(v["value"], 1);
}

/// RB-004 — Trailing comma healed (P1-ST-005 §D, HIST-YAML-022 §G).
/// H002 repair fires; values intact.
#[test]
fn rb_004_trailing_comma_healed() {
    let src = "value = [1, 2,]\n";
    let v = eval_json(src);
    assert_eq!(v["value"], serde_json::json!([1, 2]));
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "H002"),
        "H002 expected, got {codes:?}"
    );
}

/// RB-005 — Secret interpolation refused S004 (DERIVED-0008 §A).
#[test]
fn rb_005_secret_interpolation_refused() {
    let codes =
        codes_of("password = secret(\"s3cr3t-rb005\")\nurl = \"https://x/?t=${password}\"\n");
    assert!(
        codes.iter().any(|c| c == "S004"),
        "S004 expected, got {codes:?}"
    );
    // Positive control: plain interpolation works.
    assert_eq!(
        eval_json("let name = \"world\"\nmsg = \"hello ${name}\"\n")["msg"],
        "hello world"
    );
}

/// RB-006 — Secrets absent from diagnostics (DERIVED-0009 §A, P0-008 §B).
#[test]
fn rb_006_secret_redaction_in_diagnostics() {
    let raw = "super-secret-rb006";
    let src = format!("password = secret(\"{raw}\")\nbad = password + 1\n");
    let diags = parse(&src, "research")
        .map(|m| {
            Engine::new()
                .evaluate(&m)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        })
        .unwrap_or_else(|ds| format!("{ds:?}"));
    assert!(!diags.contains(raw), "leak in diagnostics: {diags}");
}

/// RB-007 — Env empty vs absent (DERIVED-0007 §A).
/// Present-but-empty wins; absent falls back. Parallel-safe var names.
#[test]
fn rb_007_env_empty_vs_absent() {
    let prior_empty = std::env::var("ACTIVE_TEST_RB007_EMPTY").ok();
    let prior_absent = std::env::var("ACTIVE_TEST_RB007_ABSENT_XYZ").ok();
    std::env::set_var("ACTIVE_TEST_RB007_EMPTY", "");
    std::env::remove_var("ACTIVE_TEST_RB007_ABSENT_XYZ");
    let v = eval_json(
        "e = env.ACTIVE_TEST_RB007_EMPTY ?: \"fallback\"\na = env.ACTIVE_TEST_RB007_ABSENT_XYZ ?: \"fallback\"\n",
    );
    match prior_empty {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB007_EMPTY", val),
        None => std::env::remove_var("ACTIVE_TEST_RB007_EMPTY"),
    }
    match prior_absent {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB007_ABSENT_XYZ", val),
        None => std::env::remove_var("ACTIVE_TEST_RB007_ABSENT_XYZ"),
    }
    assert_eq!(v["e"], "", "empty env must win over fallback");
    assert_eq!(v["a"], "fallback", "absent env must fall back");
}

/// RB-008 — Version strings keep zeros (P1-TI-003 §C).
#[test]
fn rb_008_version_strings_stay_strings() {
    let v = eval_json("version = \"1.10\"\nversion2 = \"1.20\"\n");
    assert_eq!(v["version"], "1.10");
    assert_eq!(v["version2"], "1.20");
}

/// RB-009 — Hash in string is not a comment (P1-DS-007 §F).
#[test]
fn rb_009_hash_in_string_kept() {
    let v = eval_json("value = \"abc # not comment\"\n");
    assert_eq!(v["value"], "abc # not comment");
    // Trailing comment after code is ignored.
    assert_eq!(eval_json("value = 1 # trailing\n")["value"], 1);
}

/// RB-010 — Deep nesting bounded (P0-001 §B, HIST-JSON-001 §G).
/// 300 nested tables must trip a budget/depth diagnostic, never overflow.
#[test]
fn rb_010_deep_nesting_bounded() {
    let mut src = String::from("x = ");
    for i in 0..300 {
        src.push_str(&format!("{{k{i} = "));
    }
    src.push('1');
    src.push_str(&"}".repeat(300));
    src.push('\n');
    let codes = codes_of(&src);
    assert!(!codes.is_empty(), "300-deep nesting must trip a budget");
    // Positive control: depth 5–10 succeeds.
    assert_eq!(eval_json("x = {a = {b = 1}}\n")["x"]["a"]["b"], 1);
}

/// RB-011 — Unit dimension mismatch rejected (DERIVED-0005 §A, P1-SE-011 §E).
#[test]
fn rb_011_unit_dimension_mismatch_rejected() {
    let codes = codes_of("bad = 5s + 512MiB\n");
    assert!(
        codes.iter().any(|c| c == "T001"),
        "T001 expected, got {codes:?}"
    );
    // Positive controls: same-dimension arithmetic normalizes.
    assert_eq!(eval_json("t = 5s + 100ms\n")["t"]["ms"], 5100);
    assert_eq!(
        eval_json("size = 512MiB + 64KiB\n")["size"],
        536870912 + 65536
    );
    // Scalar multiplication of a unit is allowed.
    assert_eq!(eval_json("t = 5s * 3\n")["t"]["ms"], 15000);
}

/// RB-012 — Schema range boundary inclusive (DERIVED-0015 §A).
#[test]
fn rb_012_schema_range_boundary() {
    let schema = "schema = {\n  port = Int(min = 1, max = 65535)\n}\n";
    assert_eq!(eval_json(&format!("{schema}port = 65535\n"))["port"], 65535);
    assert_eq!(eval_json(&format!("{schema}port = 1\n"))["port"], 1);
    for bad in ["port = 0\n", "port = 65536\n"] {
        let codes = codes_of(&format!("{schema}{bad}"));
        assert!(!codes.is_empty(), "{bad:?} must fail, parsed clean");
    }
}

/// RB-013 — Missing vs null distinct codes (DERIVED-0016 §A).
#[test]
fn rb_013_missing_vs_null_distinct() {
    let schema = "schema = {\n  name = String\n}\n";
    let missing_codes = codes_of(schema);
    let null_codes = codes_of(&format!("{schema}name = null\n"));
    assert!(!missing_codes.is_empty(), "missing key must fail");
    assert!(!null_codes.is_empty(), "null for String must fail");
    assert_ne!(
        missing_codes, null_codes,
        "missing and null must report different codes: {missing_codes:?} vs {null_codes:?}"
    );
    // Positive control: present string passes silent.
    let v = eval_json(&format!("{schema}name = \"svc\"\n"));
    assert_eq!(v["name"], "svc");
}

/// RB-014 — Patch remove missing path is W004 idempotent (DERIVED-0012 §A).
/// REFUSAL (spec-gated): research recommends error, but Active intent is
/// W004 idempotent success (diagnostics.md:91). Pin the specified behavior.
#[test]
fn rb_014_remove_missing_path_warns_w004() {
    let src = "[server]\nhost = \"localhost\"\n\n@remove(\"server.port\")\n";
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "W004"),
        "W004 expected for missing remove target, got {codes:?}"
    );
    // Positive control: removing an existing key succeeds.
    let v = eval_json("[server]\nhost = \"localhost\"\n\n@remove(\"server.host\")\n");
    assert!(v["server"].get("host").is_none(), "{v}");
}

/// RB-015 — Patch rename collision rejected (DERIVED-0013 §A).
#[test]
fn rb_015_rename_collision_rejected() {
    // Rename targets are plain key names in the same parent: colliding
    // with the existing `port` must fail E019 (see also
    // tests/patches/main_rename_conflict.active).
    let src = "[server]\nhost = \"localhost\"\nport = 8080\n\n@rename(\"server.host\", \"port\")\n";
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "E019"),
        "E019 expected for colliding rename, got {codes:?}"
    );
    // Positive control: rename to a free name succeeds.
    let v = eval_json("[server]\nhost = \"localhost\"\n\n@rename(\"server.host\", \"name\")\n");
    assert_eq!(v["server"]["name"], "localhost");
}

/// RB-016 — Profile precedence deterministic (DERIVED-0010 §A).
#[test]
fn rb_016_profile_precedence_deterministic() {
    let src = "active_profile = \"production\"\n\n[server]\nhost = \"localhost\"\nport = 8080\n\nprofile \"base\" {\n  server {\n    port = 9000\n  }\n}\n\nprofile \"production\" {\n  server {\n    host = \"prod.example.com\"\n  }\n}\n";
    let first = eval_json(src);
    let second = eval_json(src);
    assert_eq!(first, second, "profile selection must be deterministic");
    assert_eq!(first["server"]["host"], "prod.example.com");
    assert_eq!(first["server"]["port"], 8080);
    // Switching the selector changes the result per language rules.
    let other = eval_json(&src.replace(
        "active_profile = \"production\"",
        "active_profile = \"base\"",
    ));
    assert_eq!(other["server"]["port"], 9000);
    assert_ne!(first, other, "profile switch must change evaluation");
}

/// RB-017 — Int vs float spelling distinct (P1-TI-008 §C).
#[test]
fn rb_017_int_float_spelling_distinct() {
    let v = eval_json("i = 10\nf = 10.0\n");
    assert_eq!(v["i"], 10);
    assert_eq!(v["f"], 10.0);
    assert!(v["i"].is_i64(), "10 must stay integer: {v}");
    assert!(v["f"].is_f64(), "10.0 must stay float: {v}");
}

/// RB-018 — Yes/no/on/off quoted strings (P1-TI-002/010 §C).
#[test]
fn rb_018_keyword_lookalikes_stay_strings() {
    let v = eval_json(
        "words = [\"yes\", \"no\", \"true\", \"false\", \"null\", \"NO\", \"on\", \"off\"]\n",
    );
    let expected = ["yes", "no", "true", "false", "null", "NO", "on", "off"];
    for (i, want) in expected.iter().enumerate() {
        assert_eq!(v["words"][i], *want, "element {i} coerced: {v}");
    }
    // Bare booleans are still booleans.
    let v = eval_json("a = true\nb = false\n");
    assert_eq!(v["a"], true);
    assert_eq!(v["b"], false);
}

/// RB-019 — Empty vs null vs missing trio (P1-TI-007 §C).
#[test]
fn rb_019_empty_null_missing_trio() {
    let v = eval_json("a = \"\"\nb = null\n");
    assert_eq!(v["a"], "");
    assert!(v["b"].is_null(), "{v}");
    assert!(v.get("c").is_none(), "c must be absent: {v}");
}

/// RB-020 — Comments never alter structure (P1-DS-006 §F, HIST-YAML-013 §G).
#[test]
fn rb_020_comments_ignored_structurally() {
    let plain = eval_json("key = \"value\"\nlist = [1, 2,]\n");
    let commented = eval_json(
        "# header\nkey = \"value\" # trailing\n# own line\nlist = [1, 2,] # after comma\n",
    );
    assert_eq!(plain, commented, "comments must not alter structure");
}
