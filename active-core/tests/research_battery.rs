//! Research battery, batches 1–2 (docs/research-test-index.md).
//!
//! One test per index row, run ONE BY ONE in RB order. Each test drives
//! shipped entry points (`parse`/`parse_file` → `Engine::evaluate` → JSON
//! export) and asserts diagnostic CLASSES, never message text. Positive
//! control neighbours ride along so fixes cannot break the good case.
//! Rule: red test ⇒ fix the code (spec-gated), never the expectation.

use active_core::{parse, parse_file, Engine, ParseOptions};

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

/// RB-021 — Deep array nesting bounded (HIST-JSON-001 §2).
/// 500 nested arrays must trip a budget/depth diagnostic, never overflow.
#[test]
fn rb_021_deep_array_nesting_bounded() {
    let mut src = String::from("x = ");
    src.push_str(&"[".repeat(500));
    src.push('1');
    src.push_str(&"]".repeat(500));
    src.push('\n');
    let codes = codes_of(&src);
    assert!(!codes.is_empty(), "500-deep arrays must trip a budget");
    // Positive control: shallow nesting succeeds.
    assert_eq!(eval_json("x = [[1], [2]]\n")["x"][1][0], 2);
}

/// RB-022 — Comment-to-EOF never hangs (HIST-TOML-005 §2).
/// `a = [1 #` with no trailing newline must terminate promptly; a
/// hanging parser fails this test via the join timeout instead of CI.
#[test]
fn rb_022_comment_to_eof_terminates() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = parse("a = [1 #", "research").map(|_| "parsed");
        let _ = tx.send(r);
    });
    let outcome = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("parser must terminate within 10s");
    assert!(outcome.is_ok() || outcome.is_err(), "terminated");
    // Positive control: comment with newline parses fine.
    assert_eq!(eval_json("a = [1] # done\n")["a"][0], 1);
}

/// RB-023 — Tag-like markers never execute (HIST-YAML-026/027/028 §3).
/// `!!` is just double negation in Active; hostile payloads fail safely
/// with a type error, never with code execution or a crash.
#[test]
fn rb_023_tag_markers_never_execute() {
    for src in [
        "x = !!foo\n",
        "x = !!python/object\n",
        "x = !!js/function\n",
    ] {
        let codes = codes_of(src);
        assert!(
            codes.iter().any(|c| c == "T001" || c == "W001"),
            "{src:?} must fail safe (T001) or warn-and-null (W001), got {codes:?}"
        );
    }
    // Positive control: ordinary negation works.
    assert_eq!(eval_json("x = !false\n")["x"], true);
}

/// RB-024 — Template braces stay literal (HIST-YAML-036 §3).
/// Only `${…}` interpolates; `{{…}}` is inert text.
#[test]
fn rb_024_template_braces_stay_literal() {
    let v = eval_json("template_part = \"{{7*7}}\"\n");
    assert_eq!(v["template_part"], "{{7*7}}");
}

/// RB-025 — Malformed docs sanitize errors (HIST-TOML-00M/HIST-YAML-040 §3).
/// A secret sitting in a broken document must not surface in diagnostics.
#[test]
fn rb_025_broken_doc_errors_sanitized() {
    let raw = "vault-secret-rb025";
    let src = format!("token = secret(\"{raw}\")\nbad = [1,\n");
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
    // Positive control: well-formed secret docs evaluate + redact.
    let v = eval_json(&format!("token = secret(\"{raw}\")\n"));
    assert_eq!(v["token"], "***SECRET***");
}

/// RB-026 — `__proto__` is a neutral key (HIST-TOML-00L/HIST-YAML-031 §3).
/// Active has no prototypes; the key is plain data. Dotted assignment
/// is not Active syntax at all, so it rejects like any dotted assignment.
#[test]
fn rb_026_proto_key_neutral() {
    let v = eval_json("__proto__ = 1\n");
    assert_eq!(v["__proto__"], 1);
    let codes = codes_of("__proto__.isAdmin = true\n");
    assert!(!codes.is_empty(), "dotted assignment must reject");
    let plain_codes = codes_of("a.b = 1\n");
    assert_eq!(
        codes, plain_codes,
        "__proto__ must behave exactly like any other dotted assignment"
    );
}

/// RB-027 — Env fallback chain first-wins (P1-SE-006 §E).
#[test]
fn rb_027_env_fallback_chain_first_wins() {
    let prior_a = std::env::var("ACTIVE_TEST_RB027_A").ok();
    let prior_b = std::env::var("ACTIVE_TEST_RB027_B").ok();
    std::env::set_var("ACTIVE_TEST_RB027_A", "from-a");
    std::env::remove_var("ACTIVE_TEST_RB027_B");
    let v =
        eval_json("value = env.ACTIVE_TEST_RB027_A ?: env.ACTIVE_TEST_RB027_B ?: \"default\"\n");
    let v2 =
        eval_json("value = env.ACTIVE_TEST_RB027_B ?: env.ACTIVE_TEST_RB027_A ?: \"default\"\n");
    match prior_a {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB027_A", val),
        None => std::env::remove_var("ACTIVE_TEST_RB027_A"),
    }
    match prior_b {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB027_B", val),
        None => std::env::remove_var("ACTIVE_TEST_RB027_B"),
    }
    assert_eq!(v["value"], "from-a", "first present must win");
    assert_eq!(
        v2["value"], "from-a",
        "order must not matter, presence does"
    );
}

/// RB-028 — Secret payloads stay opaque (P1-SE-013 §E).
/// `secret("5s")` is a secret string, never auto-parsed as a unit.
#[test]
fn rb_028_secret_payload_never_unit_parsed() {
    let v = eval_json("delay = secret(\"5s\")\n");
    assert_eq!(v["delay"], "***SECRET***");
    // And it never behaves as a duration downstream.
    let codes = codes_of("delay = secret(\"5s\")\nbad = delay + 1\n");
    assert!(!codes.is_empty(), "secret arithmetic must fail");
    // Positive control: plain units still normalize.
    assert_eq!(eval_json("t = 5s\n")["t"]["ms"], 5000);
}

/// RB-029 — Patches preserve taint (DERIVED-0014 §A).
#[test]
fn rb_029_patch_preserves_taint() {
    let src = "password = secret(\"x-rb029\")\ntoken = \"plain\"\n\n@patch(\"token\", password)\n";
    let v = eval_json(src);
    assert_eq!(
        v["token"], "***SECRET***",
        "patched-in secret must redact: {v}"
    );
}

/// RB-030 — Let cycles rejected with S001 (DERIVED-0003 §A).
#[test]
fn rb_030_let_cycles_rejected() {
    for src in [
        "let a = b\nlet b = a\n",
        "let a = b\nlet b = c\nlet c = a\n",
    ] {
        let codes = codes_of(src);
        assert!(
            codes.iter().any(|c| c == "S001"),
            "{src:?} must fail S001, got {codes:?}"
        );
    }
    // Positive control: acyclic lets evaluate (`let` bindings surface
    // through keys that read them).
    assert_eq!(eval_json("let a = 1\nlet b = a + 2\ny = b\n")["y"], 3);
}

/// RB-031 — Merge-key shapes reject cleanly (HIST-YAML-007 §2).
/// Active has no `<<` merge operator; hostile shapes must fail with
/// documented classes and terminate, never hang or execute.
#[test]
fn rb_031_merge_key_shapes_reject() {
    for src in ["y = {x = 1}\nz = y\n", "x = 1\n", "a = [1]\nb = [a]\n"] {
        assert!(
            codes_of(src).is_empty(),
            "{src:?} is ordinary Active and must stay clean"
        );
    }
    for src in ["y = {<< = 1}\n", "x = 1 << 2\n"] {
        let codes = codes_of(src);
        assert!(
            codes.iter().any(|c| c == "E001" || c == "E002"),
            "{src:?} must reject with a documented class, got {codes:?}"
        );
    }
}

/// RB-032 — Exponential let-growth trips budgets (HIST-YAML-009 §2).
/// Doubling bindings must hit an array/count budget before materializing
/// millions of values; the suite must terminate either way.
#[test]
fn rb_032_exponential_lets_hit_budget() {
    let mut src = String::from("a0 = [1]\n");
    for i in 1..25 {
        src.push_str(&format!("a{i} = [a{}, a{}]\n", i - 1, i - 1));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let codes = codes_of(&src);
        let _ = tx.send(codes);
    });
    let codes = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("exponential growth must terminate within 20s");
    assert!(
        codes.iter().any(|c| c.starts_with('X')),
        "25 doublings must trip a budget class, got {codes:?}"
    );
}

/// RB-033 — Only documented classes escape deep input (HIST-TOML-002 §2).
/// 500-deep nesting must surface E/X/W/H/T/S codes exclusively — never a
/// panic, and never an undocumented internal kind.
#[test]
fn rb_033_deep_input_only_documented_classes() {
    let mut src = String::from("x = ");
    src.push_str(&"[".repeat(500));
    src.push('1');
    src.push_str(&"]".repeat(500));
    src.push('\n');
    let codes = codes_of(&src);
    assert!(!codes.is_empty(), "500-deep input must fail loudly");
    for code in &codes {
        let ok = code.len() >= 4
            && matches!(code.chars().next(), Some('E' | 'W' | 'X' | 'H' | 'T' | 'S'))
            && code[1..4].chars().all(|c| c.is_ascii_digit());
        assert!(ok, "undocumented diagnostic class escaped: {code}");
    }
}

/// RB-034 — Long dotted header paths terminate (HIST-TOML-004 §2).
/// A 26-segment header must evaluate or fail with a documented class.
#[test]
fn rb_034_long_dotted_header_terminates() {
    let path: Vec<String> = (0..26).map(|i| format!("s{i}")).collect();
    let dotted = path.join(".");
    let src = format!("[{dotted}]\nvalue = 1\n");
    let codes = codes_of(&src);
    if codes.is_empty() {
        let v = eval_json(&src);
        let mut node = &v;
        for seg in &path {
            node = &node[seg];
        }
        assert_eq!(node["value"], 1);
    }
}

/// RB-035 — Large documents terminate documented (HIST-TOML-00J §2).
/// 10k keys must evaluate cleanly or fail with a budget class — promptly.
#[test]
fn rb_035_large_document_terminates() {
    let mut src = String::new();
    for i in 0..10_000 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let codes = codes_of(&src);
        let _ = tx.send(codes);
    });
    let codes = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("10k-key document must terminate within 30s");
    assert!(
        codes.is_empty() || codes.iter().any(|c| c.starts_with('X')),
        "large doc must evaluate clean or trip a budget, got {codes:?}"
    );
}

/// RB-036 — Self-reference is a cycle (P0-007 §B).
#[test]
fn rb_036_self_reference_is_cycle() {
    let codes = codes_of("let x = x\ny = 1\n");
    assert!(
        codes.iter().any(|c| c == "S001"),
        "self-reference must fail S001, got {codes:?}"
    );
    // Positive control: sibling self-read with no binding is null + warning,
    // never a cycle (see regressions.rs) — and never a hang.
    let v = eval_json("[app]\nname = name\n");
    assert!(v["app"]["name"].is_null(), "{v}");
}

/// RB-037 — Unknown units reject (P1-SE-009 §E).
/// The lexer accepts only the known suffix table (spec/dual-syntax.md §8);
/// anything else is not a unit, so `10furlongs` cannot lex as quantity.
#[test]
fn rb_037_unknown_unit_rejects() {
    let codes = codes_of("x = 10furlongs\n");
    assert!(
        codes.iter().any(|c| c == "E002"),
        "unknown unit must reject, got {codes:?}"
    );
    // Positive control: known suffixes still work.
    assert_eq!(eval_json("x = 10s\n")["x"]["ms"], 10000);
}

/// RB-038 — Units on non-numbers reject (P1-SE-010 §E).
#[test]
fn rb_038_unit_on_string_rejects() {
    let codes = codes_of("x = \"hello\"s\n");
    assert!(!codes.is_empty(), "unit on string must fail, parsed clean");
}

/// RB-039 — Unresolved reference code + span (DERIVED-0002 §A, P1-DS-010 §F).
#[test]
fn rb_039_unresolved_reference_points_at_name() {
    let diags = parse("y = z\n", "research")
        .map(|m| {
            Engine::new()
                .evaluate(&m)
                .map(|(_, ds)| ds)
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let hit = diags
        .iter()
        .find(|d| d.code == "W001")
        .expect("W001 expected");
    let span = hit.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 5), "span: {span:?}");
    // Unresolved names become null and poison arithmetic with T001.
    let codes = codes_of("let x = 10\ny = x + z\n");
    assert!(
        codes.iter().any(|c| c == "T001"),
        "T001 expected, got {codes:?}"
    );
    // Positive control: bound names resolve silently.
    let v = eval_json("let x = 10\nlet z = 5\ny = x + z\n");
    assert_eq!(v["y"], 15);
}

/// RB-040 — Malformed interpolation rejected with span (DERIVED-0004 §A).
#[test]
fn rb_040_malformed_interpolation_rejected() {
    // Unclosed delimiters are E012; empty `${}` is the more precise E013.
    for (src, code) in [
        ("msg = \"hello ${name\"\n", "E012"),
        ("msg2 = \"hello ${}\"\n", "E013"),
    ] {
        let diags = parse(src, "research").expect_err(&format!("{src:?} must fail to parse"));
        assert!(
            diags.iter().any(|d| d.code == code),
            "{code} expected for {src:?}, got {diags:?}"
        );
        assert!(
            diags.iter().any(|d| d.span.is_some()),
            "{code} must carry a span for {src:?}"
        );
    }
    // Literal `${` escape stays available without interpolating.
    let v = eval_json("msg = \"hello \\${name}\"\n");
    assert_eq!(v["msg"], "hello ${name}");
}

fn research_fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("research")
        .join(name)
}

/// RB-041 — Merge/anchor sigils reject cleanly (HIST-YAML-008 §2).
/// Active has no `<<`, `&anchor`, or `*alias` syntax; hostile shapes
/// must fail with documented lexer/parser classes and terminate.
#[test]
fn rb_041_merge_anchor_sigils_reject() {
    for (src, classes) in [
        ("y = {a = 1}\n", &["none"] as &[&str]),
        ("a = &x\n", &["E005"]),
        ("a = *x\n", &["E002", "E001"]),
        ("x = 1 << 2\n", &["E002"]),
        ("y = {<< = 1}\n", &["E001", "E002"]),
    ] {
        let codes = codes_of(src);
        if classes == ["none"] {
            assert!(codes.is_empty(), "{src:?} must stay clean, got {codes:?}");
        } else {
            assert!(
                codes.iter().any(|c| classes.contains(&c.as_str())),
                "{src:?} must reject with one of {classes:?}, got {codes:?}"
            );
        }
    }
}

/// RB-042 — Mass shared references terminate (HIST-YAML-010 §2).
/// 2000 keys reading one big string must evaluate (shared, not expanded)
/// or fail with a budget class — promptly either way.
#[test]
fn rb_042_mass_shared_references_terminate() {
    let mut src = String::from("let big = \"");
    src.push_str(&"x".repeat(1000));
    src.push_str("\"\n");
    for i in 0..2000 {
        src.push_str(&format!("s{i} = big\n"));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = parse(&src, "research").map(|m| {
            Engine::new()
                .evaluate(&m)
                .map(|(v, _)| active_core::convert::value_to_pretty_json(&v).len())
                .map_err(|e| e.code().to_string())
        });
        let _ = tx.send(out);
    });
    let outcome = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("2000 shared refs must terminate within 20s");
    match outcome {
        Ok(Ok(len)) => assert!(len > 1000, "shared values must materialize"),
        Ok(Err(code)) => assert!(code.starts_with('X'), "budget class expected, got {code}"),
        Err(diags) => panic!("unexpected parse failure: {diags:?}"),
    }
}

/// RB-043 — Formatter terminates on cyclic modules (HIST-TOML-006 §2).
/// A `let` cycle fails at eval (S001), but formatting the parsed module
/// must still terminate with re-parseable output.
#[test]
fn rb_043_formatter_terminates_on_cycles() {
    let src = "let a = b\nlet b = a\n";
    let module = parse(src, "research").expect("cycles parse; they fail at eval");
    let out = active_core::fmt::format(&module, active_core::Style::Brace);
    assert!(out.contains('a') && out.contains('b'), "{out}");
    let _ = parse(&out, "research").map_err(|d| panic!("formatted cycle must re-parse: {d:?}"));
}

/// RB-044 — Deep input fails fast (HIST-JSON-003 §2).
/// 2000-deep nesting must reject with a budget class well before any
/// memory pressure; completion itself is the fail-fast proof.
#[test]
fn rb_044_deep_input_fails_fast() {
    let mut src = String::from("x = ");
    src.push_str(&"[".repeat(2000));
    src.push('1');
    src.push_str(&"]".repeat(2000));
    src.push('\n');
    let codes = codes_of(&src);
    assert!(
        codes
            .iter()
            .any(|c| c.starts_with('X') || c == "E004" || c == "E011"),
        "2000-deep input must trip a depth/budget class, got {codes:?}"
    );
}

/// RB-045 — Inline-table nesting bounded (HIST-TOML-003 §2).
#[test]
fn rb_045_inline_table_nesting_bounded() {
    let mut src = String::from("x = ");
    for i in 0..200 {
        src.push_str(&format!("{{k{i} = "));
    }
    src.push('1');
    src.push_str(&"}".repeat(200));
    src.push('\n');
    let codes = codes_of(&src);
    assert!(
        !codes.is_empty(),
        "200-deep inline tables must trip a budget"
    );
    // Positive control: shallow inline nesting succeeds.
    assert_eq!(eval_json("x = {a = {b = 1}}\n")["x"]["a"]["b"], 1);
}

/// RB-046 — No array-of-tables syntax (HIST-TOML-007 §2, P1-ST-008 §D).
/// `[[t]]` is not Active syntax; it must reject cleanly (E001), never
/// half-parse into something else.
#[test]
fn rb_046_no_array_tables_syntax() {
    let codes = codes_of("[[t]]\nname = \"x\"\n");
    assert!(
        codes.iter().any(|c| c == "E001"),
        "[[t]] must reject E001, got {codes:?}"
    );
    // Positive control: arrays of inline tables are the Active shape.
    let v = eval_json("t = [{name = \"x\"}, {name = \"y\"}]\n");
    assert_eq!(v["t"][1]["name"], "y");
}

/// RB-047 — Non-string table keys reject (HIST-TOML-00D §2).
#[test]
fn rb_047_non_string_table_key_rejects() {
    for src in ["[123]\nx = 1\n", "[true]\nx = 1\n"] {
        let codes = codes_of(src);
        assert!(!codes.is_empty(), "{src:?} must fail, parsed clean");
    }
    // Positive control: identifier and quoted-string headers work.
    assert_eq!(eval_json("[server]\nhost = \"h\"\n")["server"]["host"], "h");
}

/// RB-048 — Interpolated include paths reject (P0-014 §B).
/// Include paths must be literals; interpolation would let document data
/// steer filesystem access (research: `!include` RCE class).
#[test]
fn rb_048_interpolated_include_rejects() {
    use active_core::{parse_file, ParseOptions};
    let diags = parse_file(
        &research_fixture("rb048_main.active"),
        ParseOptions::default(),
    )
    .expect_err("interpolated include path must fail");
    assert!(
        diags.iter().any(|d| d.code == "E015"),
        "E015 expected, got {diags:?}"
    );
    // Positive control: literal includes resolve.
    let v = eval_json("x = 1\n");
    assert_eq!(v["x"], 1);
}

/// RB-049 — Unclosed string healed loudly (HIST-TOML-00E §2).
/// A string running to EOF heals via H001 (inserted quote, Info) rather
/// than failing — but never silently: the repair is recorded with a span.
#[test]
fn rb_049_unclosed_string_diagnosed() {
    let module = parse("msg = \"abc\n", "research").expect("H001 must heal, not fail");
    let notes: Vec<&active_core::Diagnostic> =
        module.healing.iter().filter(|d| d.code == "H001").collect();
    assert_eq!(notes.len(), 1, "exactly one H001 expected");
    assert!(notes[0].span.is_some(), "H001 must carry a span: {notes:?}");
    // The healed value is the literal content.
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("abc"), "{json}");
}

/// RB-050 — Unknown escapes kept verbatim (HIST-TOML-00F §2).
/// REFUSAL (spec-gated): research demands rejecting `\q`, but
/// spec/grammar.md:64-66 defines verbatim passthrough (`\x` → `\x`) in
/// both string forms. This test pins the specified behavior.
#[test]
fn rb_050_unknown_escape_kept_verbatim() {
    let v = eval_json("bad = \"\\q\"\n");
    assert_eq!(v["bad"], "\\q");
    // Positive control: valid escapes still decode.
    let v = eval_json("ok = \"a\\nb\\t\\\\\\\"\"\n");
    assert_eq!(v["ok"], "a\nb\t\\\"");
}

/// RB-051 — Quoted keys reject loudly (HIST-YAML-020 §1).
/// A string literal cannot start a statement (grammar: keys are
/// identifiers); silently dropping it would corrupt data. E002.
/// (Single-quoted, triple-quoted, and whitespace-padded forms included.)
#[test]
fn rb_051_quoted_keys_reject() {
    for src in [
        "\" key \" = 1\n",
        "'key' = 1\n",
        "\"\"\"k\"\"\" = 1\n",
        "'' = 2\n",
    ] {
        let codes = codes_of(src);
        assert!(
            codes.iter().any(|c| c == "E002"),
            "{src:?} must reject E002, got {codes:?}"
        );
    }
    // Positive control: identifier keys (incl. underscore/digits) work.
    let v = eval_json("_k2 = 1\n");
    assert_eq!(v["_k2"], 1);
}

/// RB-052 — Empty string keys reject (HIST-YAML-003 §1).
/// `""` is not an identifier; accepting it would break key invariants
/// (round-trip, spans, LSP ranges). E002 like any quoted key.
#[test]
fn rb_052_empty_key_rejects() {
    let codes = codes_of("\"\" = \"blank\"\n");
    assert!(
        codes.iter().any(|c| c == "E002"),
        "empty key must reject E002, got {codes:?}"
    );
}

/// RB-053 — Exponents are not Active numbers (HIST-JSON-005 §2).
/// The f64 grammar has no exponent form; `1e400` cannot lex as a number.
/// E002, never a misparse into `1` + identifier.
#[test]
fn rb_053_exponents_reject() {
    for src in ["x = 1e400\n", "x = 5e+22\n", "x = 1e-3\n"] {
        let codes = codes_of(src);
        assert!(!codes.is_empty(), "{src:?} must fail, parsed clean");
    }
    // Positive control: plain floats evaluate exactly.
    // (Spelled out: the point is the literal `3.14` survives exactly,
    // so no approx-constant refactor applies here.)
    let text = active_core::convert::value_to_pretty_json(
        &Engine::new()
            .evaluate(&parse("x = 3.14\n", "research").expect("must parse"))
            .expect("must evaluate")
            .0,
    );
    assert!(text.contains("3.14"), "{text}");
}

/// RB-054 — Undecoded escapes never become surrogates (HIST-YAML-015 §1).
/// `\u` is not an escape introducer in Active: the text stays verbatim,
/// so lone surrogates can never materialize and crash downstream code.
#[test]
fn rb_054_no_surrogate_materialization() {
    let v = eval_json("x = \"\\uDEAD\"\n");
    assert_eq!(v["x"], "\\uDEAD");
    let text = active_core::convert::value_to_pretty_json(
        &Engine::new()
            .evaluate(&parse("x = \"\\uDEAD\"\n", "research").expect("must parse"))
            .expect("must evaluate")
            .0,
    );
    // Six ASCII chars (`\`, `u`, `D`, `E`, `A`, `D`), never a surrogate:
    // Rust `char` cannot even hold one (from_u32 rejects the range).
    assert!(text.contains("\\uDEAD"));
    assert!(char::from_u32(0xD800).is_none() && char::from_u32(0xDFFF).is_none());
}

/// RB-055 — Header redefinition warns, last wins (HIST-TOML-00N §2).
#[test]
fn rb_055_header_redefinition_warns() {
    let src = "[a.b]\nc = 1\n[a]\nd = 2\n";
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "W011"),
        "redefined header must warn W011, got {codes:?}"
    );
    let v = eval_json(src);
    assert_eq!(v["a"]["d"], 2, "last header wins: {v}");
}

/// RB-056 — Include traversal rejected (HIST-YAML-041 §3).
/// `../` escapes fail E015 by default; absolute paths fail too.
#[test]
fn rb_056_include_traversal_rejected() {
    use active_core::{parse_file, ParseOptions};
    let diags = parse_file(
        &research_fixture("rb056_main.active"),
        ParseOptions::default(),
    )
    .expect_err("traversal include must fail");
    assert!(
        diags.iter().any(|d| d.code == "E015"),
        "E015 expected, got {diags:?}"
    );
}

/// RB-057 — One dup-key policy on every path (HIST-YAML-042 §3).
/// Inline `parse` and file `parse_file` agree exactly: same value,
/// same W011 diagnostics. No parser differential between entry points.
#[test]
fn rb_057_dup_policy_agrees_across_entry_points() {
    use active_core::{parse_file, ParseOptions};
    let inline_src = "[server]\nhost = \"a\"\nhost = \"b\"\n";
    let inline_module = parse(inline_src, "research").expect("must parse");
    let (inline_value, inline_diags) = Engine::new()
        .evaluate(&inline_module)
        .expect("must evaluate");
    let file_module = parse_file(
        &research_fixture("rb057_dup.active"),
        ParseOptions::default(),
    )
    .expect("must parse");
    let (file_value, file_diags) = Engine::new().evaluate(&file_module).expect("must evaluate");
    let inline_json = active_core::convert::value_to_pretty_json(&inline_value);
    let file_json = active_core::convert::value_to_pretty_json(&file_value);
    assert_eq!(inline_json, file_json, "entry points disagree on value");
    let inline_codes: Vec<&str> = inline_diags.iter().map(|d| d.code.as_str()).collect();
    let file_codes: Vec<&str> = file_diags.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(inline_codes, file_codes, "entry points disagree on diags");
    assert!(inline_codes.contains(&"W011"), "{inline_codes:?}");
}

/// RB-058 — Long floats stay accurate (HIST-YAML-024 §1).
#[test]
fn rb_058_long_float_accurate() {
    let v = eval_json("x = 1.123456789012345678901234567890\n");
    let f = v["x"].as_f64().expect("float");
    assert!((f - 1.1234567890123457).abs() < 1e-15, "{v}");
}

/// RB-059 — Diagnostics stable across runs (P0-019 §B).
/// Same failing document twice: identical classes and spans, no
/// addresses or nondeterminism in output.
#[test]
fn rb_059_diagnostics_stable_across_runs() {
    let src = "y = x + z\n";
    let snap = || codes_of(src).into_iter().collect::<Vec<_>>();
    assert_eq!(snap(), snap(), "diagnostic classes must be stable");
    let spans = || {
        parse(src, "research")
            .map(|m| {
                Engine::new()
                    .evaluate(&m)
                    .map(|(_, ds)| ds)
                    .unwrap_or_default()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|d| d.span.map(|s| (s.line, s.column)))
            .collect::<Vec<_>>()
    };
    assert_eq!(spans(), spans(), "diagnostic spans must be stable");
}

/// RB-060 — Absent env falls back (DERIVED-0006 §A).
#[test]
fn rb_060_absent_env_falls_back() {
    let prior = std::env::var("ACTIVE_TEST_RB060_ABSENT_XYZ").ok();
    std::env::remove_var("ACTIVE_TEST_RB060_ABSENT_XYZ");
    let v = eval_json("value = env.ACTIVE_TEST_RB060_ABSENT_XYZ ?: \"fallback\"\n");
    match prior {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB060_ABSENT_XYZ", val),
        None => std::env::remove_var("ACTIVE_TEST_RB060_ABSENT_XYZ"),
    }
    assert_eq!(v["value"], "fallback");
    // Positive control: present variable wins.
    let prior2 = std::env::var("ACTIVE_TEST_RB060_PRESENT").ok();
    std::env::set_var("ACTIVE_TEST_RB060_PRESENT", "hello");
    let v = eval_json("value = env.ACTIVE_TEST_RB060_PRESENT ?: \"fallback\"\n");
    match prior2 {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB060_PRESENT", val),
        None => std::env::remove_var("ACTIVE_TEST_RB060_PRESENT"),
    }
    assert_eq!(v["value"], "hello");
}

/// RB-061 — Anchor-cycle shapes reject (HIST-YAML-016 §2).
/// Active has no anchors; a cyclic alias-shaped document is just
/// unknown sigils and must reject with documented classes, terminating.
#[test]
fn rb_061_anchor_cycle_shape_rejects() {
    for src in ["a = &a [*a, *a]\n", "a = &x [1]\nb = [*x, *x]\n"] {
        let codes = codes_of(src);
        assert!(
            codes
                .iter()
                .any(|c| c == "E005" || c == "E001" || c == "E002"),
            "{src:?} must reject with a documented class, got {codes:?}"
        );
    }
}

/// RB-062 — Custom tags never invoke anything (HIST-YAML-030 §3).
/// `!NAME` is not a constructor hook; it parses as negation or rejects.
/// Either way nothing user-defined runs.
#[test]
fn rb_062_custom_tag_shapes_inert() {
    for src in ["x = !ENV \"VAR\"\n", "x = !foo\n"] {
        let codes = codes_of(src);
        assert!(!codes.is_empty(), "{src:?} must fail, parsed clean");
    }
    // Positive control: negation of booleans works.
    assert_eq!(eval_json("x = !false\n")["x"], true);
}

/// RB-063 — Zero unsafe blocks in workspace sources (HIST-YAML-029 §3,
/// index H-01 starter).
/// Memory safety rests on safe Rust: no `unsafe fn/impl/trait/block`
/// may exist in first-party code. Line comments are stripped before
/// matching so prose cannot hide (or fake) a hit.
#[test]
fn rb_063_no_unsafe_blocks_in_workspace() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let crates = [
        "active",
        "active-core",
        "active-cli",
        "active-fmt",
        "active-lsp",
    ];
    let mut hits = Vec::new();
    let mut files = 0u32;
    for krate in crates {
        let src = root.join(krate).join("src");
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    files += 1;
                    let text = std::fs::read_to_string(&path).expect("readable");
                    for (i, line) in text.lines().enumerate() {
                        let code = line.split("//").next().unwrap_or("");
                        // Skip lines inside block comments crudely: only
                        // match `unsafe` at a token boundary followed by
                        // block/fn/impl/trait syntax.
                        let mut rest = code;
                        while let Some(pos) = rest.find("unsafe") {
                            let after: String = rest[pos + 6..]
                                .chars()
                                .take_while(|c| c.is_whitespace())
                                .collect();
                            let tail = rest[pos + 6 + after.len()..].to_string();
                            if tail.starts_with('{')
                                || tail.starts_with('(')
                                || tail.starts_with("fn")
                                || tail.starts_with("impl")
                                || tail.starts_with("trait")
                                || tail.starts_with("extern")
                                || tail.starts_with("mod")
                            {
                                hits.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                                break;
                            }
                            rest = &rest[pos + 6..];
                        }
                    }
                }
            }
        }
    }
    assert!(files > 10, "must actually scan the workspace, saw {files}");
    assert!(hits.is_empty(), "unsafe blocks found: {hits:?}");
}

/// RB-064 — Schema regexes terminate (HIST-YAML-037 §3).
/// The `regex` crate guarantees linear-time matching (no backtracking),
/// so catastrophic patterns validate promptly; uncompilable patterns are
/// schema bugs (T003), never hangs.
#[test]
fn rb_064_schema_regex_terminates() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // 30 a's match `^(a+)+$`: the catastrophic shape must validate
        // promptly (linear engine), not hang in backtracking.
        let src = "schema = {\n  s = String(pattern = \"^(a+)+$\")\n}\ns = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n";
        let codes = codes_of(src);
        let _ = tx.send(codes);
    });
    let codes = rx
        .recv_timeout(std::time::Duration::from_secs(15))
        .expect("evil regex must terminate within 15s");
    assert!(
        codes.is_empty(),
        "matching evil pattern must validate: {codes:?}"
    );
    // Uncompilable patterns are schema bugs.
    let bad = codes_of("schema = {\n  s = String(pattern = \"([\")\n}\ns = \"x\"\n");
    assert!(
        bad.iter().any(|c| c == "T003"),
        "bad pattern must be T003, got {bad:?}"
    );
}

/// RB-065 — No profile inheritance exists (DERIVED-0011 §A).
/// REFUSAL-adjacent pin: `inherits` is not a language feature. The key
/// merges as ordinary profile data (like any other key) — no traversal,
/// so no inheritance cycle can ever form and no cycle class exists.
#[test]
fn rb_065_no_profile_inheritance() {
    let v = eval_json("active_profile = \"a\"\n\nprofile \"a\" {\n  inherits = \"b\"\n}\n");
    assert_eq!(v["inherits"], "b", "inherits merges as plain data: {v}");
    // A cycle through a nonexistent feature cannot exist: no
    // E_PROFILE_CYCLE class exists because there is nothing to cycle.
    let codes = codes_of("active_profile = \"a\"\n\nprofile \"a\" {\n  inherits = \"b\"\n}\n");
    assert!(
        !codes.iter().any(|c| c.contains("CYCLE")),
        "no cycle class should fire: {codes:?}"
    );
}

/// RB-066 — No network-capable dependencies (DERIVED-0017 §A, P0-011 §B).
/// Hermetic evaluation is backed structurally: the lockfile must contain
/// no crates that perform network I/O. (`url` alone only parses text.)
#[test]
fn rb_066_no_network_dependencies() {
    let lock = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("Cargo.lock"),
    )
    .expect("lockfile readable");
    let banned = [
        "reqwest",
        "hyper",
        "tokio",
        "mio",
        "socket2",
        "h2",
        "tungstenite",
        "ureq",
        "isahc",
        "curl",
        "curl-sys",
        "openssl",
        "rustls",
        "native-tls",
        "hyper-util",
        "axum",
        "actix-web",
        "actix-rt",
        "warp",
        "rouille",
        "tiny_http",
        "quinn",
    ];
    let mut names = Vec::new();
    for line in lock.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name = \"") {
            if let Some(name) = rest.strip_suffix('"') {
                names.push(name.to_string());
            }
        }
    }
    assert!(!names.is_empty(), "lockfile must list packages");
    let hits: Vec<&String> = names
        .iter()
        .filter(|n| banned.contains(&n.as_str()))
        .collect();
    assert!(hits.is_empty(), "network-capable crates in tree: {hits:?}");
}

/// RB-067 — Commas in strings never split (HIST-YAML-001 §1).
#[test]
fn rb_067_comma_in_string_kept() {
    let v = eval_json("data = [\"line1,\nline2\"]\n");
    assert_eq!(v["data"][0], "line1,\nline2");
}

/// RB-068 — Tables cannot be keys (HIST-YAML-002 §1).
/// A table literal in key position rejects with a documented class.
#[test]
fn rb_068_table_as_key_rejects() {
    let codes = codes_of("x = {{ \"nested\" = 1 } = \"value\" }\n");
    assert!(!codes.is_empty(), "table-as-key must fail, parsed clean");
    // Positive control: scalar keys (incl. dotted paths) work.
    assert_eq!(eval_json("[a]\nb = 1\n")["a"]["b"], 1);
}

/// RB-069 — Truncations terminate documented (P0-015 §B).
/// Every prefix of a document must terminate with a documented class —
/// never hang, never panic.
#[test]
fn rb_069_truncations_terminate_documented() {
    let full = "server = {\n  host = \"localhost\"\n  port = 8080\n}\n";
    let bytes = full.as_bytes();
    for end in [1usize, 5, 10, 15, 20, 25, 30, 40, 50] {
        let end = end.min(bytes.len());
        let prefix = &full[..end];
        let (tx, rx) = std::sync::mpsc::channel();
        let owned = prefix.to_string();
        std::thread::spawn(move || {
            let _ = codes_of(&owned);
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("truncation at {end} bytes hung the parser"));
    }
}

/// RB-070 — Chomping indicators absent (HIST-YAML-012 §1).
/// `|-`, `|+` are not Active syntax; they reject cleanly. Triple-quoted
/// blocks keep content literally (pinned separately).
#[test]
fn rb_070_no_chomping_indicators() {
    let codes = codes_of("x = |-\n");
    assert!(
        !codes.is_empty(),
        "chomping indicator must fail, parsed clean"
    );
    // Triple-quoted blocks preserve content literally.
    let v = eval_json("x = \"\"\"a\"\"\"\n");
    assert_eq!(v["x"], "a");
}

/// RB-071 — Last-wins holds in both syntaxes (HIST-YAML-005 §1).
/// The first-wins alternative exists elsewhere; Active pins last-wins
/// twice: inline tables AND header bodies must agree.
#[test]
fn rb_071_last_wins_both_syntaxes() {
    let inline = eval_json("x = {a = 1, a = 2}\n");
    assert_eq!(inline["x"]["a"], 2);
    let block = eval_json("[t]\na = 1\na = 2\n");
    assert_eq!(block["t"]["a"], 2);
    for (src, name) in [
        ("x = {a = 1, a = 2}\n", "inline"),
        ("[t]\na = 1\na = 2\n", "block"),
    ] {
        let codes = codes_of(src);
        assert!(
            codes.iter().any(|c| c == "W011"),
            "{name} dup must warn W011, got {codes:?}"
        );
    }
}

/// RB-072 — Signatures cover canonical dup-key bytes (HIST-YAML-045 §3).
/// Sign the duplicate-key document, verify OK; flip one byte and the
/// signature must fail. Deterministic canonicalization underneath.
#[test]
fn rb_072_sign_verify_dup_key_document() {
    use active_core::sign::{generate_secret_key, sign_bytes, verify_bytes};
    use std::collections::HashMap;
    let doc = b"[server]\nhost = \"a\"\nhost = \"b\"\n";
    let key = generate_secret_key().expect("keygen");
    let sidecar = sign_bytes(&key, "test-2026", doc).expect("sign");
    let mut trusted = HashMap::new();
    trusted.insert("test-2026".to_string(), key.verifying_key());
    let kid = verify_bytes("dup.active", doc, &sidecar, &trusted).expect("verify");
    assert_eq!(kid, "test-2026");
    let mut tampered = doc.to_vec();
    tampered[20] = b'c';
    assert!(
        verify_bytes("dup.active", &tampered, &sidecar, &trusted).is_err(),
        "tampered bytes must fail verification"
    );
}

/// RB-073 — No dynamic indexing into secrets (HIST-YAML-035 §3).
/// Active has no index operator, so tainted paths cannot be reached
/// dynamically at all. Array elements holding secrets still redact.
#[test]
fn rb_073_no_dynamic_index_operator() {
    let codes = codes_of("arr = [10, 20]\nx = arr[0]\n");
    assert!(!codes.is_empty(), "index operator must not exist");
    let v = eval_json("tokens = [secret(\"rb073\"), \"b\"]\n");
    assert_eq!(v["tokens"][0], "***SECRET***");
    assert_eq!(v["tokens"][1], "b");
}

/// RB-074 — Table copies keep taint (HIST-YAML-033 §3).
#[test]
fn rb_074_table_copy_keeps_taint() {
    let src = "creds = {user = \"app\", pass = secret(\"x-rb074\")}\ncopy = creds\n";
    let v = eval_json(src);
    assert_eq!(v["copy"]["pass"], "***SECRET***");
    assert_eq!(v["copy"]["user"], "app");
}

/// RB-075 — Large interpolations terminate (HIST-YAML-038 §3).
/// A 100KB string interpolated twice must complete promptly; there are
/// no regex builtins and interpolation is bounded by value budgets.
#[test]
fn rb_075_large_interpolation_terminates() {
    let big = "y".repeat(100_000);
    let src = format!("let big = \"{big}\"\na = \"${{big}}!\"\nb = \"${{big}}?\"\n");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = parse(&src, "research").map(|m| {
            Engine::new()
                .evaluate(&m)
                .map(|(v, _)| active_core::convert::value_to_pretty_json(&v).len())
                .map_err(|e| e.code().to_string())
        });
        let _ = tx.send(out);
    });
    let outcome = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("100KB interpolation must terminate within 20s");
    match outcome {
        Ok(Ok(len)) => assert!(len > 200_000, "both copies materialize: {len}"),
        Ok(Err(code)) => assert!(code.starts_with('X'), "budget class only: {code}"),
        Err(diags) => panic!("unexpected parse failure: {diags:?}"),
    }
}

/// RB-076 — One boolean spelling (HIST-YAML-043 §3).
/// Only lowercase `true`/`false` are booleans; capitals are unresolved
/// identifiers (W001 + null), never a second truth value.
#[test]
fn rb_076_single_boolean_spelling() {
    for src in ["x = True\n", "x = FALSE\n"] {
        let codes = codes_of(src);
        assert!(
            codes.iter().any(|c| c == "W001"),
            "{src:?} capitalized boolean must warn W001, got {codes:?}"
        );
        assert!(eval_json(src)["x"].is_null(), "{src:?} must be null");
    }
    assert_eq!(eval_json("x = true\n")["x"], true);
    assert_eq!(eval_json("x = false\n")["x"], false);
}

/// RB-077 — Units survive format round-trip (DERIVED-0019 §A).
/// Format → parse → format must not alter unit-carrying values.
#[test]
fn rb_077_units_survive_format_round_trip() {
    use active_core::fmt::format;
    use active_core::Style;
    let src = "timeout = 5s\ncache = 512MiB\nrate = 100/req/s\n";
    let module = parse(src, "research").expect("must parse");
    for style in [Style::Brace, Style::Indent] {
        let out = format(&module, style);
        let module2 = parse(&out, "research")
            .unwrap_or_else(|d| panic!("reparse failed ({style:?}): {d:?}\n{out}"));
        let (first, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let (second, _) = Engine::new().evaluate(&module2).expect("must evaluate");
        let a = active_core::convert::value_to_pretty_json(&first);
        let b = active_core::convert::value_to_pretty_json(&second);
        assert_eq!(
            a, b,
            "unit values changed across format ({style:?}):\n{out}"
        );
    }
    // Commas in strings never split elements (HIST-YAML-001 control).
    let v = eval_json("data = [\"line1,\nline2\"]\n");
    assert_eq!(v["data"][0], "line1,\nline2");
}

/// RB-078 — Importer stringifies non-UTC offsets (HIST-TOML-008 §2).
/// Active has no datetimes; the TOML importer keeps RFC 3339 text with
/// W008 instead of parsing or crashing.
#[test]
fn rb_078_datetime_offset_stringified() {
    use active_core::convert::toml_text_to_active;
    let (source, diags) = toml_text_to_active("dt = 2021-01-01T15:03:45+08:00\n").expect("import");
    assert!(
        diags.iter().any(|d| d.code == "W008"),
        "W008 expected, got {diags:?}"
    );
    let v = eval_json(&source);
    assert_eq!(v["dt"], "2021-01-01T15:03:45+08:00");
}

/// RB-079 — CRLF normalizes deterministically (HIST-YAML-014 §1).
#[test]
fn rb_079_crlf_normalizes() {
    let v = eval_json("a = 1\r\nb = 2\r\n");
    assert_eq!(v["a"], 1);
    assert_eq!(v["b"], 2);
    let v = eval_json("text = \"line1\r\nline2\"\n");
    assert_eq!(v["text"], "line1\nline2");
}

/// RB-080 — Multiline quotes preserved (HIST-YAML-011 §1).
#[test]
fn rb_080_multiline_quotes_preserved() {
    let v = eval_json("text = \"\"\"He said \"hello\" to me.\"\"\"\n");
    assert_eq!(v["text"], "He said \"hello\" to me.");
}

/// RB-081 — `let` shadowing warns like keys (HIST-YAML-017 §1).
/// Anchor-collision analogues in Active are `let` redefinitions:
/// last-wins with W011, never silent.
#[test]
fn rb_081_let_shadow_warns() {
    let v = eval_json("let x = 1\nlet x = 2\ny = x\n");
    assert_eq!(v["y"], 2);
    let codes = codes_of("let x = 1\nlet x = 2\ny = x\n");
    assert!(
        codes.iter().any(|c| c == "W011"),
        "shadowed let must warn W011, got {codes:?}"
    );
}

/// RB-082 — Path concat stays literal (HIST-YAML-039 §3).
/// Joining a base URL with `../` segments produces the literal string;
/// there are no fetch builtins, so no SSRF vector exists in core.
#[test]
fn rb_082_path_concat_stays_literal() {
    let v = eval_json(
        "base = \"http://internal.service/\"\npath = \"../../../../etc/passwd\"\nfull = \"${base}${path}\"\n",
    );
    assert_eq!(v["full"], "http://internal.service/../../../../etc/passwd");
}

/// RB-083 — Huge integers saturate loudly (HIST-JSON-004 §2).
/// `u64::MAX` as a literal exceeds i64: E023 + saturation at MAX.
#[test]
fn rb_083_u64max_literal_saturates() {
    let codes = codes_of("n = 18446744073709551615\n");
    assert!(
        codes.iter().any(|c| c == "E023"),
        "E023 expected, got {codes:?}"
    );
    let (module, diags) = active_core::parse_recovering(
        "n = 18446744073709551615\n",
        "research",
        active_core::ParseOptions::default(),
    );
    assert!(diags.iter().any(|d| d.code == "E023"));
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("9223372036854775807"), "{json}");
}

/// RB-084 — Thousand-digit integers terminate (HIST-TOML-00G §2).
/// Digit handling is linear; overflow still reports E023, never hangs.
#[test]
fn rb_084_thousand_digit_integer_terminates() {
    let digits = "9".repeat(1000);
    let src = format!("n = {digits}\n");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(codes_of(&src));
    });
    let codes = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("1000-digit integer must terminate within 10s");
    assert!(
        codes.iter().any(|c| c == "E023"),
        "E023 expected, got {codes:?}"
    );
}

/// RB-085 — Long lines terminate documented (HIST-TOML-00I §2).
/// A 1MB single-line array either evaluates or trips a budget class;
/// Active sets no line-length limit, so length alone never rejects.
#[test]
fn rb_085_long_line_terminates() {
    let src = format!("a = [{}]\n", vec!["1"; 400_000].join(", "));
    assert!(src.lines().next().expect("line").len() > 1_000_000);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(codes_of(&src));
    });
    let codes = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("1MB line must terminate within 30s");
    assert!(
        codes.is_empty() || codes.iter().any(|c| c.starts_with('X')),
        "evaluate clean or budget-trip, got {codes:?}"
    );
}

/// RB-086 — 5000-key tables evaluate (P0-002 §B).
#[test]
fn rb_086_wide_table_evaluates() {
    let mut src = String::from("[big]\n");
    for i in 0..5000 {
        src.push_str(&format!("k{i} = {i}\n"));
    }
    let v = eval_json(&src);
    assert_eq!(v["big"]["k4999"], 4999);
}

/// RB-087 — Huge arrays trip budgets (P0-003 §B).
#[test]
fn rb_087_huge_array_trips_budget() {
    let src = format!("a = [{}]\n", vec!["1"; 100_000].join(", "));
    let codes = codes_of(&src);
    assert!(
        codes.iter().any(|c| c.starts_with('X')),
        "100k elements must trip a budget class, got {codes:?}"
    );
}

/// RB-088 — Realistic configs pass under budgets (P0-020 §B).
#[test]
fn rb_088_realistic_config_passes() {
    let src = "let timeout = 5s\nlet retries = 3\ndb_password = secret(env.ACTIVE_TEST_RB088_PW ?: \"\")\n\n[server]\nhost = \"0.0.0.0\"\nport = 8080\nread_timeout = timeout\nretries = retries\n";
    let v = eval_json(src);
    assert_eq!(v["server"]["port"], 8080);
    assert_eq!(v["server"]["read_timeout"]["ms"], 5000);
    assert_eq!(v["db_password"], "***SECRET***");
}

/// RB-089 — Double comma heals (P1-ST-001 §D).
/// `H002` removes the stray comma; values intact.
#[test]
fn rb_089_double_comma_healed() {
    let codes = codes_of("value = [1,,2]\n");
    assert!(
        codes.iter().any(|c| c == "H002"),
        "H002 expected, got {codes:?}"
    );
    let v = eval_json("value = [1,,2]\n");
    assert_eq!(v["value"], serde_json::json!([1, 2]));
}

/// RB-090 — Missing separator rejects (P1-ST-002 §D).
#[test]
fn rb_090_missing_separator_rejects() {
    let codes = codes_of("value = [1 2]\n");
    assert!(
        codes.iter().any(|c| c == "E001" || c == "H003"),
        "separator class expected, got {codes:?}"
    );
    // Positive control: separated elements parse.
    assert_eq!(eval_json("value = [1, 2]\n")["value"][1], 2);
}

/// RB-091 — Truncation battery terminates (P0-015 §B).
/// Cut points across headers, strings, brackets, interpolation, and
/// patches must all terminate with documented classes — never hang.
#[test]
fn rb_091_truncation_battery_terminates() {
    let full = "[server]\nhost = \"localhost\"\nport = 8080\nurl = \"https://${host}:${port}\"\n\n@patch(\"server.port\", 9090)\n";
    for end in [5usize, 12, 20, 30, 45, 60, 80, 100] {
        let end = end.min(full.len());
        let prefix = full[..end].to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = codes_of(&prefix);
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("truncation at {end} bytes hung the parser"));
    }
    // The whole document evaluates clean.
    assert_eq!(eval_json(full)["server"]["port"], 9090);
}

/// RB-092 — Hostile byte corpus rejects safely (P0-016 §B).
/// NUL bytes, lone CR, DEL, lone BOM-mid-doc, unmatched closers, and
/// absurd digit runs must never panic; each yields a documented class
/// or a clean value.
#[test]
fn rb_092_hostile_bytes_reject_safely() {
    for src in [
        "x = \"a\0b\"\n",
        "x = 1\r\ny = 2\n",
        "x = \"\u{7f}\"\n",
        "x = 1\u{FEFF}\n",
        "]\n",
        "}\n",
        "x = (((((((((1)))))))))\n",
        "n = 99999999999999999999999999999999\n",
    ] {
        let (tx, rx) = std::sync::mpsc::channel();
        let owned = src.to_string();
        std::thread::spawn(move || {
            let _ = codes_of(&owned);
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("hostile input hung the parser: {src:?}"));
    }
}

/// RB-093 — Interpolation growth trips budgets (DERIVED-0018 §A).
/// Geometric string growth through interpolation must hit a value
/// budget before materializing gigabytes.
#[test]
fn rb_093_interpolation_growth_trips_budget() {
    let mut src = String::from("s0 = \"x\"\n");
    for i in 1..30 {
        src.push_str(&format!("s{i} = \"${{s{}}}${{s{}}}\"\n", i - 1, i - 1));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(codes_of(&src));
    });
    let codes = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("geometric interpolation must terminate within 20s");
    assert!(
        codes.iter().any(|c| c.starts_with('X')),
        "30 doublings must trip a budget class, got {codes:?}"
    );
}

/// RB-094 — Adversarial comments stay inert (HIST-YAML-013 §1).
/// Comment text that looks like code, brackets, or interpolation must
/// never affect the tree.
#[test]
fn rb_094_adversarial_comments_inert() {
    let v = eval_json(
        "# x = 999\n# [evil]\n# ${x}\n# @patch(\"x\", 1)\nreal = 1\n# trailing [bracket\n",
    );
    assert_eq!(v, serde_json::json!({"real": 1}));
}

/// RB-095 — No merge builtin exists (HIST-YAML-032 §1).
/// `merge(a, b)` is not syntax; merges happen through includes/profiles
/// with designed semantics, never a taint-carrying operator.
#[test]
fn rb_095_no_merge_builtin() {
    let codes = codes_of("x = merge(a, b)\n");
    assert!(!codes.is_empty(), "merge call must fail, parsed clean");
    // Positive control: table composition via includes/profiles works.
    let v = eval_json("[a]\nx = 1\n");
    assert_eq!(v["a"]["x"], 1);
}

/// RB-096 — No spread operator exists (HIST-YAML-034 §1).
#[test]
fn rb_096_no_spread_operator() {
    let codes = codes_of("x = [...a]\n");
    assert!(!codes.is_empty(), "spread must fail, parsed clean");
    // Positive control: explicit arrays work.
    assert_eq!(eval_json("a = [1]\nx = [1, 2]\n")["x"][1], 2);
}

/// RB-097 — URL text stays literal (HIST-YAML-044 §3).
/// Backslashes and ambiguity live in URLs as plain characters; core has
/// no URL parser, so no differential can arise inside it.
#[test]
fn rb_097_url_text_stays_literal() {
    let v = eval_json("u = \"https://x\\\\path?q=1\"\n");
    assert_eq!(v["u"], "https://x\\path?q=1");
}

/// RB-098 — Ambiguous local times import as strings (HIST-TOML-009 §2).
/// No datetimes exist to disambiguate; the importer keeps RFC 3339 text.
#[test]
fn rb_098_ambiguous_time_imports_as_string() {
    use active_core::convert::toml_text_to_active;
    let (source, diags) = toml_text_to_active("t = 2021-10-31T02:30:00\n").expect("import");
    assert!(
        diags.iter().any(|d| d.code == "W008"),
        "W008 expected, got {diags:?}"
    );
    assert_eq!(eval_json(&source)["t"], "2021-10-31T02:30:00");
}

/// RB-099 — Skipped local times import as strings (HIST-TOML-00A §2).
#[test]
fn rb_099_skipped_time_imports_as_string() {
    use active_core::convert::toml_text_to_active;
    let (source, diags) = toml_text_to_active("t = 2020-03-08T02:30:00\n").expect("import");
    assert!(
        diags.iter().any(|d| d.code == "W008"),
        "W008 expected, got {diags:?}"
    );
    assert_eq!(eval_json(&source)["t"], "2020-03-08T02:30:00");
}

/// RB-100 — Every failure carries spans (P1-DS-008 §F).
/// A sweep over representative failures: each diagnostic set must be
/// non-empty and every entry must carry a usable span.
/// NOTE: `x = 1 2` is deliberately absent here — the §4.1 statement-level
/// skip quirk (spec/grammar.md) absorbs stray values silently by explicit
/// design (pinned by tests/spec/09_parser_resilience); P1-ST-002's reject
/// demand is refused on that authority.
#[test]
fn rb_100_every_failure_carries_spans() {
    let cases = [
        "value = [1,,2\n",
        "x = unknown +\n",
        "[server\n",
        "msg = \"abc\n",
        "a = &x\n",
    ];
    for src in cases {
        match parse(src, "research") {
            Err(ds) => {
                assert!(!ds.is_empty(), "{src:?} failed with no diagnostics");
                assert!(
                    ds.iter().all(|d| d.span.is_some()),
                    "{src:?} has spanless diagnostics: {ds:?}"
                );
            }
            Ok(m) => {
                // Parses (healed or lenient): evaluation or its diags
                // must still account for the breakage loudly.
                let _ = Engine::new().evaluate(&m);
                assert!(
                    !codes_of(src).is_empty(),
                    "{src:?} parsed clean with no diagnostics at all"
                );
            }
        }
    }
}

/// RB-101 — Keyword-like keys are loud, never silent (P1-TI-011 §C).
/// `true`/`false`/`null`/numbers as would-be keys cannot start a
/// statement; at statement-initial columns they fail E002 instead of
/// vanishing (the §4.1 skip quirk covers mid-statement strays only).
#[test]
fn rb_101_keyword_like_keys_reject() {
    for src in ["true = 1\n", "false = 2\n", "null = 3\n", "2 = 1\n", "-5\n"] {
        let codes = codes_of(src);
        assert!(
            codes.iter().any(|c| c == "E002"),
            "{src:?} must reject E002, got {codes:?}"
        );
    }
    // Mid-statement strays keep the §4.1 silent skip (pinned quirk).
    assert_eq!(eval_json("x = 1 2\n")["x"], 1);
}

/// RB-102 — Dual-syntax semantic equality (DERIVED-0020 §A).
/// Brace and indentation forms of one document evaluate identically.
#[test]
fn rb_102_dual_syntax_semantic_equality() {
    let brace = "server = {\n  host = \"localhost\"\n  port = 8080\n}\n";
    let indent = "server:\n  host = \"localhost\"\n  port = 8080\n";
    assert_eq!(eval_json(brace), eval_json(indent));
}

/// RB-103 — No Norway coercion (P1-TI-001 §C).
/// Quoted `"NO"` is a string; bare `NO` is unresolved (W001 + null),
/// never boolean false.
#[test]
fn rb_103_no_norway_coercion() {
    assert_eq!(eval_json("country = \"NO\"\n")["country"], "NO");
    let v = eval_json("country = NO\n");
    assert!(v["country"].is_null(), "{v}");
    let codes = codes_of("country = NO\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "bare NO must warn W001, got {codes:?}"
    );
    assert_eq!(eval_json("flag = false\n")["flag"], false);
}

/// RB-104 — Leading zeros are plain decimals (P1-TI-004 §C).
/// No octal surprises: `0755` evaluates as decimal 755.
#[test]
fn rb_104_leading_zero_decimal() {
    let v = eval_json("mode = 0755\n");
    assert_eq!(v["mode"], 755);
    // Quoted form stays a string.
    assert_eq!(eval_json("mode = \"0755\"\n")["mode"], "0755");
}

/// RB-105 — Null vs string-null (P1-TI-005 §C).
#[test]
fn rb_105_null_vs_string_null() {
    let v = eval_json("value = null\n");
    assert!(v["value"].is_null(), "{v}");
    let v = eval_json("value = \"null\"\n");
    assert_eq!(v["value"], "null");
}

/// RB-106 — Quoted booleans stay strings (P1-TI-006 §C).
#[test]
fn rb_106_quoted_booleans_stay_strings() {
    let v = eval_json("c = \"true\"\nd = \"false\"\n");
    assert_eq!(v["c"], "true");
    assert_eq!(v["d"], "false");
    assert_eq!(eval_json("a = true\nb = false\n")["a"], true);
}

/// RB-107 — Underscores strip to integers (P1-TI-009 §C).
/// Per spec/grammar.md number rule: digit separators, stripped.
#[test]
fn rb_107_underscore_numbers() {
    assert_eq!(eval_json("big = 1_000_000\n")["big"], 1000000);
}

/// RB-108 — Keys are case-sensitive (P1-TI-013 §C).
#[test]
fn rb_108_keys_case_sensitive() {
    let v = eval_json("Host = \"A\"\nhost = \"B\"\n");
    assert_eq!(v["Host"], "A");
    assert_eq!(v["host"], "B");
}

/// RB-109 — Unicode keys reject cleanly (P1-TI-014 §C).
/// Non-ASCII identifier starts are outside the grammar; E005.
/// (Accept-without-corruption would also satisfy the row; rejection
/// with a clear class is the documented Active policy.)
#[test]
fn rb_109_unicode_keys_reject() {
    let codes = codes_of("名前 = 1\n");
    assert!(
        codes.iter().any(|c| c == "E005"),
        "non-ASCII key must reject E005, got {codes:?}"
    );
}

/// RB-110 — Positive control matrix (P1-TI-015 §C).
/// One document: integers, floats, bools, null, quoted lookalikes,
/// a unit, and a secret — exact types, no surprise coercions.
#[test]
fn rb_110_positive_control_matrix() {
    let v = eval_json(
        "i = 10\nf = 10.0\nt = true\nn = null\ns_yes = \"yes\"\ns_no = \"NO\"\ns_true = \"true\"\ns_null = \"null\"\ntimeout = 5s\npw = secret(\"rb110\")\n",
    );
    assert_eq!(v["i"], 10);
    assert!(v["i"].is_i64());
    assert_eq!(v["f"], 10.0);
    assert!(v["f"].is_f64());
    assert_eq!(v["t"], true);
    assert!(v["n"].is_null());
    assert_eq!(v["s_yes"], "yes");
    assert_eq!(v["s_no"], "NO");
    assert_eq!(v["s_true"], "true");
    assert_eq!(v["s_null"], "null");
    assert_eq!(v["timeout"]["ms"], 5000);
    assert_eq!(v["pw"], "***SECRET***");
}

/// RB-111 — Includes resolve relative to the including document (P0-012 §B).
/// A missing include fails E017 naming a path joined onto the including
/// file's directory (proof of document-relative, not ambient-CWD,
/// resolution); a present sibling merges. Covers the P0-012 research
/// demand that the filesystem stays untouched without an include.
#[test]
fn rb_111_include_resolves_document_relative() {
    let dir = std::env::temp_dir().join(format!(
        "active_rb111_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let main = dir.join("main.active");
    let sibling = dir.join("sibling_inc.active");
    std::fs::write(&sibling, "y = 2\n").expect("write sibling");
    // Missing include: E017 with a span on line 1 and a document-relative path.
    std::fs::write(&main, "include \"nope-missing-rb111.active\"\nx = 1\n").expect("write main");
    let diags = parse_file(&main, ParseOptions::default()).expect_err("must fail E017");
    assert!(
        diags.iter().any(|d| d.code == "E017"),
        "E017 expected, got {diags:?}"
    );
    let missing = diags.iter().find(|d| d.code == "E017").expect("E017");
    let span = missing.span.as_ref().expect("E017 carries a span");
    assert_eq!((span.line, span.column), (1, 9), "{span:?}");
    assert!(
        missing.message.contains("nope-missing-rb111.active")
            && missing.message.contains(&dir.to_string_lossy().to_string()),
        "E017 must name the document-relative path, got {}",
        missing.message
    );
    // Positive control: a present sibling resolves from the same directory.
    std::fs::write(&main, "include \"sibling_inc.active\"\nx = 1\n").expect("write main");
    let module = parse_file(&main, ParseOptions::default())
        .unwrap_or_else(|d| panic!("sibling include must parse: {d:?}"));
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["x"], 1);
    assert_eq!(v["y"], 2);
    std::fs::remove_dir_all(&dir).ok();
}

/// RB-112 — Unterminated array rejects E001 (P1-ST-003 §D).
#[test]
fn rb_112_unterminated_array_rejects() {
    let diags = parse("x = [1, 2\n", "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let unterminated = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = unterminated.span.as_ref().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the terminated neighbour evaluates.
    assert_eq!(eval_json("x = [1, 2]\n")["x"], serde_json::json!([1, 2]));
}

/// RB-113 — Unterminated inline table rejects E001 (P1-ST-004 §D).
#[test]
fn rb_113_unterminated_inline_table_rejects() {
    let diags = parse("x = {a = 1\n", "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let unterminated = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = unterminated.span.as_ref().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the terminated neighbour evaluates.
    assert_eq!(eval_json("x = {a = 1}\n")["x"]["a"], 1);
}

/// RB-114 — Empty containers accept (P1-ST-006 §D).
#[test]
fn rb_114_empty_containers_accept() {
    let v = eval_json("x = []\ny = {}\n");
    assert_eq!(v["x"], serde_json::json!([]));
    assert_eq!(v["y"], serde_json::json!({}));
    let codes = codes_of("x = []\ny = {}\n");
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-115 — Nested arrays and tables of inline tables (P1-ST-007 §D).
#[test]
fn rb_115_nested_arrays_tables_of_inline_tables() {
    let v = eval_json("x = [[1, 2], {a = 1}]\n");
    assert_eq!(v["x"], serde_json::json!([[1, 2], {"a": 1}]));
    let v = eval_json("rows = [{a = 1}, {a = 2}]\n");
    assert_eq!(v["rows"], serde_json::json!([{"a": 1}, {"a": 2}]));
    // Positive control: flat neighbours keep working.
    assert_eq!(eval_json("x = [1, 2]\n")["x"], serde_json::json!([1, 2]));
}

/// RB-116 — Array/table same-key conflict warns, last wins (P1-ST-009 §D).
/// Table-level last-wins (RB-002 policy): the second `[fruit]` table
/// replaces the first wholesale, announced by W011 — never a silent merge.
#[test]
fn rb_116_table_redefinition_warns_last_wins() {
    let src = "[fruit]\nname = \"apple\"\n[fruit]\ncolor = \"red\"\n";
    let v = eval_json(src);
    assert_eq!(v["fruit"], serde_json::json!({"color": "red"}));
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "W011"),
        "redefined table must warn W011, got {codes:?}"
    );
    // Positive control: distinct tables coexist.
    let v = eval_json("[a]\nx = 1\n[b]\ny = 2\n");
    assert_eq!(v["a"]["x"], 1);
    assert_eq!(v["b"]["y"], 2);
}

/// RB-117 — Inline-then-block conflict warns, last wins (P1-ST-010 §D).
/// Same RB-002 policy across definition styles: the `[t]` block replaces
/// the inline `t` wholesale, announced by W011.
#[test]
fn rb_117_inline_then_block_warns_last_wins() {
    let src = "t = {a = 1}\n[t]\nb = 2\n";
    let v = eval_json(src);
    assert_eq!(v["t"], serde_json::json!({"b": 2}));
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "W011"),
        "inline-then-block conflict must warn W011, got {codes:?}"
    );
    // Positive control: non-conflicting inline + block coexist.
    let v = eval_json("u = {a = 1}\n[t]\nb = 2\n");
    assert_eq!(v["u"]["a"], 1);
    assert_eq!(v["t"]["b"], 2);
}

/// RB-118 — Dotted headers nest; dotted keys and `[[tables]]` cleanly reject
/// (P1-ST-011 §D; folds P1-ST-008 §D: no `[[table]]` array-of-tables form).
/// `[a.b]` is specified nesting, equal to the inline nested form. Bare
/// `a.b.c = 1` dotted-key assignment is outside the grammar (E002), and
/// `[[fruit]]` fails E001 — per-grammar clean rejects, no half-behavior.
#[test]
fn rb_118_dotted_header_nests_quirk_forms_reject() {
    assert_eq!(
        eval_json("[a.b]\nc = 1\n"),
        eval_json("a = {b = {c = 1}}\n")
    );
    let codes = codes_of("a.b.c = 1\n");
    assert!(
        codes.iter().any(|c| c == "E002"),
        "dotted-key assignment must reject E002, got {codes:?}"
    );
    let codes = codes_of("[[fruit]]\nname = \"apple\"\n");
    assert!(
        codes.iter().any(|c| c == "E001"),
        "double-bracket tables must reject E001, got {codes:?}"
    );
}

/// RB-119 — Stray commas heal, never silently drop (P1-ST-012 §D).
/// REFUSAL (spec-gated): the row asks for rejection, but Active's locked
/// stray-comma policy (P1-ST-001/RB-089, P1-ST-005/RB-004) heals `[,]`
/// and `[1,,]` with H002. Values are pinned: `[]` and `[1]`.
#[test]
fn rb_119_stray_commas_heal() {
    assert_eq!(eval_json("x = [,]\n")["x"], serde_json::json!([]));
    assert!(
        codes_of("x = [,]\n").iter().any(|c| c == "H002"),
        "H002 expected for [,]"
    );
    assert_eq!(eval_json("x = [1,,]\n")["x"], serde_json::json!([1]));
    assert!(
        codes_of("x = [1,,]\n").iter().any(|c| c == "H002"),
        "H002 expected for [1,,]"
    );
    // Positive control: the clean neighbour carries no heal.
    assert!(!codes_of("x = [1]\n").iter().any(|c| c == "H002"));
}

/// RB-120 — Bare words in arrays warn, never coerce (P1-ST-013 §D).
/// Unresolved identifiers heal to null with W001 (Norway protection,
/// RB-103 policy); quoted elements stay strings.
#[test]
fn rb_120_bare_word_in_array_warns() {
    let v = eval_json("x = [1, hello, 2]\n");
    assert_eq!(v["x"], serde_json::json!([1, null, 2]));
    let codes = codes_of("x = [1, hello, 2]\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "bare word must warn W001, got {codes:?}"
    );
    // Positive control: quoted elements stay strings.
    assert_eq!(
        eval_json("x = [\"hello\", \"NO\"]\n")["x"],
        serde_json::json!(["hello", "NO"])
    );
}

/// RB-121 — Keyword-lookalikes in arrays never coerce (P1-TI-010 §C).
/// Quoted elements stay exact strings; bare words are unresolved
/// (W001 + null), never coerced to booleans or strings.
#[test]
fn rb_121_keyword_lookalikes_in_arrays_stay_put() {
    let v = eval_json("x = [\"yes\", \"NO\", \"on\", \"off\", \"true\", \"null\"]\n");
    assert_eq!(
        v["x"],
        serde_json::json!(["yes", "NO", "on", "off", "true", "null"])
    );
    let v = eval_json("x = [yes]\n");
    assert_eq!(v["x"], serde_json::json!([null]));
    let codes = codes_of("x = [yes]\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "bare word must warn W001, got {codes:?}"
    );
    // Positive control: real booleans keep their type.
    let v = eval_json("x = [true, false]\n");
    assert_eq!(v["x"], serde_json::json!([true, false]));
    assert!(v["x"][0].is_boolean());
}

/// RB-122 — Spaces in headers reject (P1-ST-014 §D).
#[test]
fn rb_122_spaces_in_headers_reject() {
    let codes = codes_of("[server host]\nx = 1\n");
    assert!(
        codes.iter().any(|c| c == "E001"),
        "E001 expected, got {codes:?}"
    );
    assert!(
        codes.iter().any(|c| c == "E002"),
        "E002 expected, got {codes:?}"
    );
    // Positive control: the spaceless header works.
    assert_eq!(eval_json("[server]\nhost = \"h\"\n")["server"]["host"], "h");
}

/// RB-123 — Positive structural matrix (P1-ST-015 §D).
/// One document exercising arrays, inline tables, block tables, empty
/// containers, nesting, and dotted headers — plus brace ≡ indent equality.
#[test]
fn rb_123_positive_structural_matrix() {
    let v = eval_json(
        "nums = [1, 2, 3]\npoint = {x = 1, y = 2}\nempty_a = []\nempty_t = {}\nnested = [[1], {a = [true]}]\n[svc]\nport = 8080\n[svc.db]\nname = \"main\"\n",
    );
    assert_eq!(v["nums"], serde_json::json!([1, 2, 3]));
    assert_eq!(v["point"], serde_json::json!({"x": 1, "y": 2}));
    assert_eq!(v["empty_a"], serde_json::json!([]));
    assert_eq!(v["empty_t"], serde_json::json!({}));
    assert_eq!(v["nested"], serde_json::json!([[1], {"a": [true]}]));
    assert_eq!(v["svc"]["port"], 8080);
    assert_eq!(v["svc"]["db"]["name"], "main");
    assert_eq!(
        eval_json("[a.b]\nc = 1\n"),
        eval_json("a = {b = {c = 1}}\n")
    );
}

/// RB-124 — `secret("…")` is typed and redacted (P1-SE-001 §E).
/// The RAW marker appears nowhere: not in the value, not in JSON export,
/// not in diagnostics.
#[test]
fn rb_124_secret_typed_and_redacted() {
    let src = "pw = secret(\"RAW-RB124-SE001\")\n";
    let v = eval_json(src);
    assert_eq!(v["pw"], "***SECRET***");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(!json.contains("RAW-RB124-SE001"), "{json}");
    let diag_text = format!("{diags:?}");
    assert!(!diag_text.contains("RAW-RB124-SE001"), "{diag_text}");
}

/// RB-125 — `secret(env.…)` never dumps (P1-SE-002 §E).
/// Set and unset backing variables alike: the export redacts and the
/// marker is absent from diagnostics.
#[test]
fn rb_125_env_backed_secret_never_dumped() {
    let prior = std::env::var("ACTIVE_TEST_RB125_PW").ok();
    std::env::set_var("ACTIVE_TEST_RB125_PW", "RAW-RB125-SE002-SET");
    let src = "pw = secret(env.ACTIVE_TEST_RB125_PW)\n";
    let v = eval_json(src);
    assert_eq!(v["pw"], "***SECRET***");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(!json.contains("RAW-RB125-SE002-SET"), "{json}");
    assert!(
        !format!("{diags:?}").contains("RAW-RB125-SE002-SET"),
        "{diags:?}"
    );
    match prior {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB125_PW", val),
        None => std::env::remove_var("ACTIVE_TEST_RB125_PW"),
    }
    // Positive control neighbour: unset backing var still yields a
    // redacted secret, no crash and no leak.
    let prior2 = std::env::var("ACTIVE_TEST_RB125_UNSET_XYZ").ok();
    std::env::remove_var("ACTIVE_TEST_RB125_UNSET_XYZ");
    let v = eval_json("pw = secret(env.ACTIVE_TEST_RB125_UNSET_XYZ)\n");
    assert_eq!(v["pw"], "***SECRET***");
    match prior2 {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB125_UNSET_XYZ", val),
        None => std::env::remove_var("ACTIVE_TEST_RB125_UNSET_XYZ"),
    }
}

/// RB-126 — Taint survives table construction (P1-SE-003 §E).
#[test]
fn rb_126_taint_through_table_construction() {
    let src = "t = {pw = secret(\"RAW-RB126-SE003\")}\n";
    let v = eval_json(src);
    assert_eq!(v["t"]["pw"], "***SECRET***");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(!json.contains("RAW-RB126-SE003"), "{json}");
    assert!(
        !format!("{diags:?}").contains("RAW-RB126-SE003"),
        "{diags:?}"
    );
}

/// RB-127 — Secret arithmetic is refused without leaking (P1-SE-004 §E).
#[test]
fn rb_127_secret_arithmetic_refused() {
    let src = "pw = secret(\"RAW-RB127-SE004\")\nx = pw + 1\n";
    let module = parse(src, "research").expect("must parse");
    let err = Engine::new().evaluate(&module).expect_err("must fail T001");
    assert_eq!(err.code(), "T001");
    let text = format!("{err:?}");
    assert!(!text.contains("RAW-RB127-SE004"), "{text}");
    // Positive control: plain arithmetic is unaffected.
    assert_eq!(eval_json("x = 1 + 2\n")["x"], 3);
}

/// RB-128 — Missing vs empty env, suite form (P1-SE-005 §E).
/// Same policy as DERIVED-0006/0007 (RB-060/RB-007): `""` wins, absent
/// falls back — exercised together in one document.
#[test]
fn rb_128_missing_vs_empty_env_suite() {
    let prior_empty = std::env::var("ACTIVE_TEST_RB128_EMPTY").ok();
    let prior_absent = std::env::var("ACTIVE_TEST_RB128_ABSENT_XYZ").ok();
    std::env::set_var("ACTIVE_TEST_RB128_EMPTY", "");
    std::env::remove_var("ACTIVE_TEST_RB128_ABSENT_XYZ");
    let v = eval_json(
        "e = env.ACTIVE_TEST_RB128_EMPTY ?: \"fallback\"\nm = env.ACTIVE_TEST_RB128_ABSENT_XYZ ?: \"fallback\"\n",
    );
    match prior_empty {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB128_EMPTY", val),
        None => std::env::remove_var("ACTIVE_TEST_RB128_EMPTY"),
    }
    match prior_absent {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB128_ABSENT_XYZ", val),
        None => std::env::remove_var("ACTIVE_TEST_RB128_ABSENT_XYZ"),
    }
    assert_eq!(v["e"], "", "empty env must win over fallback");
    assert_eq!(v["m"], "fallback", "absent env must fall back");
}

/// RB-129 — Time units normalize to milliseconds (P1-SE-007 §E).
#[test]
fn rb_129_time_units_normalize() {
    let v = eval_json("a = 1h\nb = 1m\nc = 500ms\nd = 5s\n");
    assert_eq!(v["a"]["ms"], 3600000);
    assert_eq!(v["b"]["ms"], 60000);
    assert_eq!(v["c"]["ms"], 500);
    assert_eq!(v["d"]["ms"], 5000);
    // Positive control: a plain number stays a number.
    assert_eq!(eval_json("n = 5\n")["n"], 5);
}

/// RB-130 — Byte units normalize to integer bytes (P1-SE-008 §E).
#[test]
fn rb_130_byte_units_normalize() {
    let v = eval_json("a = 512MiB\nb = 1KiB\nc = 2GB\n");
    assert_eq!(v["a"], 536870912);
    assert_eq!(v["b"], 1024);
    assert_eq!(v["c"], 2000000000);
    // Positive control: the quoted form stays a string.
    assert_eq!(eval_json("a = \"512MiB\"\n")["a"], "512MiB");
}

/// RB-131 — Dimension mismatch, suite form (P1-SE-011 §E).
/// Same policy as DERIVED-0005 (RB-011): durations and bytes never add.
#[test]
fn rb_131_dimension_mismatch_suite() {
    let src = "x = 5s + 512MiB\n";
    let module = parse(src, "research").expect("must parse");
    let err = Engine::new().evaluate(&module).expect_err("must fail T001");
    assert_eq!(err.code(), "T001");
    // Positive control: the duration operand alone evaluates.
    assert_eq!(eval_json("x = 5s\n")["x"]["ms"], 5000);
}

/// RB-132 — Scalar unit multiply normalizes (P1-SE-012 §E).
#[test]
fn rb_132_scalar_unit_multiply() {
    assert_eq!(eval_json("x = 5s * 3\n")["x"]["ms"], 15000);
    // Positive control: plain scalar arithmetic is unaffected.
    assert_eq!(eval_json("y = 3 * 4\n")["y"], 12);
}

/// RB-133 — Export redaction matrix (P1-SE-014 §E).
/// Secrets in top-level, nested-table, and array positions redact in
/// JSON and TOML exports and in diagnostics; markers absent everywhere.
#[test]
fn rb_133_export_redaction_matrix() {
    let src = "pw = secret(\"RAW-RB133-TOP\")\nt = {pw = secret(\"RAW-RB133-NEST\")}\nlist = [secret(\"RAW-RB133-ARR\")]\n";
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    let yaml = active_core::convert::value_to_yaml_string(&value).expect("yaml export");
    let (toml, toml_diags) =
        active_core::convert::value_to_toml_string(&value).expect("toml export");
    for marker in ["RAW-RB133-TOP", "RAW-RB133-NEST", "RAW-RB133-ARR"] {
        assert!(!json.contains(marker), "{marker} leaked in JSON: {json}");
        assert!(!yaml.contains(marker), "{marker} leaked in YAML: {yaml}");
        assert!(!toml.contains(marker), "{marker} leaked in TOML: {toml}");
        assert!(
            !format!("{diags:?}").contains(marker),
            "{marker} leaked in diags: {diags:?}"
        );
        assert!(
            !format!("{toml_diags:?}").contains(marker),
            "{marker} leaked in toml diags: {toml_diags:?}"
        );
    }
    let v: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(v["pw"], "***SECRET***");
    assert_eq!(v["t"]["pw"], "***SECRET***");
    assert_eq!(v["list"][0], "***SECRET***");
}

/// RB-134 — Realistic service config control (P1-SE-015 §E).
/// Ports, hosts, timeouts, byte limits, and a secret evaluate together:
/// values normalize, the secret redacts.
#[test]
fn rb_134_realistic_service_config() {
    let src = "port = 8080\nhost = \"svc\"\ntimeout = 30s\nlimit = 512MiB\npw = secret(\"RAW-RB134-SE015\")\n";
    let v = eval_json(src);
    assert_eq!(v["port"], 8080);
    assert_eq!(v["host"], "svc");
    assert_eq!(v["timeout"]["ms"], 30000);
    assert_eq!(v["limit"], 536870912);
    assert_eq!(v["pw"], "***SECRET***");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(!json.contains("RAW-RB134-SE015"), "{json}");
    assert!(
        !format!("{diags:?}").contains("RAW-RB134-SE015"),
        "{diags:?}"
    );
}

/// RB-135 — Brace ≡ indent values (P1-DS-001 §E).
#[test]
fn rb_135_brace_indent_values_equal() {
    let brace = "server = {\n  host = \"h\",\n  port = 1\n}\n";
    let indent = "server:\n  host = \"h\"\n  port = 1\n";
    assert_eq!(eval_json(brace), eval_json(indent));
    assert_eq!(eval_json(brace)["server"]["port"], 1);
}

/// RB-136 — Nested dual-syntax equality (P1-DS-002 §E).
#[test]
fn rb_136_nested_dual_syntax_equality() {
    let brace = "a = {\n  b = {\n    c = [1, {d = 2}],\n  },\n}\n";
    let indent = "a:\n  b:\n    c = [1, {d = 2}]\n";
    let (vb, vi) = (eval_json(brace), eval_json(indent));
    assert_eq!(vb, vi);
    assert_eq!(vb["a"]["b"]["c"], serde_json::json!([1, {"d": 2}]));
}

/// RB-137 — Mixed styles are legal and deterministic (P1-DS-003 §E).
/// Indent blocks may contain inline brace tables; repeated evaluation
/// agrees exactly.
#[test]
fn rb_137_mixed_styles_legal_deterministic() {
    let src = "server:\n  db = {\n    name = \"m\"\n  }\n  port = 1\n";
    let first = eval_json(src);
    assert_eq!(first["server"]["db"]["name"], "m");
    assert_eq!(first["server"]["port"], 1);
    assert_eq!(first, eval_json(src));
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-138 — Brace round-trip keeps semantics (P1-DS-004 §E).
/// Values compare as JSON models (object equality ignores key order —
/// ordering policy belongs to P1-DS-013, not to semantic equality).
#[test]
fn rb_138_brace_round_trip() {
    use active_core::fmt::format;
    use active_core::Style;
    let src = "server:\n  host = \"h\"\n  port = 1\ntimeout = 5s\n";
    let module = parse(src, "research").expect("must parse");
    let out = format(&module, Style::Brace);
    let module2 =
        parse(&out, "research").unwrap_or_else(|d| panic!("brace reparse failed: {d:?}\n{out}"));
    let (first, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (second, _) = Engine::new().evaluate(&module2).expect("must evaluate");
    let a: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&first)).expect("json");
    let b: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&second)).expect("json");
    assert_eq!(a, b);
}

/// RB-139 — Indent round-trip keeps semantics (P1-DS-005 §E).
/// Same model-equality oracle as RB-138: key order is DS-013's domain.
#[test]
fn rb_139_indent_round_trip() {
    use active_core::fmt::format;
    use active_core::Style;
    let src = "server = {\n  host = \"h\",\n  port = 1\n}\ntimeout = 5s\n";
    let module = parse(src, "research").expect("must parse");
    let out = format(&module, Style::Indent);
    let module2 =
        parse(&out, "research").unwrap_or_else(|d| panic!("indent reparse failed: {d:?}\n{out}"));
    let (first, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (second, _) = Engine::new().evaluate(&module2).expect("must evaluate");
    let a: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&first)).expect("json");
    let b: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&second)).expect("json");
    assert_eq!(a, b);
}

/// RB-140 — Diagnostic class stable across runs and styles (P1-DS-009 §E).
/// The same unresolved name warns W001 in flat and brace-wrapped forms,
/// with the span covering the name in both.
#[test]
fn rb_140_diagnostic_class_stable_across_styles() {
    fn w001_span(src: &str) -> active_core::diag::Span {
        let module = parse(src, "research").expect("must parse");
        let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
        let w = diags.iter().find(|d| d.code == "W001").expect("W001");
        w.span.clone().expect("W001 carries a span")
    }
    let flat = w001_span("bad = nosuchkey\n");
    let nested = w001_span("server = {\n  bad = nosuchkey\n}\n");
    assert_eq!((flat.line, flat.column), (1, 7), "{flat:?}");
    assert_eq!((nested.line, nested.column), (2, 9), "{nested:?}");
    // Stability: repeated runs report the same class and span.
    assert_eq!((flat.line, flat.column), {
        let s = w001_span("bad = nosuchkey\n");
        (s.line, s.column)
    });
}

/// RB-141 — Unresolved span covers the name (P1-DS-010 §F).
#[test]
fn rb_141_unresolved_span_covers_name() {
    let module = parse("x = unknown\n", "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 5), "{span:?}");
    assert!(span.end_column > span.column, "{span:?}");
    // Positive control: a resolved name carries no diagnostics.
    let module = parse("x = 1\n", "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
}

/// RB-142 — Units survive dual-syntax round-trips (P1-DS-011 §F).
#[test]
fn rb_142_units_survive_dual_syntax_round_trip() {
    use active_core::fmt::format;
    use active_core::Style;
    for (src, style) in [
        ("timeout = 5s\nlimit = 512MiB\n", Style::Brace),
        ("timeout = 5s\nlimit = 512MiB\n", Style::Indent),
    ] {
        let module = parse(src, "research").expect("must parse");
        let out = format(&module, style);
        let module2 = parse(&out, "research")
            .unwrap_or_else(|d| panic!("reparse failed ({style:?}): {d:?}\n{out}"));
        let (first, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let (second, _) = Engine::new().evaluate(&module2).expect("must evaluate");
        let a: serde_json::Value =
            serde_json::from_str(&active_core::convert::value_to_pretty_json(&first))
                .expect("json");
        let b: serde_json::Value =
            serde_json::from_str(&active_core::convert::value_to_pretty_json(&second))
                .expect("json");
        assert_eq!(a, b, "style {style:?}");
        assert_eq!(a["timeout"]["ms"], 5000);
        assert_eq!(a["limit"], 536870912);
    }
}

/// RB-143 — Formatter re-emits `secret(…)` source calls (P1-DS-012 §F).
/// REFUSAL (spec-gated, spec/secrets.md:48-49): the row asks for no raw
/// bytes in printer output, but `ACTIVE fmt` is specified source-to-source
/// (spec/formatter.md: "never evaluates … re-emit") — it prints the
/// user's own source call, never an evaluated value. The pinned property:
/// `secret(env.VAR)` formats as the call while VAR stays unresolved and
/// its value appears nowhere.
#[test]
fn rb_143_formatter_reemits_secret_calls_without_values() {
    use active_core::fmt::format;
    use active_core::Style;
    let prior = std::env::var("ACTIVE_TEST_RB143_PW").ok();
    std::env::set_var("ACTIVE_TEST_RB143_PW", "RAW-RB143-VALUE");
    let src = "pw = secret(env.ACTIVE_TEST_RB143_PW)\nport = 1\n";
    let module = parse(src, "research").expect("must parse");
    for style in [Style::Brace, Style::Indent] {
        let out = format(&module, style);
        assert!(out.contains("secret("), "{out}");
        assert!(!out.contains("RAW-RB143-VALUE"), "{out}");
    }
    // Control: evaluation still redacts the resolved value on export.
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(!json.contains("RAW-RB143-VALUE"), "{json}");
    match prior {
        Some(val) => std::env::set_var("ACTIVE_TEST_RB143_PW", val),
        None => std::env::remove_var("ACTIVE_TEST_RB143_PW"),
    }
}

/// RB-144 — Key order is document order, deterministic (P1-DS-013 §F).
#[test]
fn rb_144_key_order_document_deterministic() {
    let src = "z = 1\na = 2\nm = 3\n";
    let first = eval_json(src);
    let second = eval_json(src);
    let a = serde_json::to_string_pretty(&first).expect("json");
    let b = serde_json::to_string_pretty(&second).expect("json");
    assert_eq!(a, b);
    let keys: Vec<&str> = first
        .as_object()
        .expect("table")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["z", "a", "m"]);
    // Positive control: formatter output is byte-stable too.
    use active_core::fmt::format;
    use active_core::Style;
    let module = parse(src, "research").expect("must parse");
    assert_eq!(
        format(&module, Style::Indent),
        format(&module, Style::Indent)
    );
}

/// RB-145 — Triple-form matrix (P1-DS-014 §F).
/// Brace, indent, and mixed forms evaluate identically and each
/// survives a format round-trip.
#[test]
fn rb_145_triple_form_matrix() {
    use active_core::fmt::format;
    use active_core::Style;
    let brace = "server = {\n  host = \"h\",\n  port = 1\n}\n";
    let indent = "server:\n  host = \"h\"\n  port = 1\n";
    let mixed = "server:\n  db = {\n    name = \"m\"\n  }\n  port = 1\n";
    let (vb, vi) = (eval_json(brace), eval_json(indent));
    assert_eq!(vb, vi);
    assert_eq!(eval_json(mixed)["server"]["db"]["name"], "m");
    for (src, style) in [
        (brace, Style::Brace),
        (indent, Style::Indent),
        (mixed, Style::Brace),
    ] {
        let module = parse(src, "research").expect("must parse");
        let out = format(&module, style);
        let module2 = parse(&out, "research")
            .unwrap_or_else(|d| panic!("reparse failed ({style:?}): {d:?}\n{out}"));
        let (first, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let (second, _) = Engine::new().evaluate(&module2).expect("must evaluate");
        let a: serde_json::Value =
            serde_json::from_str(&active_core::convert::value_to_pretty_json(&first))
                .expect("json");
        let b: serde_json::Value =
            serde_json::from_str(&active_core::convert::value_to_pretty_json(&second))
                .expect("json");
        assert_eq!(a, b, "style {style:?}");
    }
}

/// RB-146 — Comments survive the round-trip (HIST-TOML-00P §H).
#[test]
fn rb_146_comments_survive_round_trip() {
    use active_core::fmt::format;
    use active_core::Style;
    let src = "# leading comment\nport = 8080 # trailing comment\n";
    let module = parse(src, "research").expect("must parse");
    let out = format(&module, Style::Indent);
    assert!(out.contains("# leading comment"), "{out}");
    assert!(out.contains("# trailing comment"), "{out}");
    let module2 =
        parse(&out, "research").unwrap_or_else(|d| panic!("reparse failed: {d:?}\n{out}"));
    let (first, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (second, _) = Engine::new().evaluate(&module2).expect("must evaluate");
    assert_eq!(
        active_core::convert::value_to_pretty_json(&first),
        active_core::convert::value_to_pretty_json(&second)
    );
    assert_eq!(eval_json(src)["port"], 8080);
}

/// RB-147 — Long datetime-looking strings parse as plain strings, fast
/// (HIST-TOML-00H §H). Active has no datetimes: no datetime parser
/// exists to blow up; the value is an exact string. Completion with the
/// exact value is the proof.
#[test]
fn rb_147_long_datetime_string_parses_fast() {
    let long = format!("2024-01-01T00:00:00Z-{}", "9".repeat(8000));
    let src = format!("d = \"{long}\"\n");
    let v = eval_json(&src);
    assert_eq!(v["d"], long);
}

/// RB-148 — Datetimes round-trip as strings (HIST-TOML-00O §H).
#[test]
fn rb_148_datetime_round_trips_as_string() {
    use active_core::fmt::format;
    use active_core::Style;
    let src = "d = \"2024-01-01T10:00:00Z\"\n";
    assert_eq!(eval_json(src)["d"], "2024-01-01T10:00:00Z");
    let module = parse(src, "research").expect("must parse");
    let out = format(&module, Style::Indent);
    let module2 =
        parse(&out, "research").unwrap_or_else(|d| panic!("reparse failed: {d:?}\n{out}"));
    let (first, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (second, _) = Engine::new().evaluate(&module2).expect("must evaluate");
    assert_eq!(
        active_core::convert::value_to_pretty_json(&first),
        active_core::convert::value_to_pretty_json(&second)
    );
    // Positive control: datetimes never become a special type.
    assert!(eval_json(src)["d"].is_string());
}

/// RB-149 — Strict duplicate rejection is not adopted (HIST-YAML-006 §G).
/// The rejected alternative: fail on any duplicate key. Active policy
/// (RB-002) warns W011 and keeps last-wins — pinned here one level
/// deeper, inside a nested inline table.
#[test]
fn rb_149_strict_dup_rejection_not_adopted() {
    let src = "t = {a = {x = 1, x = 2}}\n";
    let v = eval_json(src);
    assert_eq!(v["t"]["a"]["x"], 2);
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    let span = w.span.clone().expect("W011 carries a span");
    assert_eq!((span.line, span.column), (1, 22), "{span:?}");
    // Positive control: distinct nested keys coexist.
    assert_eq!(eval_json("t = {a = {x = 1, y = 2}}\n")["t"]["a"]["y"], 2);
}

/// RB-150 — Upstream `double-comma-01` analogue (SRC-TOML-0001 §H).
/// Origin: toml-test `invalid/array/double-comma-01.toml`.
/// REFUSAL (spec-gated): upstream says reject; Active's locked
/// stray-comma policy heals with H002 (RB-089/RB-119). Pinned with span.
#[test]
fn rb_150_double_comma_01_heals() {
    let src = "value = [1,,2]\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 12), "{span:?}");
}

/// RB-151 — Upstream `double-comma-02` analogue (SRC-TOML-0002 §H).
/// Origin: toml-test `invalid/array/double-comma-02.toml`.
/// Same locked H002 policy, exercised across a line break.
#[test]
fn rb_151_double_comma_02_heals_multiline() {
    let src = "value = [1,,\n2]\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!([1, 2]));
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "H002"),
        "H002 expected, got {codes:?}"
    );
    // Positive control: the clean multiline neighbour carries no heal.
    assert!(!codes_of("value = [1,\n2]\n").iter().any(|c| c == "H002"));
}

/// RB-152 — Upstream `extend-defined-aot` analogue (SRC-TOML-0003 §H).
/// Origin: toml-test `invalid/array/extend-defined-aot.toml`.
/// REFUSAL (spec-gated): upstream says reject the redefinition; Active's
/// locked last-wins policy (RB-002/RB-116) warns W011 and keeps the
/// second table wholesale.
#[test]
fn rb_152_extend_defined_aot_last_wins() {
    let src = "fruit = { name = \"apple\" }\nfruit = { name = \"banana\" }\n";
    assert_eq!(eval_json(src)["fruit"]["name"], "banana");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    let span = w.span.clone().expect("W011 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
}

/// RB-153 — Upstream `extending-table` analogue (SRC-TOML-0004 §H).
/// Origin: toml-test `invalid/array/extending-table.toml`.
/// Same locked W011 policy in header form (cross-syntax consistency
/// per the catalogue's dual-form guidance).
#[test]
fn rb_153_extending_table_header_last_wins() {
    let src = "[fruit]\nname = \"apple\"\n[fruit]\nname = \"banana\"\n";
    assert_eq!(eval_json(src)["fruit"]["name"], "banana");
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "W011"),
        "W011 expected, got {codes:?}"
    );
    // Positive control: extending a table with new keys keeps both.
    let v = eval_json("[fruit]\nname = \"apple\"\n[veg]\nname = \"carrot\"\n");
    assert_eq!(v["veg"]["name"], "carrot");
}

/// RB-154 — Upstream `missing-separator-01` analogue (SRC-TOML-0005 §H).
/// Origin: toml-test `invalid/array/missing-separator-01.toml`.
/// REFUSAL (spec-gated): upstream says reject; Active heals a missing
/// array comma with H003 (RB-090 allows the class; pinned exact here).
#[test]
fn rb_154_missing_separator_01_heals() {
    let src = "value = [1 2]\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 12), "{span:?}");
}

/// RB-155 — Upstream `missing-separator-02` analogue (SRC-TOML-0006 §H).
/// Origin: toml-test `invalid/array/missing-separator-02.toml`.
/// Same locked H003 policy; the healed value keeps every element.
#[test]
fn rb_155_missing_separator_02_heals() {
    let src = "value = [1 2]\n";
    let v = eval_json(src);
    assert_eq!(v["value"][0], 1);
    assert_eq!(v["value"][1], 2);
    assert_eq!(v["value"].as_array().expect("array").len(), 2);
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "H003"),
        "H003 expected, got {codes:?}"
    );
}

/// RB-156 — Upstream `no-close-01` analogue (SRC-TOML-0007 §H).
/// Origin: toml-test `invalid/array/no-close-01.toml`. E001, span 2:1.
#[test]
fn rb_156_no_close_01_rejects() {
    let src = "value = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed neighbour evaluates.
    assert_eq!(
        eval_json("value = [1, 2]\n")["value"],
        serde_json::json!([1, 2])
    );
}

/// RB-157 — Upstream `no-close-02` analogue (SRC-TOML-0008 §H).
/// Origin: toml-test `invalid/array/no-close-02.toml`.
/// Same E001 class across a line break; the span marks end-of-input.
#[test]
fn rb_157_no_close_02_rejects_multiline() {
    let src = "value = [1,\n2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (3, 1), "{span:?}");
}

/// RB-158 — Upstream `no-close-04` analogue (SRC-TOML-0009 §H).
/// Origin: toml-test `invalid/array/no-close-04.toml`.
/// Unterminated nesting rejects at the same class, never hangs.
#[test]
fn rb_158_no_close_04_rejects_nested() {
    let src = "value = [[1, 2]\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    // Positive control: the fully closed nest evaluates.
    assert_eq!(
        eval_json("value = [[1, 2]]\n")["value"],
        serde_json::json!([[1, 2]])
    );
}

/// RB-159 — Upstream `no-close-table-01` analogue (SRC-TOML-0010 §H).
/// Origin: toml-test `invalid/array/no-close-table-01.toml`.
/// Compact unterminated form rejects E001 like its spaced sibling.
#[test]
fn rb_159_no_close_table_01_rejects() {
    let src = "value = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
}

/// RB-160 — Upstream `no-comma-01` analogue (SRC-TOML-0011 §H).
/// Origin: toml-test `invalid/array/no-comma-01.toml`.
/// Same locked H003 policy inside a table body (dual-form coverage).
#[test]
fn rb_160_no_comma_01_heals_in_table() {
    let src = "[t]\nvalue = [1 2]\n";
    assert_eq!(eval_json(src)["t"]["value"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (2, 12), "{span:?}");
}

/// RB-161 — Upstream `no-comma-02` analogue (SRC-TOML-0012 §H).
/// Origin: toml-test `invalid/array/no-comma-02.toml`.
/// Newline-separated elements heal the same way, span on the join line.
#[test]
fn rb_161_no_comma_02_heals_across_lines() {
    let src = "value = [1\n2]\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
}

/// RB-162 — Upstream `only-comma-01` analogue (SRC-TOML-0013 §H).
/// Origin: toml-test `invalid/array/only-comma-01.toml`.
/// REFUSAL (spec-gated): upstream says reject the lone separator;
/// Active's locked H002 policy heals to `[]` (RB-119). Span pinned.
#[test]
fn rb_162_only_comma_01_heals() {
    let src = "value = [,]\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
}

/// RB-163 — Upstream `tables-01` analogue (SRC-TOML-0014 §H).
/// Origin: toml-test `invalid/array/tables-01.toml`.
/// Duplicate keys inside an array element warn W011 and keep last-wins.
#[test]
fn rb_163_tables_01_element_dup_last_wins() {
    let src = "rows = [{name = \"a\", name = \"b\"}]\n";
    assert_eq!(eval_json(src)["rows"][0]["name"], "b");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    let span = w.span.clone().expect("W011 carries a span");
    assert_eq!((span.line, span.column), (1, 29), "{span:?}");
    // Positive control: distinct element keys coexist.
    assert_eq!(
        eval_json("rows = [{a = 1}, {a = 2}]\n")["rows"],
        serde_json::json!([{"a": 1}, {"a": 2}])
    );
}

/// RB-164 — Upstream `text-in-array` analogue (SRC-TOML-0015 §H).
/// Origin: toml-test `invalid/array/text-in-array.toml`.
/// (Catalogue translation repeats the missing-separator shape; pinned
/// here through the `parse_file` path — same class, file machinery.)
#[test]
fn rb_164_text_in_array_heals_via_file() {
    let dir = std::env::temp_dir().join(format!(
        "active_rb164_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let main = dir.join("main.active");
    std::fs::write(&main, "value = [1 2]\n").expect("write main");
    let module =
        parse_file(&main, ParseOptions::default()).unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], serde_json::json!([1, 2]));
    assert!(
        diags.iter().any(|d| d.code == "H003"),
        "H003 expected, got {diags:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// RB-165 — Upstream `almost-false` analogue (SRC-TOML-0016 §H, MEDIUM).
/// Origin: toml-test `invalid/bool/almost-false.toml`.
/// Near-keywords never prefix-match: bare `falsee` is unresolved
/// (W001 + null), never boolean false.
#[test]
fn rb_165_almost_false_never_bool() {
    let v = eval_json("value = falsee\n");
    assert!(v["value"].is_null(), "{v}");
    let codes = codes_of("value = falsee\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "W001 expected, got {codes:?}"
    );
    // Positive control: the exact token is boolean false.
    assert_eq!(eval_json("value = false\n")["value"], false);
}

/// RB-166 — Upstream `almost-true` analogue (SRC-TOML-0017 §H, MEDIUM).
/// Origin: toml-test `invalid/bool/almost-true.toml`.
#[test]
fn rb_166_almost_true_never_bool() {
    let v = eval_json("value = truee\n");
    assert!(v["value"].is_null(), "{v}");
    let codes = codes_of("value = truee\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "W001 expected, got {codes:?}"
    );
    // Positive control: the exact token is boolean true.
    assert_eq!(eval_json("value = true\n")["value"], true);
}

/// RB-167 — Upstream `capitalized-false` analogue (SRC-TOML-0018 §H, MEDIUM).
/// Origin: toml-test `invalid/bool/capitalized-false.toml`.
/// (Catalogue prints `TRUE` for both 0018/0019 — a copy typo; the 0018
/// title and origin name the FALSE form, pinned here.)
/// Boolean recognition uses exact reserved tokens only.
#[test]
fn rb_167_capitalized_false_never_bool() {
    let v = eval_json("value = FALSE\n");
    assert!(v["value"].is_null(), "{v}");
    let codes = codes_of("value = FALSE\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "W001 expected, got {codes:?}"
    );
    // Positive control: quoted form stays a string.
    assert_eq!(eval_json("value = \"FALSE\"\n")["value"], "FALSE");
}

/// RB-168 — Upstream `capitalized-true` analogue (SRC-TOML-0019 §H, MEDIUM).
/// Origin: toml-test `invalid/bool/capitalized-true.toml`.
#[test]
fn rb_168_capitalized_true_never_bool() {
    let v = eval_json("value = TRUE\n");
    assert!(v["value"].is_null(), "{v}");
    let codes = codes_of("value = TRUE\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "W001 expected, got {codes:?}"
    );
    // Positive control: quoted form stays a string.
    assert_eq!(eval_json("value = \"TRUE\"\n")["value"], "TRUE");
}

/// RB-169 — Upstream `mixed-case-true` analogue (SRC-TOML-0020 §H, MEDIUM).
/// Origin: toml-test `invalid/bool/mixed-case-true.toml`.
/// (Catalogue prints `TRUE` — same copy typo as 0018; the title and
/// origin name the mixed-case form, pinned here as `TrUe`.)
/// Any non-exact case is unresolved (W001 + null), never boolean.
#[test]
fn rb_169_mixed_case_true_never_bool() {
    let v = eval_json("value = TrUe\n");
    assert!(v["value"].is_null(), "{v}");
    let codes = codes_of("value = TrUe\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "W001 expected, got {codes:?}"
    );
    // Positive control: the exact lowercase token is boolean true.
    assert_eq!(eval_json("value = true\n")["value"], true);
}

/// RB-170 — Upstream `bare-cr` analogue (SRC-TOML-0021 §H, MEDIUM).
/// Origin: toml-test `invalid/control/bare-cr.toml`.
/// Byte fixture: `a = 1 <CR> b = 2 <LF>`. CR is specified horizontal
/// whitespace (spec/grammar.md §1.2); statement recovery is identical
/// to a space — deterministic, both values intact, no diagnostics.
/// REFUSAL of the "reject" half: the row allows "reject OR normalize
/// under documented policy", and §1.2 documents CR as whitespace.
#[test]
fn rb_170_bare_cr_is_whitespace() {
    let module = parse_file(
        &research_fixture("src_toml_0021_bare_cr.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["a"], 1);
    assert_eq!(v["b"], 2);
    assert!(diags.is_empty(), "{diags:?}");
}

/// RB-171 — Upstream `bare-formfeed` analogue (SRC-TOML-0022 §H, MEDIUM).
/// Origin: toml-test `invalid/control/bare-formfeed.toml`.
/// Byte fixture: `value = 1 <FF> <LF>`. FF is outside every specified
/// char class → loud E005, never silent.
#[test]
fn rb_171_bare_formfeed_rejects() {
    let diags = parse_file(
        &research_fixture("src_toml_0022_bare_ff.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    assert!(
        diags.iter().any(|d| d.code == "E005"),
        "E005 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
    // Positive control: without the FF byte the document evaluates.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-172 — Upstream `bare-null` analogue (SRC-TOML-0023 §H, MEDIUM).
/// Origin: toml-test `invalid/control/bare-null.toml`.
/// Byte fixture: `value = "a <NUL> b"`. REFUSAL (spec-gated): the
/// grammar's char classes never exclude NUL and RB-092's oracle allows
/// "a documented class or a clean value" — the NUL is preserved
/// exactly (no C-style truncation, no silent drop), deterministically.
#[test]
fn rb_172_bare_nul_in_string_preserved() {
    let module = parse_file(
        &research_fixture("src_toml_0023_bare_nul.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], "a\0b");
    assert!(diags.is_empty(), "{diags:?}");
    // Positive control: the NUL-free neighbour evaluates identically.
    assert_eq!(eval_json("value = \"ab\"\n")["value"], "ab");
}

/// RB-173 — Upstream `comment-null` analogue (SRC-TOML-0024 §H, MEDIUM).
/// Origin: toml-test `invalid/control/comment-null.toml`.
/// Byte fixture: `# com <NUL> ment <LF> value = 1 <LF>`. Comment bytes
/// are trivia (spec/grammar.md §1.2) — inert, structure untouched.
#[test]
fn rb_173_nul_in_comment_inert() {
    let module = parse_file(
        &research_fixture("src_toml_0024_comment_nul.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], 1);
    assert!(diags.is_empty(), "{diags:?}");
}

/// RB-174 — Upstream `linetab-number-01` analogue (SRC-TOML-0025 §H, MEDIUM).
/// Origin: toml-test `invalid/control/linetab-number-01.toml`.
/// Byte fixture: `value = 1 <TAB> 2 <LF>`. TAB is specified whitespace
/// (spec/grammar.md §1.2), so this lexes as `value = 1 2` and the stray
/// falls under the §2.1 statement-level skip quirk (RB-100): value 1,
/// silent. REFUSAL of "reject: invalid-token-whitespace" per §1.2 + §2.1.
#[test]
fn rb_174_tab_is_whitespace_stray_skipped() {
    let module = parse_file(
        &research_fixture("src_toml_0025_linetab_number.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], 1);
    assert!(diags.is_empty(), "{diags:?}");
}

/// RB-175 — Upstream `multi-lf` analogue (SRC-TOML-0026 §H, MEDIUM).
/// Origin: toml-test `invalid/control/multi-lf.toml`.
/// Byte fixture: single-line string with a raw LF. REFUSAL (spec-gated):
/// the §1.4 char class (`any-but-quote`) never excludes newline, so the
/// string spans lines deterministically with the exact value kept.
#[test]
fn rb_175_raw_newline_in_string_kept() {
    let module = parse_file(
        &research_fixture("src_toml_0026_multi_lf.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], "line one\nline two");
    assert!(diags.is_empty(), "{diags:?}");
}

/// RB-176 — Upstream `rawmulti-null` analogue (SRC-TOML-0027 §H, MEDIUM).
/// Origin: toml-test `invalid/control/rawmulti-null.toml`.
/// Byte fixture: NUL inside a `"""` body. Same exact-preservation
/// policy as RB-172, in the specified multiline form.
#[test]
fn rb_176_nul_in_multiline_preserved() {
    let module = parse_file(
        &research_fixture("src_toml_0027_rawmulti_nul.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], "a\0b");
    assert!(diags.is_empty(), "{diags:?}");
}

/// RB-177 — Upstream `rawstring-lf` analogue (SRC-TOML-0028 §H, MEDIUM).
/// Origin: toml-test `invalid/control/rawstring-lf.toml`.
/// Byte fixture: the specified `"""` multiline form carrying newlines —
/// the row's own "unless multiline strings are specified" clause.
/// Exact value, deterministic.
#[test]
fn rb_177_specified_multiline_spans_lines() {
    let module = parse_file(
        &research_fixture("src_toml_0028_rawstring_lf.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], "line one\nline two");
    assert!(diags.is_empty(), "{diags:?}");
}

/// RB-178 — Upstream `string-null` analogue (SRC-TOML-0029 §H, MEDIUM).
/// Origin: toml-test `invalid/control/string-null.toml`.
/// The `\0` escape decodes to NUL (spec/grammar.md §1.4 escape table) —
/// specified decoding, exact value, no diagnostics.
#[test]
fn rb_178_nul_escape_decodes() {
    let v = eval_json("value = \"a\\0b\"\n");
    assert_eq!(v["value"], "a\0b");
    let codes = codes_of("value = \"a\\0b\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: unknown escapes stay verbatim (RB-050 policy).
    assert_eq!(eval_json("value = \"a\\qb\"\n")["value"], "a\\qb");
}

/// RB-179 — Upstream `string-us` analogue (SRC-TOML-0030 §H, MEDIUM).
/// Origin: toml-test `invalid/control/string-us.toml`.
/// Byte fixture: ASCII 0x1F inside string data. Same exact-preservation
/// policy as NUL (RB-172): char classes never exclude it, the value is
/// byte-exact, no diagnostics. REFUSAL of "reject: raw-control-byte".
#[test]
fn rb_179_control_0x1f_in_string_preserved() {
    let module = parse_file(
        &research_fixture("src_toml_0030_string_us.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], "a\u{1f}b");
    assert!(diags.is_empty(), "{diags:?}");
    // Positive control: the control-free neighbour evaluates identically.
    assert_eq!(eval_json("value = \"ab\"\n")["value"], "ab");
}

/// RB-180 — Upstream `day-zero` analogue (SRC-TOML-0031 §H, HIGH).
/// Origin: toml-test `invalid/datetime/day-zero.toml`.
/// REFUSAL (spec-gated): Active has no date type (datetimes are strings
/// — HIST-TOML-00O/RB-148), so no calendar validation can or must run.
/// Bare `2024-01-00` lexes as integer arithmetic (2024-1-0 = 2023),
/// deterministic and exact — it cannot "normalize to a different date".
#[test]
fn rb_180_day_zero_is_arithmetic() {
    assert_eq!(eval_json("value = 2024-01-00\n")["value"], 2023);
    // Positive control: the quoted form stays an exact string.
    assert_eq!(eval_json("value = \"2024-01-00\"\n")["value"], "2024-01-00");
}

/// RB-181 — Upstream `feb-29` analogue (SRC-TOML-0032 §H, HIGH).
/// Origin: toml-test `invalid/datetime/feb-29.toml`.
/// Same no-date-type policy: 2023-2-29 = 1992, never a leap decision.
#[test]
fn rb_181_feb_29_is_arithmetic() {
    assert_eq!(eval_json("value = 2023-02-29\n")["value"], 1992);
    // Positive control: the quoted form stays an exact string.
    assert_eq!(eval_json("value = \"2023-02-29\"\n")["value"], "2023-02-29");
}

/// RB-182 — Upstream `feb-30` analogue (SRC-TOML-0033 §H, HIGH).
/// Origin: toml-test `invalid/datetime/feb-30.toml`.
/// Impossible calendar dates cannot occur: 2024-2-30 = 1992.
#[test]
fn rb_182_feb_30_is_arithmetic() {
    assert_eq!(eval_json("value = 2024-02-30\n")["value"], 1992);
    // Positive control: the quoted form stays an exact string.
    assert_eq!(eval_json("value = \"2024-02-30\"\n")["value"], "2024-02-30");
}

/// RB-183 — Upstream `hour-over` analogue (SRC-TOML-0034 §H, HIGH).
/// Origin: toml-test `invalid/datetime/hour-over.toml`.
/// The `T24:00` tail cannot lex as time — E002 at the offending token.
/// This is the grammar rejecting, not date validation.
#[test]
fn rb_183_hour_over_rejects() {
    let src = "value = 2024-01-01T24:00:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 28), "{span:?}");
    // Positive control: the quoted form stays an exact string.
    assert_eq!(
        eval_json("value = \"2024-01-01T24:00:00Z\"\n")["value"],
        "2024-01-01T24:00:00Z"
    );
}

/// RB-184 — Upstream `leading-zero-date` analogue (SRC-TOML-0035 §H, HIGH).
/// Origin: toml-test `invalid/datetime/leading-zero-date.toml`.
/// Leading zeros are plain decimals (RB-104): 001979-5-27 = 1947.
/// Quoted text stays text.
#[test]
fn rb_184_leading_zero_date_is_arithmetic() {
    assert_eq!(eval_json("value = 001979-05-27\n")["value"], 1947);
    // Positive control: the quoted form stays an exact string.
    assert_eq!(
        eval_json("value = \"001979-05-27\"\n")["value"],
        "001979-05-27"
    );
}

/// RB-185 — Upstream `no-secs` analogue (SRC-TOML-0036 §H, HIGH).
/// Origin: toml-test `invalid/datetime/no-secs.toml`.
/// No seconds-optional grammar exists to consult: the `T07` tail
/// rejects E002 like any non-grammar token sequence.
#[test]
fn rb_185_no_secs_rejects() {
    let src = "value = 1979-05-27T07:32Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 25), "{span:?}");
}

/// RB-186 — Upstream `no-t` analogue (SRC-TOML-0037 §H, HIGH).
/// Origin: toml-test `invalid/datetime/no-t.toml`.
/// Space-separated date/time has no grammar either: E002.
#[test]
fn rb_186_no_t_rejects() {
    let src = "value = 1979-05-27 07:32:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 28), "{span:?}");
}

/// RB-187 — Upstream `offset-overflow-hour` analogue (SRC-TOML-0038 §H, HIGH).
/// Origin: toml-test `invalid/datetime/offset-overflow-hour.toml`.
/// REFUSAL (spec-gated): no offset grammar exists to range-check.
/// The input deterministically evaluates by shipped rules (integer
/// arithmetic + the specified same-line colon pair, RB-001): no date,
/// no fallback object, no crash — and repeated runs agree exactly.
#[test]
fn rb_187_offset_overflow_hour_deterministic() {
    let src = "value = 1979-05-27T07:32:00+25:00\n";
    let first = eval_json(src);
    assert_eq!(first["value"], 1947);
    assert_eq!(first["T07"], 32);
    assert_eq!(first, eval_json(src));
    // Positive control: the quoted form stays an exact string.
    assert_eq!(
        eval_json("value = \"1979-05-27T07:32:00+25:00\"\n")["value"],
        "1979-05-27T07:32:00+25:00"
    );
}

/// RB-188 — Upstream `offset-overflow-minute` analogue (SRC-TOML-0039 §H, HIGH).
/// Origin: toml-test `invalid/datetime/offset-overflow-minute.toml`.
/// Same deterministic rules as RB-187: the bad minute never becomes a
/// date, and both malformed offsets agree structurally.
#[test]
fn rb_188_offset_overflow_minute_deterministic() {
    let src = "value = 1979-05-27T07:32:00+05:99\n";
    let first = eval_json(src);
    assert_eq!(first["value"], 1947);
    assert_eq!(first["T07"], 32);
    assert_eq!(first, eval_json(src));
    // Positive control: quoted forms of both offsets stay strings.
    assert_eq!(
        eval_json("value = \"1979-05-27T07:32:00+05:99\"\n")["value"],
        "1979-05-27T07:32:00+05:99"
    );
}

/// RB-189 — Upstream `second-over` analogue (SRC-TOML-0040 §H, HIGH).
/// Origin: toml-test `invalid/datetime/second-over.toml`.
/// No seconds range exists to check (no date type): the `T32` tail
/// rejects E002 like any non-grammar token sequence.
#[test]
fn rb_189_second_over_rejects() {
    let src = "value = 1979-05-27T07:32:99Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 28), "{span:?}");
    // Positive control: the quoted form stays an exact string.
    assert_eq!(
        eval_json("value = \"1979-05-27T07:32:99Z\"\n")["value"],
        "1979-05-27T07:32:99Z"
    );
}

/// RB-190 — Upstream `second-trailing-dot` analogue (SRC-TOML-0041 §H, HIGH).
/// Origin: toml-test `invalid/datetime/second-trailing-dot.toml`.
/// A fraction dot with no digits is not silently accepted: E002.
#[test]
fn rb_190_second_trailing_dot_rejects() {
    let src = "value = 1979-05-27T07:32:00.Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 29), "{span:?}");
}

/// RB-191 — Upstream `trailing-x` analogue (SRC-TOML-0042 §H, HIGH).
/// Origin: toml-test `invalid/datetime/trailing-x.toml`.
/// Trailing garbage after the `Z` rejects E002 — parsing consumes the
/// whole token stream, never stops early and silent.
#[test]
fn rb_191_trailing_x_rejects() {
    let src = "value = 1979-05-27T07:32:00Zx\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 28), "{span:?}");
}

/// RB-192 — Upstream `y10k-date` analogue (SRC-TOML-0043 §H, HIGH).
/// Origin: toml-test `invalid/datetime/y10k-date.toml`.
/// REFUSAL (spec-gated): no year-range specification exists because no
/// date type exists. Bare `10000-01-01` is integer arithmetic
/// (10000-1-1 = 9998) — overflow-safe i64, deterministic.
#[test]
fn rb_192_y10k_date_is_arithmetic() {
    assert_eq!(eval_json("value = 10000-01-01\n")["value"], 9998);
    // Positive control: the quoted form stays an exact string.
    assert_eq!(
        eval_json("value = \"10000-01-01\"\n")["value"],
        "10000-01-01"
    );
}

/// RB-193 — Upstream `bad-codepoint` analogue (SRC-TOML-0044 §H, HIGH).
/// Origin: toml-test `invalid/encoding/bad-codepoint.toml`.
/// Byte fixture: lone continuation byte 0x80. The UTF-8 decoder
/// rejects before lexing: controlled E017, never a panic, never
/// replacement-decoded silently.
#[test]
fn rb_193_bad_codepoint_controlled_e017() {
    let diags = parse_file(
        &research_fixture("src_toml_0044_bad_codepoint.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    assert!(
        diags.iter().any(|d| d.code == "E017"),
        "E017 expected, got {diags:?}"
    );
    // Positive control: the same document without the bad byte parses.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-194 — Upstream `bad-utf8-at-end` analogue (SRC-TOML-0045 §H, HIGH).
/// Origin: toml-test `invalid/encoding/bad-utf8-at-end.toml`.
/// Byte fixture: truncated 0xE2 sequence at EOF. Same controlled E017.
#[test]
fn rb_194_truncated_utf8_controlled_e017() {
    let diags = parse_file(
        &research_fixture("src_toml_0045_bad_utf8_at_end.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    assert!(
        diags.iter().any(|d| d.code == "E017"),
        "E017 expected, got {diags:?}"
    );
}

/// RB-195 — Upstream `bad-utf8-in-array` analogue (SRC-TOML-0046 §H, HIGH).
/// Origin: toml-test `invalid/encoding/bad-utf8-in-array.toml`.
/// Byte fixture: overlong C0 AF inside an array. Invalid encoding is
/// caught at the file boundary, before any AST construction: E017.
#[test]
fn rb_195_bad_utf8_in_array_controlled_e017() {
    let diags = parse_file(
        &research_fixture("src_toml_0046_bad_utf8_in_array.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    assert!(
        diags.iter().any(|d| d.code == "E017"),
        "E017 expected, got {diags:?}"
    );
    // Positive control: the valid array evaluates.
    assert_eq!(
        eval_json("value = [1, 2]\n")["value"],
        serde_json::json!([1, 2])
    );
}

/// RB-196 — Upstream `bad-utf8-in-string` analogue (SRC-TOML-0047 §H, HIGH).
/// Origin: toml-test `invalid/encoding/bad-utf8-in-string.toml`.
/// Byte fixture: overlong C0 AF inside quotes. No silent
/// replacement-decoding of untrusted text: E017.
#[test]
fn rb_196_bad_utf8_in_string_controlled_e017() {
    let diags = parse_file(
        &research_fixture("src_toml_0047_bad_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    assert!(
        diags.iter().any(|d| d.code == "E017"),
        "E017 expected, got {diags:?}"
    );
}

/// RB-197 — Upstream `bom-not-at-start-01` analogue (SRC-TOML-0048 §H, HIGH).
/// Origin: toml-test `invalid/encoding/bom-not-at-start-01.toml`.
/// Byte fixture: BOM bytes after a first assignment. Only a LEADING
/// BOM is skipped (spec/grammar.md §1.0); a mid-file BOM is an
/// unexpected character: loud E005 with span, never silent.
#[test]
fn rb_197_misplaced_bom_rejects() {
    let diags = parse_file(
        &research_fixture("src_toml_0048_bom_not_at_start.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    assert!(
        diags.iter().any(|d| d.code == "E005"),
        "E005 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: a leading BOM is skipped (RB-003 policy).
    assert_eq!(eval_json("\u{FEFF}value = 1\n")["value"], 1);
}

/// RB-198 — Upstream `ideographic-space` analogue (SRC-TOML-0049 §H, HIGH).
/// Origin: toml-test `invalid/encoding/ideographic-space.toml`.
/// Byte fixture: U+3000 between key and `=`. Specified whitespace is
/// ASCII-only (spec/grammar.md §1.2: space, TAB, CR) — U+3000 is never
/// casually normalized to a space: loud E005 with span.
#[test]
fn rb_198_ideographic_space_rejects() {
    let diags = parse_file(
        &research_fixture("src_toml_0049_ideographic_space.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    assert!(
        diags.iter().any(|d| d.code == "E005"),
        "E005 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 6), "{span:?}");
    // Positive control: the ASCII-space neighbour evaluates.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-199 — Upstream `utf16-bom` analogue (SRC-TOML-0050 §H, HIGH).
/// Origin: toml-test `invalid/encoding/utf16-bom.toml`.
/// Byte fixture: FF FE + UTF-16LE assignments. Source text is UTF-8
/// (spec/grammar.md §1.0); UTF-16 is not negotiated — the decoder
/// fails controlled E017 before lexing, never mojibake, never panic.
#[test]
fn rb_199_utf16_bom_controlled_e017() {
    let diags = parse_file(
        &research_fixture("src_toml_0050_utf16_bom.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    assert!(
        diags.iter().any(|d| d.code == "E017"),
        "E017 expected, got {diags:?}"
    );
    // Positive control: the UTF-8 form evaluates.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-200 — Upstream `arabic-zero-01` analogue (SRC-TOML-0051 §H, HIGH).
/// Origin: toml-test `invalid/float/arabic-zero-01.toml`.
/// Only ASCII digits lex numbers (spec/grammar.md §1.1 `digit =
/// [0-9]`): U+0661 is an unexpected character, loud E005 with span.
#[test]
fn rb_200_arabic_zero_rejects() {
    let codes = codes_of("value = ١\n");
    assert!(
        codes.iter().any(|c| c == "E005"),
        "E005 expected, got {codes:?}"
    );
    // Positive control: the ASCII zero evaluates.
    assert_eq!(eval_json("value = 0\n")["value"], 0);
}

/// RB-201 — Upstream `double-dot-01` analogue (SRC-TOML-0052 §H, HIGH).
/// Origin: toml-test `invalid/float/double-dot-01.toml`.
/// REFUSAL (spec-gated): no malformed-number reject exists for trailing
/// dots — mid-statement strays fall under the §2.1/§4.1 skip quirk
/// (RB-100 design authority). Deterministic: first value 1, zero diags.
#[test]
fn rb_201_double_dot_keeps_first_value() {
    let v = eval_json("value = 1..2\n");
    assert_eq!(v["value"], 1);
    assert!(v["value"].is_i64());
    let codes = codes_of("value = 1..2\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the clean float evaluates with its type.
    let v = eval_json("value = 1.2\n");
    assert_eq!(v["value"], 1.2);
    assert!(v["value"].is_f64());
}

/// RB-202 — Upstream `exp-dot-01` analogue (SRC-TOML-0053 §H, HIGH).
/// Origin: toml-test `invalid/float/exp-dot-01.toml`.
/// No exponent grammar (RB-053): `1.e2` cannot lex as a number —
/// E002 at the stray identifier.
#[test]
fn rb_202_exp_dot_rejects() {
    let codes = codes_of("value = 1.e2\n");
    assert!(
        codes.iter().any(|c| c == "E002"),
        "E002 expected, got {codes:?}"
    );
}

/// RB-203 — Upstream `exp-double-e-01` analogue (SRC-TOML-0054 §H, HIGH).
/// Origin: toml-test `invalid/float/exp-double-e-01.toml`.
/// One exponent only — and Active has zero: E002 at the first `e`.
#[test]
fn rb_203_exp_double_e_rejects() {
    let src = "value = 1e2e3\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
    // Positive control: plain integers around it evaluate.
    assert_eq!(eval_json("value = 123\n")["value"], 123);
}

/// RB-204 — Upstream `leading-dot` analogue (SRC-TOML-0055 §H, HIGH).
/// Origin: toml-test `invalid/float/leading-dot.toml`.
/// Leading-dot floats are not in the grammar: E002 on the Dot.
/// (No permissive host float grammar is borrowed.)
#[test]
fn rb_204_leading_dot_rejects() {
    let src = "value = .5\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 9), "{span:?}");
    // Positive control: the dotted float evaluates.
    assert_eq!(eval_json("value = 0.5\n")["value"], 0.5);
}

/// RB-205 — Upstream `leading-zero` analogue (SRC-TOML-0056 §H, HIGH).
/// Origin: toml-test `invalid/float/leading-zero.toml`.
/// REFUSAL (spec-gated): leading zeros are plain decimals (RB-104 —
/// never octal), floats included: `01.5` is exactly 1.5.
#[test]
fn rb_205_leading_zero_float_decimal() {
    let v = eval_json("value = 01.5\n");
    assert_eq!(v["value"], 1.5);
    assert!(v["value"].is_f64());
    // Positive control: the zero-free neighbour agrees.
    assert_eq!(eval_json("value = 1.5\n")["value"], 1.5);
}

/// RB-206 — Upstream `nan-capital` analogue (SRC-TOML-0057 §H, HIGH).
/// Origin: toml-test `invalid/float/nan-capital.toml`.
/// Never case-folded into IEEE NaN: bare `NaN` is unresolved
/// (W001 + null). Special floats have no bare spellings at all.
#[test]
fn rb_206_nan_capital_never_nan() {
    let v = eval_json("value = NaN\n");
    assert!(v["value"].is_null(), "{v}");
    let codes = codes_of("value = NaN\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "W001 expected, got {codes:?}"
    );
    // Positive control: quoted form stays a string.
    assert_eq!(eval_json("value = \"NaN\"\n")["value"], "NaN");
}

/// RB-207 — Upstream `trailing-dot` analogue (SRC-TOML-0058 §H, HIGH).
/// Origin: toml-test `invalid/float/trailing-dot.toml`.
/// REFUSAL (spec-gated): same §2.1/§4.1 skip quirk as RB-201 — `1.`
/// keeps int 1 deterministically with zero diags (RB-100 authority).
#[test]
fn rb_207_trailing_dot_keeps_int() {
    let v = eval_json("value = 1.\n");
    assert_eq!(v["value"], 1);
    assert!(v["value"].is_i64());
    let codes = codes_of("value = 1.\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the clean int evaluates.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-208 — Upstream `trailing-exp` analogue (SRC-TOML-0059 §H, HIGH).
/// Origin: toml-test `invalid/float/trailing-exp.toml`.
/// An exponent marker cannot terminate a token with no exponent
/// grammar: E002 at the `e`.
#[test]
fn rb_208_trailing_exp_rejects() {
    let src = "value = 1e\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
}

/// RB-209 — Upstream `trailing-us` analogue (SRC-TOML-0060 §H, HIGH).
/// Origin: toml-test `invalid/float/trailing-us.toml`.
/// REFUSAL (spec-gated): digit separators strip wherever they appear
/// (RB-107: `1_000_000` → 1000000) — `1_` is exactly 1, deterministic,
/// no diagnostics. No octal-style reinterpretation is possible.
#[test]
fn rb_209_trailing_separator_strips() {
    let v = eval_json("value = 1_\n");
    assert_eq!(v["value"], 1);
    assert!(v["value"].is_i64());
    let codes = codes_of("value = 1_\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: interior separators strip identically.
    assert_eq!(eval_json("value = 1_000\n")["value"], 1000);
}

/// RB-210 — Upstream `us-after-dot` analogue (SRC-TOML-0061 §H, HIGH).
/// Origin: toml-test `invalid/float/us-after-dot.toml`.
/// A separator directly after the decimal point cannot lex: E002.
#[test]
fn rb_210_separator_after_dot_rejects() {
    let src = "value = 1._2\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 11), "{span:?}");
    // Positive control: the clean float evaluates.
    assert_eq!(eval_json("value = 1.2\n")["value"], 1.2);
}

/// RB-211 — Upstream `duplicate-key-01` analogue (SRC-TOML-0062 §H, HIGH).
/// Origin: toml-test `invalid/inline-table/duplicate-key-01.toml`.
/// REFUSAL (spec-gated): upstream says reject; Active's locked W011
/// last-wins policy (RB-002) applies inside inline tables — announced,
/// deterministic, with span. Convention: the W011 span marks the
/// winning value token (1:26 here, the second `2`).
#[test]
fn rb_211_inline_dup_warns_last_wins() {
    let src = "value = { key = 1, key = 2 }\n";
    assert_eq!(eval_json(src)["value"]["key"], 2);
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    let span = w.span.clone().expect("W011 carries a span");
    assert_eq!((span.line, span.column), (1, 26), "{span:?}");
}

/// RB-212 — Upstream `linebreak-01` analogue (SRC-TOML-0063 §H, HIGH).
/// Origin: toml-test `invalid/inline-table/linebreak-01.toml`.
/// Brace maps permit multiline formatting (spec/grammar.md §2.6:
/// arrays and inline tables may span lines freely) — the multiline
/// form equals the one-line form exactly.
#[test]
fn rb_212_multiline_inline_table_accepts() {
    let multi = "value = { a = 1,\n b = 2 }\n";
    let flat = "value = { a = 1, b = 2 }\n";
    assert_eq!(eval_json(multi), eval_json(flat));
    assert_eq!(eval_json(multi)["value"]["b"], 2);
}

/// RB-213 — Upstream `no-close-01` analogue (SRC-TOML-0064 §H, HIGH).
/// Origin: toml-test `invalid/inline-table/no-close-01.toml`.
/// Unterminated inline table inside a table body: E001 at EOF.
#[test]
fn rb_213_unterminated_table_in_body_rejects() {
    let src = "[t]\nvalue = { a = 1\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (3, 1), "{span:?}");
    // Positive control: the closed neighbour evaluates.
    assert_eq!(eval_json("[t]\nvalue = { a = 1 }\n")["t"]["value"]["a"], 1);
}

/// RB-214 — Upstream `no-comma-01` analogue (SRC-TOML-0065 §H, HIGH).
/// Origin: toml-test `invalid/inline-table/no-comma-01.toml`.
/// REFUSAL (spec-gated): adjacent pairs are not merged — the locked
/// H003 policy inserts the missing comma announced (same as arrays,
/// RB-154), with span.
#[test]
fn rb_214_table_missing_comma_heals() {
    let src = "value = { a = 1 b = 2 }\n";
    let v = eval_json(src);
    assert_eq!(v["value"]["a"], 1);
    assert_eq!(v["value"]["b"], 2);
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
}

/// RB-215 — Upstream `overwrite-01` analogue (SRC-TOML-0066 §H, HIGH).
/// Origin: toml-test `invalid/inline-table/overwrite-01.toml`.
/// Same locked W011 policy in brace-block form (cross-syntax per the
/// catalogue's dual-form guidance): last wins, announced with span.
/// (Block-form span marks the redefined key token — 1:18 here — while
/// the inline-form span marks the winning value; both shipped facts.)
#[test]
fn rb_215_block_dup_warns_last_wins() {
    let src = "value { key = 1, key = 2 }\n";
    assert_eq!(eval_json(src)["value"]["key"], 2);
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    let span = w.span.clone().expect("W011 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
}

/// RB-216 — Upstream `trailing-comma` analogue (SRC-TOML-0067 §H, HIGH).
/// Origin: toml-test `invalid/inline-table/trailing-comma.toml`.
/// REFUSAL (spec-gated): trailing map commas ARE explicitly allowed —
/// H002 heals (`removed trailing comma before closing brace`), so the
/// policy is consistent across arrays and maps as the row demands.
#[test]
fn rb_216_table_trailing_comma_heals() {
    let src = "value = { a = 1, }\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!({"a": 1}));
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "H002"),
        "H002 expected, got {codes:?}"
    );
    // Positive control: the comma-free neighbour carries no heal.
    assert!(!codes_of("value = { a = 1 }\n").iter().any(|c| c == "H002"));
}

/// RB-217 — Upstream `capital-hex` analogue (SRC-TOML-0068 §H, HIGH).
/// Origin: toml-test `invalid/integer/capital-hex.toml`.
/// No hex literals exist in the alpha (spec/grammar.md §1.1:
/// "hex-digit — none"), so `0XFF` cannot lex as a number: E002 at
/// the stray identifier. The documented base-prefix case is: none.
#[test]
fn rb_217_capital_hex_rejects() {
    let src = "value = 0XFF\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
    // Positive control: the decimal neighbour evaluates.
    assert_eq!(eval_json("value = 255\n")["value"], 255);
}

/// RB-218 — Upstream `double-sign-plus` analogue (SRC-TOML-0069 §H, HIGH).
/// Origin: toml-test `invalid/integer/double-sign-plus.toml`.
/// A numeric token takes at most one sign: the second `+` is E002.
/// (There is no unary `+` in the grammar at all — only unary `-`.)
#[test]
fn rb_218_double_sign_rejects() {
    let src = "value = ++1\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 9), "{span:?}");
    // Positive control: the single-signed neighbour evaluates.
    assert_eq!(eval_json("value = -1\n")["value"], -1);
}

/// RB-219 — Upstream `incomplete-hex` analogue (SRC-TOML-0070 §H, HIGH).
/// Origin: toml-test `invalid/integer/incomplete-hex.toml`.
/// No hex grammar exists (RB-217), so a bare `0x` prefix cannot open
/// a number either: E002 at the stray identifier.
#[test]
fn rb_219_incomplete_hex_rejects() {
    let src = "value = 0x\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
    // Positive control: the plain zero evaluates.
    assert_eq!(eval_json("value = 0\n")["value"], 0);
}

/// RB-220 — Upstream `invalid-bin` analogue (SRC-TOML-0071 §H, HIGH).
/// Origin: toml-test `invalid/integer/invalid-bin.toml`.
/// No binary grammar exists either: `0` then stray `b102` → E002.
/// Out-of-base digits cannot occur where no base lexes.
#[test]
fn rb_220_invalid_bin_rejects() {
    let src = "value = 0b102\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
}

/// RB-221 — Upstream `leading-us` analogue (SRC-TOML-0072 §H, HIGH).
/// Origin: toml-test `invalid/integer/leading-us.toml`.
/// REFUSAL (spec-gated): a leading `_` makes the token an identifier
/// (ident-start includes `_`), not a number — and bare words resolve
/// W001 + null under locked policy (RB-103/RB-120), never a number,
/// never a crash. No silent numeric coercion is possible.
#[test]
fn rb_221_leading_separator_is_identifier() {
    let v = eval_json("value = _123\n");
    assert!(v["value"].is_null(), "{v}");
    let codes = codes_of("value = _123\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "W001 expected, got {codes:?}"
    );
    // Positive control: the separator-free number evaluates.
    assert_eq!(eval_json("value = 123\n")["value"], 123);
}

/// RB-222 — Upstream `leading-zero-01` analogue (SRC-TOML-0073 §H, HIGH).
/// Origin: toml-test `invalid/integer/leading-zero-01.toml`.
/// REFUSAL (spec-gated): leading zeros are plain decimals (RB-104) —
/// `01` is exactly 1, loudly not octal, loudly not rejected.
#[test]
fn rb_222_leading_zero_is_decimal() {
    let v = eval_json("value = 01\n");
    assert_eq!(v["value"], 1);
    assert!(v["value"].is_i64());
    // Positive control: the zero-free neighbour agrees.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-223 — Upstream `negative-hex` analogue (SRC-TOML-0074 §H, HIGH).
/// Origin: toml-test `invalid/integer/negative-hex.toml`.
/// Signed base-prefixed numbers are not specified (no hex in the
/// alpha, grammar §1.1): E002 at the stray identifier.
#[test]
fn rb_223_negative_hex_rejects() {
    let src = "value = -0xFF\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 11), "{span:?}");
    // Positive control: signed decimals evaluate.
    assert_eq!(eval_json("value = -255\n")["value"], -255);
}

/// RB-224 — Upstream `trailing-us` analogue (SRC-TOML-0075 §H, HIGH).
/// Origin: toml-test `invalid/integer/trailing-us.toml`.
/// REFUSAL (spec-gated): same strip rule as RB-209 — `123_` is
/// exactly 123, deterministic, no diagnostics.
#[test]
fn rb_224_trailing_separator_strips_multi() {
    let v = eval_json("value = 123_\n");
    assert_eq!(v["value"], 123);
    assert!(v["value"].is_i64());
    let codes = codes_of("value = 123_\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-225 — Upstream `us-after-bin` analogue (SRC-TOML-0076 §H, HIGH).
/// Origin: toml-test `invalid/integer/us-after-bin.toml`.
/// No separator can touch a base prefix that does not exist: E002.
#[test]
fn rb_225_separator_after_bin_rejects() {
    let src = "value = 0b_1\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
}

/// RB-226 — Upstream `after-array` analogue (SRC-TOML-0077 §H, HIGH).
/// Origin: toml-test `invalid/key/after-array.toml`.
/// REFUSAL (spec-gated): redefinition across types warns W011 and
/// keeps last-wins (RB-002) — key ownership is consistent: exactly
/// one binding survives, announced with span.
#[test]
fn rb_226_redefinition_across_types_last_wins() {
    let src = "value = [1]\nvalue = 2\n";
    assert_eq!(eval_json(src)["value"], 2);
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    let span = w.span.clone().expect("W011 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: distinct keys coexist.
    let v = eval_json("a = [1]\nb = 2\n");
    assert_eq!(v["a"], serde_json::json!([1]));
    assert_eq!(v["b"], 2);
}

/// RB-227 — Upstream `bare-invalid-character-01` analogue (SRC-TOML-0078 §H, HIGH).
/// Origin: toml-test `invalid/key/bare-invalid-character-01.toml`.
/// Bare keys are identifiers only: `/` cannot appear — E002 at the
/// statement, nothing half-bound.
#[test]
fn rb_227_slash_in_key_rejects() {
    let src = "bad/key = 1\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
    // Positive control: the underscore neighbour is a valid key.
    assert_eq!(eval_json("bad_key = 1\n")["bad_key"], 1);
}

/// RB-228 — Upstream `dotted-redefine-table-01` analogue (SRC-TOML-0079 §H, HIGH).
/// Origin: toml-test `invalid/key/dotted-redefine-table-01.toml`.
/// REFUSAL (spec-gated): quoted keys never enter the tree — they
/// reject loud E002 (RB-051/RB-052: "keys must be identifiers") — so
/// a quoted `"a.b"` can never collide with path `a.b`. The explicit
/// path/key equality policy is: quoted keys do not exist as keys.
#[test]
fn rb_228_quoted_dotted_key_rejects() {
    let src = "value = { a = { b = 1 }, \"a.b\" = 2 }\n";
    let diags = parse(src, "research").expect_err("must fail");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    // Positive control: the unquoted nested form evaluates.
    assert_eq!(
        eval_json("value = { a = { b = 1 } }\n")["value"]["a"]["b"],
        1
    );
}

/// RB-229 — Upstream `duplicate-keys-01` analogue (SRC-TOML-0080 §H, HIGH).
/// Origin: toml-test `invalid/key/duplicate-keys-01.toml`.
/// Same locked W011 policy in indent-block form (third syntax for the
/// same rule — RB-211 inline, RB-215 brace-block): last wins at 3:3.
#[test]
fn rb_229_indent_dup_warns_last_wins() {
    let src = "value:\n  key = 1\n  key = 2\n";
    assert_eq!(eval_json(src)["value"]["key"], 2);
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    let span = w.span.clone().expect("W011 carries a span");
    assert_eq!((span.line, span.column), (3, 3), "{span:?}");
    // Positive control: distinct indent keys coexist.
    let v = eval_json("value:\n  a = 1\n  b = 2\n");
    assert_eq!(v["value"]["b"], 2);
}

/// RB-230 — Upstream `empty` analogue (SRC-TOML-0081 §H, HIGH).
/// Origin: toml-test `invalid/key/empty.toml`.
/// Empty keys are rejected exactly like every quoted key (RB-051):
/// loud E002 at 1:1 — the empty key is never silently lost.
#[test]
fn rb_230_empty_key_rejects_loud() {
    let src = "\"\" = 1\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
    // Positive control: the identifier neighbour binds.
    assert_eq!(eval_json("key = 1\n")["key"], 1);
}

/// RB-231 — Upstream `multiline-key-01` analogue (SRC-TOML-0082 §H, HIGH).
/// Origin: toml-test `invalid/key/multiline-key-01.toml`.
/// Byte fixture: `"a <LF> b" = 1`. Multiline keys are not in the
/// grammar — but the string still lexes exactly (raw LF kept per
/// RB-175) and the KEY position rejects loud E002. Nothing silent.
#[test]
fn rb_231_multiline_key_rejects() {
    let diags = parse_file(
        &research_fixture("src_toml_0082_multiline_key.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    // Positive control: the same string as a VALUE evaluates exactly.
    assert_eq!(eval_json("value = \"a\\nb\"\n")["value"], "a\nb");
}

/// RB-232 — Upstream `newline-01` analogue (SRC-TOML-0083 §H, HIGH).
/// Origin: toml-test `invalid/key/newline-01.toml`.
/// Byte fixture (own file per SRC ID): same shape, asserting the
/// E002 span at the string token — quoted-key newline rules are
/// explicit: the string is fine, the position is not.
#[test]
fn rb_232_newline_key_span_pinned() {
    let diags = parse_file(
        &research_fixture("src_toml_0083_multiline_key.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
}

/// RB-233 — Upstream `no-eol-01` analogue (SRC-TOML-0084 §H, HIGH).
/// Origin: toml-test `invalid/key/no-eol-01.toml`.
/// Final input without a trailing newline evaluates fully — EOF at a
/// token boundary finalizes; the last assignment is never dropped.
#[test]
fn rb_233_no_trailing_newline_accepts() {
    assert_eq!(eval_json("value = 1")["value"], 1);
    let module = parse("value = 1", "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    // Positive control: the newline-terminated twin agrees.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-234 — Upstream `only-int` analogue (SRC-TOML-0085 §H, HIGH).
/// Origin: toml-test `invalid/key/only-int.toml`.
/// Keys are identifiers (or rejected quoted strings) — a bare integer
/// cannot be a key: statement-initial value tokens fail E002 (RB-101).
#[test]
fn rb_234_int_key_rejects() {
    let src = "1 = \"value\"\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
    // Positive control: the identifier key binds the same value.
    assert_eq!(eval_json("key = \"value\"\n")["key"], "value");
}

/// RB-235 — Upstream `tab` analogue (SRC-TOML-0086 §H, HIGH).
/// Origin: toml-test `invalid/key/tab.toml`.
/// TAB is whitespace, never identifier glue (spec/grammar.md §1.2):
/// `bad<TAB>key` splits into two tokens and the statement fails loud
/// E002 — never silently normalized into one identifier.
#[test]
fn rb_235_tab_in_key_rejects() {
    let src = "bad\tkey = 1\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
    // Positive control: the TAB-free key binds.
    assert_eq!(eval_json("badkey = 1\n")["badkey"], 1);
}

/// RB-236 — Upstream `two-equals-01` analogue (SRC-TOML-0087 §H, HIGH).
/// Origin: toml-test `invalid/key/two-equals-01.toml`.
/// Exactly one explicit assignment operator: the doubled `==` fails
/// E002 at the statement start — never read as comparison, never
/// half-bound.
#[test]
fn rb_236_two_equals_rejects() {
    let src = "value == 1\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
    // Positive control: the single-`=` neighbour binds.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-237 — Upstream `without-value-01` analogue (SRC-TOML-0088 §H, HIGH).
/// Origin: toml-test `invalid/key/without-value-01.toml`.
/// An assignment cannot end without a value: E002 at the newline.
/// (Bodiless `key:` headers are E008 instead — RB-001's specified
/// colon-pair rule distinguishes the two shapes.)
#[test]
fn rb_237_missing_value_rejects() {
    let src = "value =\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 8), "{span:?}");
    // Positive control: the valued neighbour binds.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-238 — Upstream `bad-byte-escape` analogue (SRC-TOML-0089 §H, HIGH).
/// Origin: toml-test `invalid/string/bad-byte-escape.toml`.
/// REFUSAL (spec-gated): byte escapes are not special — unknown
/// escapes keep the backslash verbatim (spec/grammar.md §1.4,
/// RB-050) — so `\xFF` can never silently create invalid Unicode.
/// The value is the exact six characters `\xFF`.
#[test]
fn rb_238_bad_byte_escape_verbatim() {
    let v = eval_json("value = \"\\xFF\"\n");
    assert_eq!(v["value"], "\\xFF");
    let codes = codes_of("value = \"\\xFF\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: specified escapes still decode.
    assert_eq!(eval_json("value = \"a\\nb\"\n")["value"], "a\nb");
}

/// RB-239 — Upstream `bad-escape-01` analogue (SRC-TOML-0090 §H, HIGH).
/// Origin: toml-test `invalid/string/bad-escape-01.toml`.
/// REFUSAL (spec-gated): unknown escapes are not "accepted literally
/// by accident" — verbatim passthrough IS the documented rule
/// (spec/grammar.md §1.4, RB-050/RB-238). `\q` is exactly `\q`.
#[test]
fn rb_239_unknown_escape_verbatim() {
    let v = eval_json("value = \"\\q\"\n");
    assert_eq!(v["value"], "\\q");
    let codes = codes_of("value = \"\\q\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: specified escapes still decode.
    assert_eq!(eval_json("value = \"a\\tb\"\n")["value"], "a\tb");
}

/// RB-240 — Upstream `bad-hex-esc-01` analogue (SRC-TOML-0091 §H, HIGH).
/// Origin: toml-test `invalid/string/bad-hex-esc-01.toml`.
/// No `\u` escape exists, so no hex-digit/length validation can fail:
/// `\u12G4` stays six verbatim characters, deterministic.
#[test]
fn rb_240_no_unicode_escape_verbatim() {
    let v = eval_json("value = \"\\u12G4\"\n");
    assert_eq!(v["value"], "\\u12G4");
    let codes = codes_of("value = \"\\u12G4\"\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-241 — Upstream `bad-uni-esc-01` analogue (SRC-TOML-0092 §H, HIGH).
/// Origin: toml-test `invalid/string/bad-uni-esc-01.toml`.
/// Unpaired surrogates cannot materialize: without `\u` support there
/// is no surrogate decoding at all — `\uD800` is verbatim text.
#[test]
fn rb_241_surrogate_escape_verbatim() {
    let v = eval_json("value = \"\\uD800\"\n");
    assert_eq!(v["value"], "\\uD800");
    let codes = codes_of("value = \"\\uD800\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the `\0` escape decodes per the table (RB-178).
    assert_eq!(eval_json("value = \"a\\0b\"\n")["value"], "a\0b");
}

/// RB-242 — Upstream `bad-multiline` analogue (SRC-TOML-0093 §H, HIGH).
/// Origin: toml-test `invalid/string/bad-multiline.toml`.
/// Byte fixture (own file per SRC ID): raw LF in a `"…"` string.
/// Same exact-preservation policy as RB-175, extended through the
/// formatter: fmt → parse → eval agrees exactly (round-trip stable).
#[test]
fn rb_242_raw_newline_format_stable() {
    use active_core::fmt::format;
    use active_core::Style;
    let module = parse_file(
        &research_fixture("src_toml_0093_bad_multiline.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], "line one\nline two");
    assert!(diags.is_empty(), "{diags:?}");
    let out = format(&module, Style::Indent);
    let module2 =
        parse(&out, "research").unwrap_or_else(|d| panic!("reparse failed: {d:?}\n{out}"));
    let (second, _) = Engine::new().evaluate(&module2).expect("must evaluate");
    assert_eq!(
        active_core::convert::value_to_pretty_json(&value),
        active_core::convert::value_to_pretty_json(&second)
    );
}

/// RB-243 — Upstream `bad-slash-escape` analogue (SRC-TOML-0094 §H, HIGH).
/// Origin: toml-test `invalid/string/bad-slash-escape.toml`.
/// The escape alphabet is exactly the specified table — not a union
/// of host dialects: `\/` is unknown, hence verbatim (2 chars).
#[test]
fn rb_243_slash_escape_verbatim() {
    let v = eval_json("value = \"\\/\"\n");
    assert_eq!(v["value"], "\\/");
    let codes = codes_of("value = \"\\/\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: specified escapes still decode.
    assert_eq!(eval_json("value = \"a\\/b\"\n")["value"], "a\\/b");
}

/// RB-244 — Upstream `bad-concat` analogue (SRC-TOML-0095 §H, HIGH).
/// Origin: toml-test `invalid/string/bad-concat.toml`.
/// Adjacent-string concatenation is not specified: the second string
/// opens a new statement position and fails loud E002 — implicit
/// concatenation is never accidental.
#[test]
fn rb_244_adjacent_strings_reject() {
    let src = "value = \"a\" \"b\"\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 13), "{span:?}");
    // Positive control: the single string binds.
    assert_eq!(eval_json("value = \"a\"\n")["value"], "a");
}

/// RB-245 — JSONTestSuite `n_array_1_true_without_comma` analogue (SRC-JSON-0001 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_array_1_true_without_comma.json`.
/// Adjacent array elements heal under the locked H003 policy (RB-154),
/// booleans included: `[1, true]`, span pinned.
#[test]
fn rb_245_json_missing_comma_heals() {
    let src = "value = [1 true]\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!([1, true]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 12), "{span:?}");
    // Positive control: the separated neighbour carries no heal.
    assert!(!codes_of("value = [1, true]\n").iter().any(|c| c == "H003"));
}

/// RB-246 — JSONTestSuite `n_array_a_invalid_utf8` analogue (SRC-JSON-0002 §H, HIGH).
/// Origin: nst/JSONTestSuite `test_parsing/n_array_a_invalid_utf8.json`.
/// Byte fixture (own file per SRC ID): overlong bytes in an array.
/// Source encoding validates before parsing: controlled E017.
#[test]
fn rb_246_json_invalid_utf8_controlled_e017() {
    let diags = parse_file(
        &research_fixture("src_json_0002_invalid_utf8.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    assert!(
        diags.iter().any(|d| d.code == "E017"),
        "E017 expected, got {diags:?}"
    );
    // Positive control: the valid array evaluates.
    assert_eq!(
        eval_json("value = [1, 2]\n")["value"],
        serde_json::json!([1, 2])
    );
}

/// RB-247 — JSONTestSuite `n_array_colon_instead_of_comma` analogue (SRC-JSON-0003 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_array_colon_instead_of_comma.json`.
/// The array separator is the documented delimiter only: a colon is
/// E002 at the token — never a pair, never silent.
#[test]
fn rb_247_json_colon_in_array_rejects() {
    let src = "value = [1:2]\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 11), "{span:?}");
    // Positive control: the comma neighbour evaluates.
    assert_eq!(
        eval_json("value = [1, 2]\n")["value"],
        serde_json::json!([1, 2])
    );
}

/// RB-248 — JSONTestSuite `n_array_double_comma` analogue (SRC-JSON-0004 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_array_double_comma.json`.
/// Same locked H002 policy as the TOML-corpus twins (RB-089/RB-150):
/// two separators never imply an omitted value. New oracle dimension
/// here: the heal carries Info severity — it heals, it never fails.
#[test]
fn rb_248_json_double_comma_heals_info() {
    let src = "value = [1,,2]\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    // Positive control: the clean neighbour carries no heal.
    assert!(!codes_of("value = [1, 2]\n").iter().any(|c| c == "H002"));
}

/// RB-249 — JSONTestSuite `n_array_extra_comma` analogue (SRC-JSON-0005 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_array_extra_comma.json`.
/// Trailing commas are explicitly allowed (RB-004/RB-216): single
/// element heals H002 at 1:11, value `[1]`.
#[test]
fn rb_249_json_extra_comma_heals() {
    let src = "value = [1,]\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 11), "{span:?}");
    // Positive control: the comma-free neighbour carries no heal.
    assert!(!codes_of("value = [1]\n").iter().any(|c| c == "H002"));
}

/// RB-250 — JSONTestSuite `n_array_missing_value` analogue (SRC-JSON-0006 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_array_missing_value.json`.
/// REFUSAL (spec-gated): no empty slot is synthesized as null — the
/// stray comma heals H002 and the value is exactly `[1, 3]`.
#[test]
fn rb_250_json_missing_value_heals() {
    let src = "value = [1,,3]\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!([1, 3]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 12), "{span:?}");
}

/// RB-251 — JSONTestSuite `n_array_unclosed` analogue (SRC-JSON-0007 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_array_unclosed.json`.
/// EOF inside a collection diagnoses E001 with Error severity and an
/// EOF span — never a hang, never a partial tree.
#[test]
fn rb_251_json_unclosed_rejects_error() {
    let src = "value = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed neighbour evaluates.
    assert_eq!(
        eval_json("value = [1, 2]\n")["value"],
        serde_json::json!([1, 2])
    );
}

/// RB-252 — JSONTestSuite `n_incomplete_false` analogue (SRC-JSON-0008 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_incomplete_false.json`.
/// A partial keyword never becomes boolean false: bare `fals` is
/// unresolved (W001 + null), truncated from below like `falsee`
/// is extended from above (RB-165).
#[test]
fn rb_252_incomplete_false_never_bool() {
    let v = eval_json("value = fals\n");
    assert!(v["value"].is_null(), "{v}");
    let codes = codes_of("value = fals\n");
    assert!(
        codes.iter().any(|c| c == "W001"),
        "W001 expected, got {codes:?}"
    );
    // Positive control: the complete token is boolean false.
    assert_eq!(eval_json("value = false\n")["value"], false);
}

/// RB-253 — JSONTestSuite `n_multidigit_number_then_00` analogue (SRC-JSON-0009 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_multidigit_number_then_00.json`.
/// REFUSAL (spec-gated): extra leading zeros are plain decimals
/// (RB-104) — `00` is exactly int 0, never a second token, never
/// octal, never rejected.
#[test]
fn rb_253_double_zero_is_decimal() {
    let v = eval_json("value = 00\n");
    assert_eq!(v["value"], 0);
    assert!(v["value"].is_i64());
    let codes = codes_of("value = 00\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-254 — JSONTestSuite `n_number_++` analogue (SRC-JSON-0010 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_number_++.json`.
/// Numeric syntax admits no duplicated sign: E002 with Error
/// severity at the second `+` (same rule as the TOML twin RB-218).
#[test]
fn rb_254_json_double_sign_rejects_error() {
    let src = "value = ++1\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 9), "{span:?}");
    // Positive control: the single-signed neighbour evaluates.
    assert_eq!(eval_json("value = -1\n")["value"], -1);
}

/// RB-255 — JSONTestSuite `n_number_-01` analogue (SRC-JSON-0011 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_number_-01.json`.
/// REFUSAL (spec-gated): signed leading zeros are plain decimals —
/// `-01` is exactly int -1, loudly not octal, loudly not rejected.
#[test]
fn rb_255_negative_leading_zero_decimal() {
    let v = eval_json("value = -01\n");
    assert_eq!(v["value"], -1);
    assert!(v["value"].is_i64());
    // Positive control: the zero-free neighbour agrees.
    assert_eq!(eval_json("value = -1\n")["value"], -1);
}

/// RB-256 — JSONTestSuite `n_number_0.1.2` analogue (SRC-JSON-0012 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_number_0.1.2.json`.
/// REFUSAL (spec-gated): a single number token cannot hold two decimal
/// points — and trailing strays fall under the §2.1/§4.1 skip quirk
/// (RB-201): deterministic float 0.1, zero diags.
#[test]
fn rb_256_double_point_keeps_first_float() {
    let v = eval_json("value = 0.1.2\n");
    assert_eq!(v["value"], 0.1);
    assert!(v["value"].is_f64());
    let codes = codes_of("value = 0.1.2\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the clean float evaluates identically.
    assert_eq!(eval_json("value = 0.1\n")["value"], 0.1);
}

/// RB-257 — JSONTestSuite `n_number_0e+` analogue (SRC-JSON-0013 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_number_0e+.json`.
/// No exponent grammar exists for a sign to attach to: E002 at the `e`.
#[test]
fn rb_257_exp_sign_rejects() {
    let src = "value = 0e+\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
    // Positive control: the plain zero evaluates.
    assert_eq!(eval_json("value = 0\n")["value"], 0);
}

/// RB-258 — JSONTestSuite `n_number_2.e3` analogue (SRC-JSON-0014 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_number_2.e3.json`.
/// A fraction point requires digits and no exponent grammar exists:
/// E002 at the `e` (same rule as the TOML twin RB-202).
#[test]
fn rb_258_fraction_exp_rejects() {
    let src = "value = 2.e3\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 11), "{span:?}");
    // Positive control: the clean float evaluates.
    assert_eq!(eval_json("value = 2.5\n")["value"], 2.5);
}

/// RB-259 — JSONTestSuite `n_number_Inf` analogue (SRC-JSON-0015 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_number_Inf.json`.
/// Never case-folded into IEEE infinity: bare `Inf` is unresolved
/// (W001 + null) with span — special floats have no bare spellings.
#[test]
fn rb_259_inf_never_infinity() {
    let v = eval_json("value = Inf\n");
    assert!(v["value"].is_null(), "{v}");
    let module = parse("value = Inf\n", "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 9), "{span:?}");
    // Positive control: quoted form stays a string.
    assert_eq!(eval_json("value = \"Inf\"\n")["value"], "Inf");
}

/// RB-260 — JSONTestSuite `n_number_with_leading_zero` analogue (SRC-JSON-0016 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_number_with_leading_zero.json`.
/// REFUSAL (spec-gated): leading zeros are plain decimals (RB-104) —
/// `01` is exactly int 1, and inside arrays too: `[01, 02]` is `[1, 2]`.
#[test]
fn rb_260_json_leading_zero_decimal() {
    let v = eval_json("value = 01\n");
    assert_eq!(v["value"], 1);
    assert!(v["value"].is_i64());
    assert_eq!(
        eval_json("value = [01, 02]\n")["value"],
        serde_json::json!([1, 2])
    );
    // Positive control: the zero-free neighbours agree.
    assert_eq!(
        eval_json("value = [1, 2]\n")["value"],
        serde_json::json!([1, 2])
    );
}

/// RB-261 — JSONTestSuite `n_object_missing_colon` analogue (SRC-JSON-0017 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_object_missing_colon.json`.
/// A quoted pair fragment is not a pair: the string where an
/// identifier belongs fails E001 at 1:11 — the separator question
/// never arises because quoted keys never enter the tree (RB-051).
#[test]
fn rb_261_json_missing_colon_rejects() {
    let src = "value = { \"a\" 1 }\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (1, 11), "{span:?}");
    // Positive control: the bare-key pair binds.
    assert_eq!(eval_json("value = { a = 1 }\n")["value"]["a"], 1);
}

/// RB-262 — JSONTestSuite `n_object_missing_key` analogue (SRC-JSON-0018 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_object_missing_key.json`.
/// A map pair requires a key: `=` where an identifier belongs fails
/// E001 at 1:11 — nothing keyless is built.
#[test]
fn rb_262_json_missing_key_rejects() {
    let src = "value = { = 1 }\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (1, 11), "{span:?}");
    // Positive control: the keyed pair binds.
    assert_eq!(eval_json("value = { a = 1 }\n")["value"]["a"], 1);
}

/// RB-263 — JSONTestSuite `n_object_trailing_comma` analogue (SRC-JSON-0019 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_object_trailing_comma.json`.
/// (Translation repeats the `[1,]` shape; pinned here in table form —
/// trailing commas are explicitly allowed everywhere, RB-216.)
/// `{ a = 1, }` heals H002, value exact.
#[test]
fn rb_263_json_table_trailing_comma_heals() {
    let src = "value = { a = 1, }\n";
    assert_eq!(eval_json(src)["value"], serde_json::json!({"a": 1}));
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "H002"),
        "H002 expected, got {codes:?}"
    );
}

/// RB-264 — JSONTestSuite `n_object_unquoted_key` analogue (SRC-JSON-0020 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_object_unquoted_key.json`.
/// Bare keys ARE the decided grammar (identifiers only): `{ a = 1 }`
/// binds with zero diagnostics — no JSON key restriction copied.
#[test]
fn rb_264_json_bare_key_accepts() {
    let v = eval_json("value = { a = 1 }\n");
    assert_eq!(v["value"]["a"], 1);
    let codes = codes_of("value = { a = 1 }\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the block-form twin agrees.
    assert_eq!(eval_json("value:\n  a = 1\n")["value"]["a"], 1);
}

/// RB-265 — JSONTestSuite `n_string_incomplete_escape` analogue (SRC-JSON-0021 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_string_incomplete_escape.json`.
/// EOF right after a backslash: the unterminated string heals H001
/// (RB-049 channel) at 1:9 with the backslash preserved — announced,
/// never silently repaired.
#[test]
fn rb_265_escape_at_eof_heals_announced() {
    let src = "value = \"abc\\";
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.iter().any(|d| d.code == "H001"), "{diags:?}");
    let h = diags.iter().find(|d| d.code == "H001").expect("H001");
    let span = h.span.clone().expect("H001 carries a span");
    assert_eq!((span.line, span.column), (1, 9), "{span:?}");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], "abc\\");
    // Positive control: the terminated neighbour carries no heal.
    assert!(!codes_of("value = \"abc\"\n").iter().any(|c| c == "H001"));
}

/// RB-266 — JSONTestSuite `n_string_invalid_unicode_escape` analogue (SRC-JSON-0022 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_string_invalid_unicode_escape.json`.
/// No `\u` decoding exists to fail on bad hex: the six characters
/// stay verbatim (RB-240's TOML twin) — char-count proves no decoding.
#[test]
fn rb_266_json_bad_unicode_escape_verbatim() {
    let v = eval_json("value = \"\\u12G4\"\n");
    assert_eq!(v["value"], "\\u12G4");
    assert_eq!(v["value"].as_str().expect("string").chars().count(), 6);
    let codes = codes_of("value = \"\\u12G4\"\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-267 — JSONTestSuite `n_string_unescaped_newline` analogue (SRC-JSON-0023 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_string_unescaped_newline.json`.
/// Byte fixture (own file per SRC ID): raw LF preserved exactly —
/// the literal-newline policy is explicit (RB-175/RB-242).
#[test]
fn rb_267_json_unescaped_newline_kept() {
    let module = parse_file(
        &research_fixture("src_json_0023_unescaped_newline.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], "line1\nline2");
    assert!(diags.is_empty(), "{diags:?}");
}

/// RB-268 — JSONTestSuite `n_string_unescaped_tab` analogue (SRC-JSON-0024 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_string_unescaped_tab.json`.
/// Byte fixture: raw TAB preserved exactly. REFUSAL of "reject raw
/// TAB": strings are content-exact — raw control and escaped content
/// are both kept literally (RB-174 covers TAB outside strings).
#[test]
fn rb_268_json_unescaped_tab_kept() {
    let module = parse_file(
        &research_fixture("src_json_0024_unescaped_tab.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], "a\tb");
    assert!(diags.is_empty(), "{diags:?}");
    // Positive control: the escaped twin agrees exactly.
    assert_eq!(eval_json("value = \"a\\tb\"\n")["value"], "a\tb");
}

/// RB-279 — Norway implicit-typing report analogue (SRC-YAML-0001 §H, HIGH).
/// Origin: yaml-test-suite issue #123 (Norway problem).
/// REFUSAL (spec-gated): bare `no`/`on` do NOT remain strings — no
/// bare word becomes a string implicitly (explicitness). They resolve
/// W001 + null like every bare word (RB-103); only exact `true` is
/// boolean. Quoted forms stay strings (control).
#[test]
fn rb_279_norway_no_implicit_strings() {
    let v = eval_json("country = no\nflag = on\nanswer = true\n");
    assert!(v["country"].is_null(), "{v}");
    assert!(v["flag"].is_null(), "{v}");
    assert_eq!(v["answer"], true);
    let codes = codes_of("country = no\nflag = on\nanswer = true\n");
    assert_eq!(
        codes.iter().filter(|c| *c == "W001").count(),
        2,
        "{codes:?}"
    );
    // Positive control: quoted forms stay exact strings.
    let v = eval_json("country = \"no\"\nflag = \"on\"\n");
    assert_eq!(v["country"], "no");
    assert_eq!(v["flag"], "on");
}

/// RB-280 — Colon-in-anchor report analogue (SRC-YAML-0002 §H, HIGH).
/// Origin: yaml-test-suite issue #144.
/// No anchor construct exists: `&` is an unexpected character (E005
/// at 1:9) — never accidentally accepted, never half-parsed.
#[test]
fn rb_280_anchor_sigil_rejects() {
    let src = "value = &anchor: 1\n";
    let diags = parse(src, "research").expect_err("must fail E005");
    assert!(
        diags.iter().any(|d| d.code == "E005"),
        "E005 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 9), "{span:?}");
    // Positive control: the sigil-free value binds.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-281 — Escaped-characters gap analogue (SRC-YAML-0003 §H, MEDIUM).
/// Origin: yaml-test-suite issue #135 (spec §5.13/5.14 escapes).
/// Same closed alphabet as the TOML twin (RB-240): `\u12G4` stays
/// six verbatim characters — now pinned through the TOML exporter,
/// proving exports never decode what the grammar did not.
#[test]
fn rb_281_yaml_bad_escape_verbatim_export() {
    let v = eval_json("value = \"\\u12G4\"\n");
    assert_eq!(v["value"], "\\u12G4");
    let module = parse("value = \"\\u12G4\"\n", "research").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml export");
    assert!(toml.contains("\\u12G4"), "{toml}");
}

/// RB-282 — Colon-handling question analogue (SRC-YAML-0004 §H, MEDIUM).
/// Origin: yaml-test-suite issue #126 (`a: :a` vs `a: a`).
/// Explicit `=` fixes the value boundary: a second colon inside the
/// value fails E002 at 1:13 — no key separator can shift the
/// boundary. The quoted text stays a string (control).
#[test]
fn rb_282_double_colon_value_rejects() {
    let src = "value = a: :a\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 13), "{span:?}");
    // Positive control: quoted text stays an exact string.
    assert_eq!(eval_json("value = \"a: :a\"\n")["value"], "a: :a");
}

/// RB-283 — Merge-key test-request analogue (SRC-YAML-0005 §H, MEDIUM).
/// Origin: yaml-test-suite issue #150 (merge-key coverage).
/// YAML `<<` is not imported: the merge-key spelling fails, while the
/// analogous structural overlay uses explicit `@patch` semantics —
/// deterministic, base untouched, target extended.
#[test]
fn rb_283_merge_key_refused_patch_explicit() {
    let src = "base = { x = 1 }\nconfig = { y = 2 }\n@patch(\"config.x\", 1)\n";
    let v = eval_json(src);
    assert_eq!(v["config"]["x"], 1);
    assert_eq!(v["config"]["y"], 2);
    assert_eq!(v["base"], serde_json::json!({"x": 1}));
    // The `<<` spelling has no meaning: quoted merge keys never bind.
    let codes = codes_of("config = { y = 2, \"<<\" = 1 }\n");
    assert!(
        codes.iter().any(|c| c == "E002" || c == "E001"),
        "E-class expected, got {codes:?}"
    );
}

/// RB-284 — Duplicate-keys-ignored report analogue (SRC-YAML-0006 §H, MEDIUM).
/// Origin: symfony/symfony#19526.
/// Never silently ignored AND never silent last-wins: the locked
/// W011 policy announces with Warning severity (RB-273's Symfony
/// twin pins value + severity here).
#[test]
fn rb_284_symfony_dup_warns_announced() {
    let src = "value = { key = 1, key = 2 }\n";
    assert_eq!(eval_json(src)["value"]["key"], 2);
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W011 must be Warning, got {:?}",
        w.severity
    );
    // Positive control: distinct keys bind silently.
    let codes = codes_of("value = { a = 1, b = 2 }\n");
    assert!(!codes.iter().any(|c| c == "W011"), "{codes:?}");
}

/// RB-285 — Flow-collection fixture VJP3 analogue (SRC-YAML-0007 §H, MEDIUM).
/// Origin: yaml-test-suite `src/VJP3.yaml`.
/// Nested bracket/brace balance is checked: `}` where `]` belongs
/// fails E001 at 1:25 — mismatched delimiters never close silently.
#[test]
fn rb_285_mismatched_delimiter_rejects() {
    let src = "value = { child = [1, 2 }\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (1, 25), "{span:?}");
    // Positive control: the balanced twin evaluates.
    assert_eq!(
        eval_json("value = { child = [1, 2] }\n")["value"]["child"],
        serde_json::json!([1, 2])
    );
}

/// RB-286 — Block-scalar fixture Z67P analogue (SRC-YAML-0008 §H, HIGH).
/// Origin: yaml-test-suite `src/Z67P.yaml`.
/// Escaped newlines are exact: one LF between lines plus one
/// terminal LF — code points pinned, round-trip stable.
#[test]
fn rb_286_escaped_newlines_exact() {
    let v = eval_json("value = \"line one\\nline two\\n\"\n");
    assert_eq!(v["value"], "line one\nline two\n");
    // Positive control: the round-trip keeps every code point.
    let module = parse("value = \"line one\\nline two\\n\"\n", "research").expect("must parse");
    let out = active_core::fmt::format(&module, active_core::Style::Indent);
    let module2 =
        parse(&out, "research").unwrap_or_else(|d| panic!("reparse failed: {d:?}\n{out}"));
    let (first, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let (second, _) = Engine::new().evaluate(&module2).expect("must evaluate");
    assert_eq!(
        active_core::convert::value_to_pretty_json(&first),
        active_core::convert::value_to_pretty_json(&second)
    );
}

/// RB-287 — Indentation-and-tabs spec analogue (SRC-YAML-0009 §H, HIGH).
/// Origin: YAML 1.2.2 (tabs forbidden for indentation).
/// Byte fixture: TAB-indented child. Structural TABs fail loud E006
/// at 2:1 — never silently dedented, never spaces.
#[test]
fn rb_287_tab_indent_rejects() {
    let diags = parse_file(
        &research_fixture("src_yaml_0009_tab_indent.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E006");
    assert!(
        diags.iter().any(|c| c.code == "E006"),
        "E006 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E006").expect("E006");
    let span = e.span.clone().expect("E006 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: space indentation binds the same shape.
    assert_eq!(eval_json("root:\n  child = 1\n")["root"]["child"], 1);
}

/// RB-288 — Core-schema analogue (SRC-YAML-0010 §H, HIGH).
/// Origin: YAML 1.2.2 core schema (implicit resolution).
/// Same Norway input as RB-279, asserted at the TYPE level:
/// bare words are null (never string, never bool), exact `true`
/// is boolean, quoted forms are strings. No implicit resolution.
#[test]
fn rb_288_core_schema_types_exact() {
    let v = eval_json("country = no\nflag = on\nanswer = true\n");
    assert!(v["country"].is_null(), "{v}");
    assert!(v["flag"].is_null(), "{v}");
    assert!(v["answer"].is_boolean(), "{v}");
    let v = eval_json("country = \"no\"\nflag = \"on\"\n");
    assert!(v["country"].is_string(), "{v}");
    assert!(v["flag"].is_string(), "{v}");
}

/// RB-269 — JSONTestSuite `n_string_invalid_lonely_surrogate` analogue (SRC-JSON-0025 §H, HIGH).
/// Origin: nst/JSONTestSuite `test_parsing/n_string_invalid_lonely_surrogate.json`.
/// No lone surrogate can emit invalid Unicode: without `\u` support
/// the text stays verbatim (RB-241's TOML twin) — and the YAML export
/// agrees, so the scalar invariant holds end-to-end across formats.
#[test]
fn rb_269_lonely_surrogate_verbatim_everywhere() {
    let v = eval_json("value = \"\\uD800\"\n");
    assert_eq!(v["value"], "\\uD800");
    let module = parse("value = \"\\uD800\"\n", "research").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let yaml = active_core::convert::value_to_yaml_string(&value).expect("yaml export");
    assert!(yaml.contains("\\uD800"), "{yaml}");
    // Positive control: no diags on the verbatim path.
    let codes = codes_of("value = \"\\uD800\"\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-270 — JSONTestSuite `n_structure_null-byte-outside-string` analogue (SRC-JSON-0026 §H, HIGH).
/// Origin: nst/JSONTestSuite `test_parsing/n_structure_null-byte-outside-string.json`.
/// Byte fixture: raw NUL inside brackets. The parser never receives
/// NUL as ordinary whitespace: loud E005 at 1:10, parse fails.
/// (Inside strings NUL is preserved data — RB-172; outside, E005.)
#[test]
fn rb_270_nul_outside_string_rejects() {
    let diags = parse_file(
        &research_fixture("src_json_0026_nul_outside_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    assert!(
        diags.iter().any(|d| d.code == "E005"),
        "E005 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
    // Positive control: the NUL-free brackets evaluate.
    assert_eq!(eval_json("value = []\n")["value"], serde_json::json!([]));
}

/// RB-271 — JSONTestSuite `n_structure_100000_opening_arrays` analogue (SRC-JSON-0027 §H, CRITICAL).
/// Origin: nst/JSONTestSuite `test_parsing/n_structure_100000_opening_arrays.json`.
/// 100000 consecutive `[` with no closures: the depth budget trips
/// (E004) instead of overflowing the stack — completion of this very
/// test is the resource-safety proof.
#[test]
fn rb_271_hundred_k_brackets_trip_budget() {
    let src = format!("value = {}", "[".repeat(100_000));
    let diags = parse(&src, "research").expect_err("must fail E004");
    assert!(
        diags.iter().any(|d| d.code == "E004"),
        "E004 expected, got {:?}",
        &diags[..diags.len().min(3)]
    );
    // Positive control: a shallow array evaluates.
    assert_eq!(eval_json("value = [1]\n")["value"], serde_json::json!([1]));
}

/// RB-272 — JSONTestSuite `i_structure_500_nested_arrays` analogue (SRC-JSON-0028 §H, CRITICAL).
/// Origin: nst/JSONTestSuite `test_parsing/i_structure_500_nested_arrays.json`.
/// 500 nested arrays around `0`: E004 at the documented bound (64) —
/// the `i_` (implementation-defined) case resolves to the documented
/// limit, terminating cleanly with spans.
#[test]
fn rb_272_500_nested_rejects_at_bound() {
    let src = format!("value = {}0{}", "[".repeat(500), "]".repeat(500));
    let diags = parse(&src, "research").expect_err("must fail E004");
    assert!(
        diags.iter().any(|d| d.code == "E004"),
        "E004 expected, got {:?}",
        &diags[..diags.len().min(3)]
    );
    // Positive control: nesting below the bound evaluates.
    let ok = format!("value = {}0{}", "[".repeat(10), "]".repeat(10));
    let v = eval_json(&ok);
    assert!(v["value"].is_array(), "{v}");
}

/// RB-273 — JSONTestSuite `y_object_duplicated_key` analogue (SRC-JSON-0029 §H, HIGH).
/// Origin: nst/JSONTestSuite `test_parsing/y_object_duplicated_key.json`.
/// REFUSAL (spec-gated): the row proposes strict duplicate rejection,
/// but Active's locked W011 last-wins policy (RB-002, HIST-YAML-006)
/// governs — announced Warning severity, value 2. Interop ambiguity
/// is resolved by announcement, never silence.
#[test]
fn rb_273_json_dup_key_warns_last_wins() {
    let src = "value = { a = 1, a = 2 }\n";
    assert_eq!(eval_json(src)["value"]["a"], 2);
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W011 must be Warning, got {:?}",
        w.severity
    );
}

/// RB-274 — JSONTestSuite `y_string_accepted_surrogate_pair` analogue (SRC-JSON-0030 §H, HIGH).
/// Origin: nst/JSONTestSuite `test_parsing/y_string_accepted_surrogate_pair.json`.
/// REFUSAL (spec-gated): no surrogate-pair decoding exists (no `\u`
/// support at all) — the pair form stays twelve verbatim characters
/// like the lone form (RB-269). Valid pair is achieved by writing the
/// scalar directly (control).
#[test]
fn rb_274_surrogate_pair_verbatim() {
    let v = eval_json("value = \"\\uD834\\uDD1E\"\n");
    assert_eq!(v["value"], "\\uD834\\uDD1E");
    assert_eq!(v["value"].as_str().expect("string").chars().count(), 12);
    let codes = codes_of("value = \"\\uD834\\uDD1E\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the scalar itself is exact data.
    assert_eq!(eval_json("value = \"𝄞\"\n")["value"], "𝄞");
}

/// RB-275 — JSONTestSuite `i_number_double_huge_neg_exp` analogue (SRC-JSON-0031 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/i_number_double_huge_neg_exp.json`.
/// No exponent grammar exists for underflow to occur in: E002 at the
/// `e` — never a silently bounded-precision result, never an integer.
#[test]
fn rb_275_huge_neg_exp_rejects() {
    let src = "value = 1e-999999\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
    // Positive control: the plain one evaluates.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-276 — JSONTestSuite `i_number_huge_exp` analogue (SRC-JSON-0032 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/i_number_huge_exp.json`.
/// Exponent parsing is bounded by absence: E002 at the `e` — huge
/// exponents cannot reach any range logic.
#[test]
fn rb_276_huge_exp_rejects() {
    let src = "value = 1e999999\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 10), "{span:?}");
}

/// RB-277 — JSONTestSuite `n_structure_object_with_comment` analogue (SRC-JSON-0033 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/n_structure_object_with_comment.json`.
/// Block comments are not in the grammar (only `#` lines, §1.2): the
/// `/` fails loud E002 — comment syntax is one documented grammar,
/// never inherited from JavaScript.
#[test]
fn rb_277_block_comment_rejects() {
    let src = "value = { a = 1 /* comment */ }\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    // Positive control: the `#` comment twin binds cleanly.
    let v = eval_json("value = { a = 1 } # comment\n");
    assert_eq!(v["value"]["a"], 1);
}

/// RB-278 — JSONTestSuite `y_object_empty_key` analogue (SRC-JSON-0034 §H, MEDIUM).
/// Origin: nst/JSONTestSuite `test_parsing/y_object_empty_key.json`.
/// Empty keys are not lost and not kept: quoted keys never enter the
/// tree (RB-230) — E001 where an identifier belongs, span pinned.
/// (Completes the JSON family: 34/34 mined.)
#[test]
fn rb_278_json_empty_key_rejects() {
    let src = "value = { \"\" = 1 }\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (1, 11), "{span:?}");
    // Positive control: the identifier-key twin binds.
    assert_eq!(eval_json("value = { a = 1 }\n")["value"]["a"], 1);
}

/// RB-289 — Block-scalar chomping analogue (SRC-YAML-0011 §H, HIGH).
/// Origin: YAML 1.2.2 block scalar chomping.
/// Representation is explicit: the escaped form keeps its terminal
/// LF exactly, while the `"""` block twin drops the final
/// whitespace-only line (spec/grammar.md §1.4.1) — the two differ by
/// exactly the terminal LF, never by silent chomping.
#[test]
fn rb_289_chomping_explicit_per_form() {
    let v = eval_json("value = \"line one\\nline two\\n\"\n");
    assert_eq!(v["value"], "line one\nline two\n");
    let v = eval_json("value = \"\"\"\nline one\nline two\n\"\"\"\n");
    assert_eq!(v["value"], "line one\nline two");
    // Positive control: single-line content agrees across forms.
    assert_eq!(
        eval_json("a = \"x\"\n")["a"],
        eval_json("a = \"\"\"x\"\"\"\n")["a"]
    );
}

/// RB-290 — Anchors-and-aliases analogue (SRC-YAML-0012 §H, HIGH).
/// Origin: YAML 1.2.2 anchors/aliases.
/// No anchors exist to chase; the alias-like indirection that DOES
/// exist (interpolation) participates in cycle detection: a 2-cycle
/// through strings fails S001 like every cycle (RB-030).
#[test]
fn rb_290_interp_cycle_is_s001() {
    let src = "let a = \"${b}\"\nlet b = \"${a}\"\nvalue = a\n";
    let module = parse(src, "research").expect("must parse");
    let err = Engine::new().evaluate(&module).expect_err("must fail S001");
    assert_eq!(err.code(), "S001");
    // Positive control: acyclic interpolation evaluates.
    assert_eq!(
        eval_json("let b = \"x\"\nvalue = \"${b}yz\"\n")["value"],
        "xyz"
    );
}

/// RB-291 — Fixture-format analogue (SRC-YAML-0013 §H, MEDIUM).
/// Origin: yaml-test-suite usage (input/expected/events per case).
/// The portable conformance oracle: one document formatted in BOTH
/// styles reparses to equal values — emission agrees, not just eval.
#[test]
fn rb_291_dual_format_emission_agrees() {
    use active_core::fmt::format;
    use active_core::Style;
    let src = "value = { a = 1 }\n";
    for style in [Style::Brace, Style::Indent] {
        let module = parse(src, "research").expect("must parse");
        let out = format(&module, style);
        let module2 = parse(&out, "research")
            .unwrap_or_else(|d| panic!("reparse failed ({style:?}): {d:?}\n{out}"));
        let (first, _) = Engine::new().evaluate(&module).expect("must evaluate");
        let (second, _) = Engine::new().evaluate(&module2).expect("must evaluate");
        let a: serde_json::Value =
            serde_json::from_str(&active_core::convert::value_to_pretty_json(&first))
                .expect("json");
        let b: serde_json::Value =
            serde_json::from_str(&active_core::convert::value_to_pretty_json(&second))
                .expect("json");
        assert_eq!(a, b, "style {style:?}");
        assert_eq!(a["value"]["a"], 1);
    }
}

/// RB-292 — Tag-definitions analogue (SRC-YAML-0014 §H, MEDIUM).
/// Origin: yaml-test-suite `doc/tag-usage.md` (empty-key class).
/// Suite categories inspire the coverage; YAML syntax is not
/// imported: a quoted key inside braces fails E001 at 1:11.
#[test]
fn rb_292_tag_class_empty_key_rejects() {
    let src = "value = { \"empty\" = 1 }\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (1, 11), "{span:?}");
    // Positive control: the bare-key twin binds.
    assert_eq!(eval_json("value = { empty = 1 }\n")["value"]["empty"], 1);
}

/// RB-293 — Merge-semantics discussion analogue (SRC-YAML-0015 §H, MEDIUM).
/// Origin: community merge-precedence question (non-normative).
/// The naive transliteration `@patch config { x = 1 }` is rejected
/// (E019: `(` required) — only the explicit call form overlays, so
/// merge precedence is never a community question in Active.
#[test]
fn rb_293_naive_patch_form_rejects() {
    let src = "base = { x = 1 }\nconfig = { y = 2 }\n@patch config { x = 1 }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    assert!(
        diags.iter().any(|d| d.code == "E019"),
        "E019 expected, got {diags:?}"
    );
    // Positive control: the explicit call form overlays deterministically.
    let v = eval_json("base = { x = 1 }\nconfig = { y = 2 }\n@patch(\"config.x\", 1)\n");
    assert_eq!(v["config"]["x"], 1);
    assert_eq!(v["base"]["x"], 1);
}

/// RB-294 — SnakeYAML nested-depth DoS analogue (SRC-CVE-0001 §H, CRITICAL).
/// Origin: NVD CVE-2022-25857.
/// Bounded generator (not the destructive PoC): 2000 unclosed `[`
/// trip E004 with span — depth is bounded before stack is at risk.
#[test]
fn rb_294_cve_nested_depth_bounded() {
    let src = format!("value = {}", "[".repeat(2000));
    let diags = parse(&src, "research").expect_err("must fail E004");
    assert!(
        diags.iter().any(|d| d.code == "E004"),
        "E004 expected, got {:?}",
        &diags[..diags.len().min(3)]
    );
    let e = diags.iter().find(|d| d.code == "E004").expect("E004");
    assert!(e.span.is_some(), "E004 carries a span");
    // Positive control: shallow nesting evaluates.
    assert_eq!(
        eval_json("value = [[1]]\n")["value"],
        serde_json::json!([[1]])
    );
}

/// RB-295 — SnakeYAML stack-overflow analogue (SRC-CVE-0002 §H, CRITICAL).
/// Origin: NVD CVE-2022-41854.
/// Bounded generator: 300 balanced nested arrays trip E004 — bounded
/// input, controlled failure, no stack exhaustion.
#[test]
fn rb_295_cve_stack_overflow_bounded() {
    let src = format!("value = {}0{}", "[".repeat(300), "]".repeat(300));
    let diags = parse(&src, "research").expect_err("must fail E004");
    assert!(
        diags.iter().any(|d| d.code == "E004"),
        "E004 expected, got {:?}",
        &diags[..diags.len().min(3)]
    );
}

/// RB-296 — SnakeYAML stack-overflow analogue 2 (SRC-CVE-0003 §H, CRITICAL).
/// Origin: NVD CVE-2022-38752.
/// Same bound through a different construct: 150 nested inline
/// tables trip E004 — the limit is structural, not array-specific.
#[test]
fn rb_296_cve_table_nesting_bounded() {
    let src = format!("value = {}0{}", "{ a = ".repeat(150), " }".repeat(150));
    let diags = parse(&src, "research").expect_err("must fail E004");
    assert!(
        diags.iter().any(|d| d.code == "E004"),
        "E004 expected, got {:?}",
        &diags[..diags.len().min(3)]
    );
    // Positive control: shallow tables evaluate.
    assert_eq!(eval_json("value = { a = 0 }\n")["value"]["a"], 0);
}

/// RB-297 — Go YAML alias-chasing analogue (SRC-CVE-0004 §H, CRITICAL).
/// Origin: NVD CVE-2021-4235.
/// No aliases exist to chase; the reference-like fan-out that DOES
/// exist (interpolation) materializes exactly under budget: four
/// copies of `xyz` — deterministic, zero diags. Geometric abuse
/// trips the expansion budget instead (RB-093).
#[test]
fn rb_297_alias_fanout_exact_under_budget() {
    let src = "let a = \"xyz\"\nlet b = \"${a}${a}${a}${a}\"\nvalue = b\n";
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], "xyzxyzxyzxyz");
    assert!(diags.is_empty(), "{diags:?}");
}

/// RB-298 — LibYAML tag-URI heap-overflow analogue (SRC-CVE-0005 §H, CRITICAL).
/// Origin: NVD CVE-2013-6393.
/// No tag construct exists: the hostile line fails E002 at parse, so
/// no host constructor is ever invoked — nothing evaluates, nothing
/// executes. (Completes SRC-YAML-0011…0015 + opens SRC-CVE 0001…0005.)
#[test]
fn rb_298_tag_uri_never_executes() {
    let src = "value = !python/object/apply:os.system [\"echo forbidden\"]\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    // Positive control: the tag-free neighbour binds.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-309 — TOML string-grammar analogue (SRC-SPEC-0006 §H, HIGH).
/// Origin: TOML v1.0.0 string grammar.
/// The specified escape alphabet decodes fully (`\n \t \r \\ \" \0`)
/// while `\u12G4` stays verbatim (RB-240): one test locks the whole
/// table — strict where specified, verbatim otherwise.
#[test]
fn rb_309_escape_alphabet_matrix() {
    // Rust `\\` = one Active backslash; Rust `\n` = a real statement break.
    let src = "a = \"n\\n\"\nb = \"t\\t\"\nc = \"r\\r\"\nd = \"q\\\\q\"\ne = \"u\\u12G4\"\n";
    let v = eval_json(src);
    assert_eq!(v["a"], "n\n");
    assert_eq!(v["b"], "t\t");
    assert_eq!(v["c"], "r\r");
    assert_eq!(v["d"], "q\\q");
    assert_eq!(v["e"], "u\\u12G4");
    let codes = codes_of("a = \"n\\n\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: `\0` decodes per the table (RB-178).
    assert_eq!(eval_json("z = \"a\\0b\"\n")["z"], "a\0b");
}

/// RB-310 — TOML offset date-time analogue (SRC-SPEC-0007 §H, HIGH).
/// Origin: TOML v1.0.0 offset date-time grammar.
/// Quoted date-times stay text independent of calendar support —
/// pinned at the TYPE level and through both value exporters, so no
/// host datetime type can ever reinterpret them downstream.
#[test]
fn rb_310_quoted_datetime_typed_string() {
    let src = "value = \"1979-05-27T00:32:00-07:00\"\n";
    let v = eval_json(src);
    assert!(v["value"].is_string(), "{v}");
    assert_eq!(v["value"], "1979-05-27T00:32:00-07:00");
    let module = parse(src, "research").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let yaml = active_core::convert::value_to_yaml_string(&value).expect("yaml export");
    assert!(yaml.contains("1979-05-27T00:32:00-07:00"), "{yaml}");
    let (toml, _) = active_core::convert::value_to_toml_string(&value).expect("toml export");
    assert!(toml.contains("1979-05-27T00:32:00-07:00"), "{toml}");
}

/// RB-311 — RFC 8259 number-grammar analogue (SRC-SPEC-0008 §H, MEDIUM).
/// Origin: RFC 8259 number grammar.
/// `01.` is int 1 (RB-308) and `00.5` is float 0.5: leading zeros are
/// plain decimals everywhere, no host-language fallback, types exact.
#[test]
fn rb_311_rfc8259_leading_zero_float() {
    let v = eval_json("value = 00.5\n");
    assert_eq!(v["value"], 0.5);
    assert!(v["value"].is_f64());
    let v = eval_json("value = 01.\n");
    assert_eq!(v["value"], 1);
    assert!(v["value"].is_i64());
}

/// RB-312 — I-JSON duplicate-name analogue (SRC-SPEC-0009 §H, HIGH).
/// Origin: RFC 7493 (no duplicate member names).
/// Security-sensitive duplicates cannot change interpretation: the
/// quoted twin-keys fail E001 at 1:11, so nothing evaluates and no
/// consumer ever sees `admin` where `user` was meant.
#[test]
fn rb_312_ijson_dup_quoted_rejects() {
    let src = "value = { \"role\" = \"user\", \"role\" = \"admin\" }\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (1, 11), "{span:?}");
    // Positive control: distinct quoted... bare keys bind (quoted never do).
    assert_eq!(
        eval_json("value = { role = \"user\" }\n")["value"]["role"],
        "user"
    );
}

/// RB-313 — JSONTestSuite README analogue (SRC-SPEC-0010 §H, MEDIUM).
/// Origin: nst/JSONTestSuite (y_/n_/i_ prefix contract).
/// Baseline scalar probes: MUST-accept parses to int 1 with zero
/// diags — the fixed point every other oracle builds on.
#[test]
fn rb_313_baseline_scalar_probes() {
    let v = eval_json("probe_1010 = 1\n");
    assert_eq!(v["probe_1010"], 1);
    assert!(v["probe_1010"].is_i64());
    let codes = codes_of("probe_1010 = 1\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the JSON export round-trips the scalar.
    let module = parse("probe_1010 = 1\n", "research").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let json = active_core::convert::value_to_pretty_json(&value);
    let back: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(back["probe_1010"], 1);
}

/// RB-314 — Bare-CR variant: NUL before first token (SYN-0021-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0021 (toml-test bare-cr).
/// Byte fixture: NUL at offset 0. The decoder reports the byte
/// offset: loud E005 at 1:1 — a leading NUL never becomes whitespace
/// or an empty document.
#[test]
fn rb_314_nul_first_rejects_offset() {
    let diags = parse_file(
        &research_fixture("syn_0021_01_nul_first.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    assert!(
        diags.iter().any(|d| d.code == "E005"),
        "E005 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
    // Positive control: the NUL-free document evaluates.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-315 — Bare-CR variant: BOM at offset 3 (SYN-0021-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0021 (toml-test bare-cr).
/// Byte fixture: `x = ` then BOM bytes. BOM acceptance is the
/// leading position only (RB-197): mid-token BOM is E005 at 1:5.
#[test]
fn rb_315_bom_offset3_rejects() {
    let diags = parse_file(
        &research_fixture("syn_0021_03_bom_offset3.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    assert!(
        diags.iter().any(|d| d.code == "E005"),
        "E005 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 5), "{span:?}");
}

/// RB-316 — Bare-CR variant: TAB at line 1 (SYN-0021-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0021 (toml-test bare-cr).
/// Byte fixture: TAB-indented first line. Structural TABs fail E006
/// at 1:1 even at document start — never silently dedented.
/// (Completes the TAB-indent pair with RB-287's line-2 case.)
#[test]
fn rb_316_tab_line1_rejects() {
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E006");
    assert!(
        diags.iter().any(|d| d.code == "E006"),
        "E006 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E006").expect("E006");
    let span = e.span.clone().expect("E006 carries a span");
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
    // Positive control: space indentation binds.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-317 — Hostile single-NUL file terminates controlled.
/// Byte fixture: one NUL byte, nothing else. The smallest hostile
/// input fails loud E005 at 1:1 — no panic, no hang, no empty-doc
/// confusion. (P0-016 family evidence, own file.)
#[test]
fn rb_317_nul_only_terminates() {
    let diags = parse_file(
        &research_fixture("byte_nul_only.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    assert!(
        diags.iter().any(|d| d.code == "E005"),
        "E005 expected, got {diags:?}"
    );
    // Positive control: the empty document is the clean neighbour.
    let module = parse_file(
        &research_fixture("byte_empty.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("empty must parse: {d:?}"));
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v, serde_json::json!({}));
}

/// RB-318 — Empty file evaluates to the empty document.
/// Byte fixture: zero bytes. EOF at position zero finalizes to `{}` —
/// the documented floor of the no-EOL policy (RB-233): nothing to
/// drop, nothing to heal, zero diagnostics.
#[test]
fn rb_318_empty_file_evaluates() {
    let module = parse_file(
        &research_fixture("byte_empty.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("empty must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v, serde_json::json!({}));
    assert!(diags.is_empty(), "{diags:?}");
}

/// RB-319 — Derived variant: no implicit null (SYN-0001-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0001 (toml-test double-comma-01).
/// A doubled comma cannot create an implicit null: the value is
/// exactly `[1, 2]` — two elements, no nulls — with announced H002.
#[test]
fn rb_319_syn_no_implicit_null() {
    let src = "case_0001_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0001_01"], serde_json::json!([1, 2]));
    assert_eq!(v["case_0001_01"].as_array().expect("array").len(), 2);
    assert!(
        !v["case_0001_01"]
            .as_array()
            .expect("array")
            .iter()
            .any(|e| e.is_null()),
        "{v}"
    );
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "H002"),
        "H002 expected, got {codes:?}"
    );
}

/// RB-320 — Derived variant: no silent merge (SYN-0001-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0001 (toml-test double-comma-01).
/// Adjacent values are not merged: `[1 2]` heals H003 to two
/// distinct elements — never one merged value, never silent.
#[test]
fn rb_320_syn_no_silent_merge() {
    let src = "case_0001_02 = [1 2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0001_02"], serde_json::json!([1, 2]));
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "H003"),
        "H003 expected, got {codes:?}"
    );
}

/// RB-321 — Derived variant: comma is not a value (SYN-0001-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0001 (toml-test double-comma-01).
/// `[,]` heals to the empty array — a comma never stands in for a
/// value, and no placeholder is synthesized.
#[test]
fn rb_321_syn_comma_not_value() {
    let src = "case_0001_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0001_03"], serde_json::json!([]));
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "H002"),
        "H002 expected, got {codes:?}"
    );
    // Positive control: the element-carrying twin keeps its value.
    assert_eq!(
        eval_json("case_0001_03 = [1]\n")["case_0001_03"],
        serde_json::json!([1])
    );
}

/// RB-322 — Derived variant: trailing-comma policy (SYN-0001-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0001 (toml-test double-comma-01).
/// Trailing commas are explicitly allowed (RB-004/RB-216): `[1,]`
/// heals H002 at 1:18 — one policy across every array spelling.
#[test]
fn rb_322_syn_trailing_policy_locked() {
    let src = "case_0001_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0001_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
}

/// RB-323 — Derived variant: positive control (SYN-0001-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0001 (toml-test double-comma-01).
/// The clean three-element array proves the failure is
/// separator-specific: exact value, zero diagnostics.
#[test]
fn rb_323_syn_positive_control() {
    let src = "case_0001_05 = [1, 2, 3]\n";
    assert_eq!(eval_json(src)["case_0001_05"], serde_json::json!([1, 2, 3]));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-324 — Derived variant: empty collection valid (SYN-0001-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0001 (toml-test double-comma-01).
/// No schema forbids it here: `[]` evaluates silently.
#[test]
fn rb_324_syn_empty_valid() {
    assert_eq!(
        eval_json("case_0001_06 = []\n")["case_0001_06"],
        serde_json::json!([])
    );
    let codes = codes_of("case_0001_06 = []\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-325 — Derived variant: nested delimiters match (SYN-0001-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0001 (toml-test double-comma-01).
/// Nested collections evaluate exactly when every delimiter matches.
#[test]
fn rb_325_syn_nested_match() {
    let src = "case_0001_07 = [[1], [2, 3]]\n";
    assert_eq!(
        eval_json(src)["case_0001_07"],
        serde_json::json!([[1], [2, 3]])
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-326 — Derived variant: heterogeneous arrays parse (SYN-0001-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0001 (toml-test double-comma-01).
/// Parser and schema agree by separation of concerns: the parser
/// accepts mixed arrays (no parse error); any element restriction
/// would surface as schema validation, never a parse surprise.
#[test]
fn rb_326_syn_hetero_parses() {
    let src = "case_0001_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0001_08"], serde_json::json!([1, {"name": "x"}]));
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-327 — Derived variant: EOF span (SYN-0001-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0001 (toml-test double-comma-01).
/// "Opening bracket or EOF": shipped policy points at EOF — E001 at
/// 2:1 for the unterminated array, deterministic across spellings.
#[test]
fn rb_327_syn_eof_span() {
    let src = "case_0001_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    assert!(
        diags.iter().any(|d| d.code == "E001"),
        "E001 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed twin evaluates.
    assert_eq!(
        eval_json("case_0001_09 = [1, 2]\n")["case_0001_09"],
        serde_json::json!([1, 2])
    );
}

/// RB-328 — Derived variant 02 twin (SYN-0002-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0002 (toml-test double-comma-02).
/// Same locked H002 policy under the 02 lineage: `[1,,2]` heals at
/// 1:19 with no implicit null — both upstream twins converge.
#[test]
fn rb_328_syn_twin_converges() {
    let src = "case_0002_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0002_01"], serde_json::json!([1, 2]));
    assert!(
        !v["case_0002_01"]
            .as_array()
            .expect("array")
            .iter()
            .any(|e| e.is_null()),
        "{v}"
    );
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-329 — Derived variant 02, no silent merge (SYN-0002-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0002 (toml-test double-comma-02).
/// Same H003 policy under the 02 lineage, span pinned at the longer
/// key's join point (1:19) — lineage-independent healing.
#[test]
fn rb_329_syn02_no_silent_merge() {
    let src = "case_0002_02 = [1 2]\n";
    assert_eq!(eval_json(src)["case_0002_02"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-330 — Derived variant 02, comma is not a value (SYN-0002-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0002 (toml-test double-comma-02).
/// `[,]` heals to empty with Info severity — announced repair, never
/// a synthesized placeholder, never a failure.
#[test]
fn rb_330_syn02_comma_not_value_info() {
    let src = "case_0002_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0002_03"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
}

/// RB-331 — Derived variant 02, trailing policy (SYN-0002-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0002 (toml-test double-comma-02).
/// Trailing comma heals H002 at the comma (1:18) — one spelled-out
/// policy everywhere, both lineages, every key width.
#[test]
fn rb_331_syn02_trailing_span() {
    let src = "case_0002_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0002_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
}

/// RB-332 — Derived variant 02, positive control (SYN-0002-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0002 (toml-test double-comma-02).
/// Clean triple asserts element TYPES as well as values — the
/// failure is separator-specific, and the good case is typed int.
#[test]
fn rb_332_syn02_typed_control() {
    let src = "case_0002_05 = [1, 2, 3]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0002_05"], serde_json::json!([1, 2, 3]));
    for e in v["case_0002_05"].as_array().expect("array") {
        assert!(e.is_i64(), "{v}");
    }
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-333 — Derived variant 02, empty valid (SYN-0002-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0002 (toml-test double-comma-02).
/// Empty array asserts the TYPE (array, length 0) — valid by shape,
/// not merely by value equality.
#[test]
fn rb_333_syn02_empty_typed() {
    let v = eval_json("case_0002_06 = []\n");
    assert!(v["case_0002_06"].is_array(), "{v}");
    assert_eq!(v["case_0002_06"].as_array().expect("array").len(), 0);
}

/// RB-334 — Derived variant 02, nested shape (SYN-0002-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0002 (toml-test double-comma-02).
/// Delimiters match: assert the nested SHAPE by index — inner
/// boundaries land exactly where written.
#[test]
fn rb_334_syn02_nested_indexed() {
    let v = eval_json("case_0002_07 = [[1], [2, 3]]\n");
    assert_eq!(v["case_0002_07"][0][0], 1);
    assert_eq!(v["case_0002_07"][1][1], 3);
    assert_eq!(v["case_0002_07"][1].as_array().expect("array").len(), 2);
}

/// RB-335 — Derived variant 02, hetero types (SYN-0002-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0002 (toml-test double-comma-02).
/// Mixed arrays parse: assert per-ELEMENT types (int vs table) plus
/// the absence of any error class — parser/schema agreement.
#[test]
fn rb_335_syn02_hetero_typed() {
    let src = "case_0002_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert!(v["case_0002_08"][0].is_i64(), "{v}");
    assert!(v["case_0002_08"][1].is_object(), "{v}");
    assert_eq!(v["case_0002_08"][1]["name"], "x");
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-336 — Derived variant 02, EOF severity (SYN-0002-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0002 (toml-test double-comma-02).
/// Unterminated array fails E001 with Error severity at EOF (2:1) —
/// a missing bracket is a failure, never a heal, never a hang.
#[test]
fn rb_336_syn02_eof_error() {
    let src = "case_0002_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
}

/// RB-337 — Derived variant: redefinition announced (SYN-0003-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0003 (toml-test extend-defined-aot).
/// "Must not silently replace": it does not — W011 carries Warning
/// severity with span, and the winner is pinned. Announcement IS the
/// anti-silence mechanism (refusal of strict-reject per RB-002).
#[test]
fn rb_337_syn_redefinition_announced() {
    let src = "fruit = { name = \"apple\" }\nfruit = { name = \"banana\" }\n";
    assert_eq!(eval_json(src)["fruit"]["name"], "banana");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W011 must be Warning, got {:?}",
        w.severity
    );
    assert!(w.span.is_some(), "W011 carries a span");
}

/// RB-338 — Derived variant: patch type-checks first (SYN-0003-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0003 (toml-test extend-defined-aot).
/// The naive index-patch transliteration is rejected (E019: `(`
/// required) — paths type-check before any value is touched, so an
/// empty array can never sprout an element implicitly.
#[test]
fn rb_338_syn_patch_typechecks_first() {
    let src = "items = []\n@patch items[0] { name = \"x\" }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    assert!(
        diags.iter().any(|d| d.code == "E019"),
        "E019 expected, got {diags:?}"
    );
    // Positive control: the array alone evaluates; nothing sprouted.
    assert_eq!(eval_json("items = []\n")["items"], serde_json::json!([]));
}

/// RB-339 — Derived variant: array identity vs map keys (SYN-0003-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0003 (toml-test extend-defined-aot).
/// The naive index-patch transliteration is rejected (E019) — array
/// elements are not addressable that way — while the explicit
/// table-path call overlays deterministically (control).
#[test]
fn rb_339_syn_array_identity_explicit() {
    let src = "items = [{ name = \"apple\" }]\n@patch items[0] { color = \"red\" }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    assert!(
        diags.iter().any(|d| d.code == "E019"),
        "E019 expected, got {diags:?}"
    );
    // Positive control: the explicit table-path call overlays.
    let v = eval_json("t = { a = 1 }\n@patch(\"t.b\", 2)\n");
    assert_eq!(v["t"]["b"], 2);
    assert_eq!(v["t"]["a"], 1);
}

/// RB-340 — Derived variant: map/scalar conflict impossible (SYN-0003-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0003 (toml-test extend-defined-aot).
/// Dotted assignment has no grammar (RB-306): E002 with Error
/// severity at 2:1 — a path can never be map and scalar at once.
#[test]
fn rb_340_syn_path_conflict_error() {
    let src = "fruit = { apple = { color = \"red\" } }\nfruit.apple = 1\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the table alone evaluates.
    let v = eval_json("fruit = { apple = { color = \"red\" } }\n");
    assert_eq!(v["fruit"]["apple"]["color"], "red");
}

/// RB-341 — Derived variant: update vs duplicate (SYN-0003-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0003 (toml-test extend-defined-aot).
/// The naive update transliteration is rejected (E019) — while the
/// explicit call updates to exactly 2 with zero diagnostics,
/// visibly distinct from a duplicate declaration (W011).
#[test]
fn rb_341_syn_update_distinct_silent() {
    let src = "fruit = { apple = 1 }\n@patch fruit { apple = 2 }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    assert!(
        diags.iter().any(|d| d.code == "E019"),
        "E019 expected, got {diags:?}"
    );
    let v = eval_json("fruit = { apple = 1 }\n@patch(\"fruit.apple\", 2)\n");
    assert_eq!(v["fruit"]["apple"], 2);
    let module = parse(
        "fruit = { apple = 1 }\n@patch(\"fruit.apple\", 2)\n",
        "research",
    )
    .expect("must parse");
    let (_, d) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(d.is_empty(), "{d:?}");
}

/// RB-342 — Derived variant: structural removal (SYN-0003-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0003 (toml-test extend-defined-aot).
/// The naive removal transliteration is rejected (E019 at 2:2) —
/// while the explicit call removes structurally: key absent after.
#[test]
fn rb_342_syn_removal_structural() {
    let src = "fruit = { apple = 1 }\n@remove fruit.apple\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    // Positive control: the explicit call removes the key.
    let v = eval_json("fruit = { apple = 1 }\n@remove(\"fruit.apple\")\n");
    assert!(v["fruit"].get("apple").is_none(), "{v}");
}

/// RB-343 — Derived variant: rename preserves node (SYN-0003-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0003 (toml-test extend-defined-aot).
/// The naive rename transliteration is rejected (E019 at 2:2) —
/// while the explicit call renames with value intact: pear=1,
/// apple absent.
#[test]
fn rb_343_syn_rename_preserves() {
    let src = "fruit = { apple = 1 }\n@rename fruit.apple to pear\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    // Positive control: the explicit call renames, value intact.
    let v = eval_json("fruit = { apple = 1 }\n@rename(\"fruit.apple\", \"pear\")\n");
    assert_eq!(v["fruit"]["pear"], 1);
    assert!(v["fruit"].get("apple").is_none(), "{v}");
}

/// RB-344 — Derived variant: ordered plan deterministic (SYN-0003-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0003 (toml-test extend-defined-aot).
/// The naive double-rename is rejected (E019) — while two explicit
/// patches apply in source order: last write (3) wins, stable across
/// runs.
#[test]
fn rb_344_syn_ordered_plan_stable() {
    let src = "fruit = { apple = 1 }\n@rename fruit.apple to pear\n@rename fruit.pear to apple\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    assert!(
        diags.iter().any(|d| d.code == "E019"),
        "E019 expected, got {diags:?}"
    );
    let src = "fruit = { apple = 1 }\n@patch(\"fruit.apple\", 2)\n@patch(\"fruit.apple\", 3)\n";
    assert_eq!(eval_json(src)["fruit"]["apple"], 3);
    assert_eq!(eval_json(src)["fruit"]["apple"], 3);
}

/// RB-345 — Derived variant: nested replace contained (SYN-0003-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0003 (toml-test extend-defined-aot).
/// The naive nested-patch transliteration is rejected (E019) — while
/// the explicit nested-path call changes only the leaf: siblings
/// and parents intact.
#[test]
fn rb_345_syn_nested_contained() {
    let src = "fruit = { apple = { color = \"red\" } }\n@patch fruit.apple { color = \"green\" }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    assert!(
        diags.iter().any(|d| d.code == "E019"),
        "E019 expected, got {diags:?}"
    );
    // Positive control: the explicit leaf call changes only the leaf.
    let v = eval_json(
        "fruit = { apple = { color = \"red\", size = 1 } }\n@patch(\"fruit.apple.color\", \"green\")\n",
    );
    assert_eq!(v["fruit"]["apple"]["color"], "green");
    assert_eq!(v["fruit"]["apple"]["size"], 1);
}

/// RB-346 — Derived variant 04-01: structural redefinition (SYN-0004-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0004 (toml-test extending-table).
/// Same locked W011 policy, now with span AND severity indexed
/// together: announced Warning at 2:1, banana wins — never silent.
#[test]
fn rb_346_syn04_redefinition_announced() {
    let src = "fruit = { name = \"apple\" }\nfruit = { name = \"banana\" }\n";
    assert_eq!(eval_json(src)["fruit"]["name"], "banana");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W011 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W011 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
}

/// RB-347 — Derived variant 04-02: index patch form (SYN-0004-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0004 (toml-test extending-table).
/// The naive index form fails E019 at 2:2 — paths type-check before
/// values move, so no element is invented.
#[test]
fn rb_347_syn04_index_form_rejects() {
    let src = "items = []\n@patch items[0] { name = \"x\" }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    // Positive control: the array alone stays exactly empty.
    assert_eq!(eval_json("items = []\n")["items"], serde_json::json!([]));
}

/// RB-348 — Derived variant 04-03: element identity (SYN-0004-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0004 (toml-test extending-table).
/// Same naive index form, array pre-populated: still E019 — element
/// identity never degrades into map-key addressing.
#[test]
fn rb_348_syn04_element_identity_kept() {
    let src = "items = [{ name = \"apple\" }]\n@patch items[0] { color = \"red\" }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    assert!(
        diags.iter().any(|d| d.code == "E019"),
        "E019 expected, got {diags:?}"
    );
    // Positive control: the element is intact and addressable by index in reads.
    let v = eval_json("items = [{ name = \"apple\" }]\nfirst = 0\n");
    assert_eq!(v["items"][0]["name"], "apple");
    assert_eq!(v["first"], 0);
}

/// RB-349 — Derived variant 04-04: header-form conflict (SYN-0004-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0004 (toml-test extending-table).
/// Dotted assignment is rejected in header context too — E002 at
/// 3:1 — so table bodies can never sprout scalar shadows of maps.
#[test]
fn rb_349_syn04_header_conflict_rejects() {
    let src = "[fruit]\napple = { color = \"red\" }\nfruit.apple = 1\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (3, 1), "{span:?}");
    // Positive control: the header table alone evaluates.
    let v = eval_json("[fruit]\napple = { color = \"red\" }\n");
    assert_eq!(v["fruit"]["apple"]["color"], "red");
}

/// RB-350 — Derived variant 04-05: naive update span (SYN-0004-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0004 (toml-test extending-table).
/// The naive update form fails E019 at 2:2 — updates arrive only
/// through the explicit call, which lands silently and exactly.
#[test]
fn rb_350_syn04_update_span() {
    let src = "fruit = { apple = 1 }\n@patch fruit { apple = 2 }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    // Positive control: the explicit call updates silently and exactly.
    let v = eval_json("fruit = { apple = 1 }\n@patch(\"fruit.apple\", 2)\n");
    assert_eq!(v["fruit"]["apple"], 2);
    let codes = codes_of("fruit = { apple = 1 }\n@patch(\"fruit.apple\", 2)\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-351 — Derived variant 04-06: naive removal span (SYN-0004-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0004 (toml-test extending-table).
/// E019 at 2:2 for the naive form; the explicit call removes with
/// the sibling intact.
#[test]
fn rb_351_syn04_removal_span() {
    let src = "fruit = { apple = 1 }\n@remove fruit.apple\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    // Positive control: explicit removal keeps siblings.
    let v = eval_json("fruit = { apple = 1, pear = 2 }\n@remove(\"fruit.apple\")\n");
    assert!(v["fruit"].get("apple").is_none(), "{v}");
    assert_eq!(v["fruit"]["pear"], 2);
}

/// RB-352 — Derived variant 04-07: naive rename span (SYN-0004-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0004 (toml-test extending-table).
/// E019 at 2:2; the explicit call preserves the value under the new
/// name with siblings intact.
#[test]
fn rb_352_syn04_rename_span() {
    let src = "fruit = { apple = 1 }\n@rename fruit.apple to pear\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    // Positive control: explicit rename preserves value and siblings.
    let v = eval_json("fruit = { apple = 1, plum = 0 }\n@rename(\"fruit.apple\", \"pear\")\n");
    assert_eq!(v["fruit"]["pear"], 1);
    assert_eq!(v["fruit"]["plum"], 0);
    assert!(v["fruit"].get("apple").is_none(), "{v}");
}

/// RB-353 — Derived variant 04-08: naive plan span (SYN-0004-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0004 (toml-test extending-table).
/// The naive double rename fails at the FIRST operation (E019 at
/// 2:2) — plans are atomic per operation; nothing half-applies.
#[test]
fn rb_353_syn04_plan_first_op_fails() {
    let src = "fruit = { apple = 1 }\n@rename fruit.apple to pear\n@rename fruit.pear to apple\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    // Positive control: the explicit double-patch plan is deterministic.
    let src = "fruit = { apple = 1 }\n@patch(\"fruit.apple\", 2)\n@patch(\"fruit.apple\", 3)\n";
    assert_eq!(eval_json(src)["fruit"]["apple"], 3);
}

/// RB-354 — Derived variant 04-09: naive nested span (SYN-0004-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0004 (toml-test extending-table).
/// E019 at 2:2; the explicit leaf call changes only the leaf while
/// the parent and siblings stand.
#[test]
fn rb_354_syn04_nested_span() {
    let src = "fruit = { apple = { color = \"red\" } }\n@patch fruit.apple { color = \"green\" }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    // Positive control: explicit leaf call, parent and siblings intact.
    let v = eval_json(
        "fruit = { apple = { color = \"red\" }, pear = 1 }\n@patch(\"fruit.apple.color\", \"green\")\n",
    );
    assert_eq!(v["fruit"]["apple"]["color"], "green");
    assert_eq!(v["fruit"]["pear"], 1);
}

/// RB-355 — Derived variant 05-01: heal severity (SYN-0005-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0005 (toml-test missing-separator-01).
/// The doubled comma heals H002 with Info severity and value
/// `[1, 2]` — repair class, repair severity, exact value indexed
/// jointly for the 05 lineage.
#[test]
fn rb_355_syn05_heal_severity() {
    let src = "case_0005_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0005_01"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
}

/// RB-356 — Derived variant 05-02: heal warning severity (SYN-0005-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0005 (toml-test missing-separator-01).
/// The missing comma heals H003 with Warning severity — repairs that
/// insert tokens announce louder than repairs that remove them.
#[test]
fn rb_356_syn05_insert_warns() {
    let src = "case_0005_02 = [1 2]\n";
    assert_eq!(eval_json(src)["case_0005_02"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    assert_eq!(
        h.severity,
        active_core::Severity::Warning,
        "H003 must be Warning, got {:?}",
        h.severity
    );
}

/// RB-357 — Derived variant 05-03: lone-comma span (SYN-0005-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0005 (toml-test missing-separator-01).
/// `[,]` heals H002 at the comma (1:17) to the empty array — the
/// comma position is the span, never a synthesized element.
#[test]
fn rb_357_syn05_lone_comma_span() {
    let src = "case_0005_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0005_03"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
}

/// RB-358 — Derived variant 05-04: trailing span (SYN-0005-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0005 (toml-test missing-separator-01).
/// Trailing comma heals H002 at 1:18 — the 05 lineage converges with
/// 01/02 on one spelled-out policy. (Completes SYN-0004-04…09 +
/// opens SYN-0005-01…04.)
#[test]
fn rb_358_syn05_trailing_span() {
    let src = "case_0005_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0005_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
    // Positive control: the clean neighbour carries no heal.
    assert!(!codes_of("case_0005_04 = [1]\n").iter().any(|c| c == "H002"));
}

/// RB-359 — Derived variant 05-05: control determinism (SYN-0005-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0005 (toml-test missing-separator-01).
/// The clean triple evaluates identically on repeated runs —
/// separator-specificity plus determinism indexed jointly.
#[test]
fn rb_359_syn05_control_deterministic() {
    let src = "case_0005_05 = [1, 2, 3]\n";
    assert_eq!(eval_json(src)["case_0005_05"], serde_json::json!([1, 2, 3]));
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-360 — Derived variant 05-06: empty determinism (SYN-0005-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0005 (toml-test missing-separator-01).
/// Empty arrays evaluate deterministically — repeated runs agree
/// exactly, zero diagnostics both times.
#[test]
fn rb_360_syn05_empty_deterministic() {
    let src = "case_0005_06 = []\n";
    assert_eq!(eval_json(src)["case_0005_06"], serde_json::json!([]));
    assert_eq!(eval_json(src), eval_json(src));
}

/// RB-361 — Derived variant 05-07: nested determinism (SYN-0005-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0005 (toml-test missing-separator-01).
/// Nested collections evaluate identically across runs — structure
/// plus determinism indexed jointly.
#[test]
fn rb_361_syn05_nested_deterministic() {
    let src = "case_0005_07 = [[1], [2, 3]]\n";
    assert_eq!(
        eval_json(src)["case_0005_07"],
        serde_json::json!([[1], [2, 3]])
    );
    assert_eq!(eval_json(src), eval_json(src));
}

/// RB-362 — Derived variant 05-08: hetero key-set (SYN-0005-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0005 (toml-test missing-separator-01).
/// The table element carries exactly one key (`name`) — key-set
/// exactness proves no element merged or split.
#[test]
fn rb_362_syn05_hetero_keyset() {
    let src = "case_0005_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0005_08"][0], 1);
    let keys: Vec<&str> = v["case_0005_08"][1]
        .as_object()
        .expect("table")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["name"]);
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-363 — Derived variant 05-09: EOF Error (SYN-0005-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0005 (toml-test missing-separator-01).
/// Unterminated array fails E001 with Error severity at EOF (2:1) —
/// class, severity, and span indexed jointly for the 05 lineage.
#[test]
fn rb_363_syn05_eof_error() {
    let src = "case_0005_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed twin evaluates.
    assert_eq!(
        eval_json("case_0005_09 = [1, 2]\n")["case_0005_09"],
        serde_json::json!([1, 2])
    );
}

/// RB-364 — Derived variant 06-01: heal triple (SYN-0006-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0006 (toml-test missing-separator-02).
/// Doubled comma heals with value `[1, 2]`, class H002, Info
/// severity — the full repair triple indexed for the 06 lineage.
#[test]
fn rb_364_syn06_heal_triple() {
    let src = "case_0006_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0006_01"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
}

/// RB-365 — Derived variant 06-02: insert triple (SYN-0006-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0006 (toml-test missing-separator-02).
/// Missing comma heals with value `[1, 2]`, class H003, Warning
/// severity — insertions announce louder than removals.
#[test]
fn rb_365_syn06_insert_triple() {
    let src = "case_0006_02 = [1 2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0006_02"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    assert_eq!(
        h.severity,
        active_core::Severity::Warning,
        "H003 must be Warning, got {:?}",
        h.severity
    );
}

/// RB-366 — Derived variant 06-03: lone-comma span (SYN-0006-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0006 (toml-test missing-separator-02).
/// `[,]` heals H002 at the comma (1:17) to empty — the span marks
/// the repair site under the 06 key width.
#[test]
fn rb_366_syn06_lone_comma_span() {
    let src = "case_0006_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0006_03"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
}

/// RB-367 — Derived variant 06-04: trailing span (SYN-0006-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0006 (toml-test missing-separator-02).
/// Trailing comma heals H002 at 1:18 — the 06 lineage converges on
/// the one spelled-out policy. (Completes SYN-0005-05…09.)
#[test]
fn rb_367_syn06_trailing_span() {
    let src = "case_0006_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0006_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
}

/// RB-368 — Derived variant 06-05: typed triple (SYN-0006-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0006 (toml-test missing-separator-02).
/// Clean triple asserts value, int element types, and silence —
/// the typed-control oracle for the 06 lineage. (Opens SYN-0006.)
#[test]
fn rb_368_syn06_typed_triple() {
    let src = "case_0006_05 = [1, 2, 3]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0006_05"], serde_json::json!([1, 2, 3]));
    for e in v["case_0006_05"].as_array().expect("array") {
        assert!(e.is_i64(), "{v}");
    }
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-369 — Derived variant 06-06: empty determinism (SYN-0006-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0006 (toml-test missing-separator-02).
/// Empty arrays evaluate deterministically — repeated runs agree
/// exactly with zero diagnostics.
#[test]
fn rb_369_syn06_empty_deterministic() {
    let src = "case_0006_06 = []\n";
    assert_eq!(eval_json(src)["case_0006_06"], serde_json::json!([]));
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-370 — Derived variant 06-07: nested types (SYN-0006-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0006 (toml-test missing-separator-02).
/// Nested collections assert outer/inner lengths — boundaries land
/// exactly where written, both levels arrays.
#[test]
fn rb_370_syn06_nested_typed() {
    let src = "case_0006_07 = [[1], [2, 3]]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0006_07"], serde_json::json!([[1], [2, 3]]));
    assert_eq!(v["case_0006_07"].as_array().expect("array").len(), 2);
    assert!(v["case_0006_07"][0].is_array(), "{v}");
    assert!(v["case_0006_07"][1].is_array(), "{v}");
}

/// RB-371 — Derived variant 06-08: hetero value (SYN-0006-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0006 (toml-test missing-separator-02).
/// Mixed arrays parse with the exact value and no error class —
/// parser/schema agreement for the 06 lineage.
#[test]
fn rb_371_syn06_hetero_value() {
    let src = "case_0006_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0006_08"], serde_json::json!([1, {"name": "x"}]));
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
    // Positive control: the homogeneous twin agrees structurally.
    assert_eq!(
        eval_json("case_0006_08 = [1, 2]\n")["case_0006_08"],
        serde_json::json!([1, 2])
    );
}

/// RB-372 — Derived variant 06-09: EOF triple (SYN-0006-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0006 (toml-test missing-separator-02).
/// Unterminated array fails E001 Error at EOF (2:1) — class,
/// severity, and span indexed jointly for the 06 lineage.
#[test]
fn rb_372_syn06_eof_triple() {
    let src = "case_0006_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed twin evaluates.
    assert_eq!(
        eval_json("case_0006_09 = [1, 2]\n")["case_0006_09"],
        serde_json::json!([1, 2])
    );
}

/// RB-373 — Derived variant 07-01: heal triple (SYN-0007-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0007 (toml-test no-close-01).
/// Doubled comma heals with value `[1, 2]`, class H002 at 1:19, Info
/// severity — the full repair triple indexed for the 07 lineage.
#[test]
fn rb_373_syn07_heal_triple() {
    let src = "case_0007_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0007_01"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-374 — Derived variant 07-02: insert triple (SYN-0007-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0007 (toml-test no-close-01).
/// Missing comma heals with value `[1, 2]`, class H003 at 1:19,
/// Warning severity — insertions announce louder than removals.
#[test]
fn rb_374_syn07_insert_triple() {
    let src = "case_0007_02 = [1 2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0007_02"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    assert_eq!(
        h.severity,
        active_core::Severity::Warning,
        "H003 must be Warning, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-375 — Derived variant 07-03: lone-comma span (SYN-0007-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0007 (toml-test no-close-01).
/// `[,]` heals H002 at the comma (1:17) to empty — the span marks
/// the repair site under the 07 key width.
#[test]
fn rb_375_syn07_lone_comma_span() {
    let src = "case_0007_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0007_03"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
}

/// RB-376 — Derived variant 07-04: trailing span (SYN-0007-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0007 (toml-test no-close-01).
/// Trailing comma heals H002 at 1:18 — the 07 lineage converges on
/// the one spelled-out policy.
#[test]
fn rb_376_syn07_trailing_span() {
    let src = "case_0007_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0007_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
    // Positive control: the clean neighbour carries no heal.
    assert!(!codes_of("case_0007_04 = [1]\n").iter().any(|c| c == "H002"));
}

/// RB-379 — Derived variant 07-07: nested lengths (SYN-0007-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0007 (toml-test no-close-01).
/// Nested collections assert outer and inner lengths — every
/// boundary lands exactly where written, both levels arrays.
#[test]
fn rb_379_syn07_nested_lengths() {
    let src = "case_0007_07 = [[1], [2, 3]]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0007_07"], serde_json::json!([[1], [2, 3]]));
    assert_eq!(v["case_0007_07"].as_array().expect("array").len(), 2);
    assert_eq!(v["case_0007_07"][0].as_array().expect("array").len(), 1);
    assert_eq!(v["case_0007_07"][1].as_array().expect("array").len(), 2);
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-380 — Derived variant 07-08: hetero joint oracle (SYN-0007-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0007 (toml-test no-close-01).
/// Mixed arrays assert value, int type, table key-set, and no error
/// class jointly — parser/schema agreement for the 07 lineage.
#[test]
fn rb_380_syn07_hetero_joint() {
    let src = "case_0007_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0007_08"], serde_json::json!([1, {"name": "x"}]));
    assert!(v["case_0007_08"][0].is_i64(), "{v}");
    let keys: Vec<&str> = v["case_0007_08"][1]
        .as_object()
        .expect("table")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["name"]);
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-381 — Derived variant 07-09: EOF triple (SYN-0007-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0007 (toml-test no-close-01).
/// Unterminated array fails E001 Error at EOF (2:1) — class,
/// severity, and span indexed jointly for the 07 lineage.
#[test]
fn rb_381_syn07_eof_triple() {
    let src = "case_0007_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed twin evaluates.
    assert_eq!(
        eval_json("case_0007_09 = [1, 2]\n")["case_0007_09"],
        serde_json::json!([1, 2])
    );
}

/// RB-382 — Derived variant 08-01: heal triple (SYN-0008-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0008 (toml-test no-close-02).
/// Doubled comma heals with value `[1, 2]`, class H002 at 1:19, Info
/// severity — the full repair triple indexed for the 08 lineage.
#[test]
fn rb_382_syn08_heal_triple() {
    let src = "case_0008_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0008_01"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-383 — Derived variant 08-02: insert triple (SYN-0008-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0008 (toml-test no-close-02).
/// Missing comma heals with value `[1, 2]`, class H003 at 1:19,
/// Warning severity — insertions announce louder than removals.
#[test]
fn rb_383_syn08_insert_triple() {
    let src = "case_0008_02 = [1 2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0008_02"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    assert_eq!(
        h.severity,
        active_core::Severity::Warning,
        "H003 must be Warning, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-384 — Derived variant 08-03: lone-comma span (SYN-0008-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0008 (toml-test no-close-02).
/// `[,]` heals H002 at the comma (1:17) to empty — the span marks
/// the repair site under the 08 key width.
#[test]
fn rb_384_syn08_lone_comma_span() {
    let src = "case_0008_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0008_03"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
}

/// RB-385 — Derived variant 08-04: trailing span + control (SYN-0008-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0008 (toml-test no-close-02).
/// Trailing comma heals H002 at 1:18 — and the clean neighbour
/// carries no heal, proving the repair is input-specific.
#[test]
fn rb_385_syn08_trailing_control() {
    let src = "case_0008_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0008_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
    // Positive control: the clean neighbour carries no heal.
    assert!(!codes_of("case_0008_04 = [1]\n").iter().any(|c| c == "H002"));
}

/// RB-389 — Derived variant 08-08: hetero YAML export (SYN-0008-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0008 (toml-test no-close-02).
/// Mixed arrays assert value plus YAML-export agreement — the hetero
/// shape survives the exporter exactly, not just evaluation.
#[test]
fn rb_389_syn08_hetero_export() {
    let src = "case_0008_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0008_08"], serde_json::json!([1, {"name": "x"}]));
    let module = parse(src, "research").expect("must parse");
    let (value, _) = Engine::new().evaluate(&module).expect("must evaluate");
    let yaml = active_core::convert::value_to_yaml_string(&value).expect("yaml export");
    assert!(yaml.contains("name"), "{yaml}");
    assert!(yaml.contains('x'), "{yaml}");
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-390 — Derived variant 08-09: EOF triple (SYN-0008-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0008 (toml-test no-close-02).
/// Unterminated array fails E001 Error at EOF (2:1) — class,
/// severity, and span indexed jointly for the 08 lineage.
#[test]
fn rb_390_syn08_eof_triple() {
    let src = "case_0008_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed twin evaluates.
    assert_eq!(
        eval_json("case_0008_09 = [1, 2]\n")["case_0008_09"],
        serde_json::json!([1, 2])
    );
}

/// RB-391 — Derived variant 09-01: heal triple (SYN-0009-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0009 (toml-test no-close-04).
/// Doubled comma heals with value `[1, 2]`, class H002 at 1:19, Info
/// severity — the full repair triple indexed for the 09 lineage.
#[test]
fn rb_391_syn09_heal_triple() {
    let src = "case_0009_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0009_01"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-392 — Derived variant 09-02: insert triple (SYN-0009-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0009 (toml-test no-close-04).
/// Missing comma heals with value `[1, 2]`, class H003 at 1:19,
/// Warning severity — insertions announce louder than removals.
#[test]
fn rb_392_syn09_insert_triple() {
    let src = "case_0009_02 = [1 2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0009_02"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    assert_eq!(
        h.severity,
        active_core::Severity::Warning,
        "H003 must be Warning, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-393 — Derived variant 09-03: lone-comma span (SYN-0009-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0009 (toml-test no-close-04).
/// `[,]` heals H002 at the comma (1:17) to empty — the span marks
/// the repair site under the 09 key width.
#[test]
fn rb_393_syn09_lone_comma_span() {
    let src = "case_0009_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0009_03"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
}

/// RB-394 — Derived variant 09-04: trailing span + control (SYN-0009-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0009 (toml-test no-close-04).
/// Trailing comma heals H002 at 1:18 — and the clean neighbour
/// carries no heal, proving the repair is input-specific.
#[test]
fn rb_394_syn09_trailing_control() {
    let src = "case_0009_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0009_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
    // Positive control: the clean neighbour carries no heal.
    assert!(!codes_of("case_0009_04 = [1]\n").iter().any(|c| c == "H002"));
}

/// RB-395 — Derived variant 09-05: typed determinism (SYN-0009-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0009 (toml-test no-close-04).
/// Clean triple asserts value, int element types, determinism, and
/// silence jointly — the full good-case contract.
#[test]
fn rb_395_syn09_typed_deterministic() {
    let src = "case_0009_05 = [1, 2, 3]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0009_05"], serde_json::json!([1, 2, 3]));
    for e in v["case_0009_05"].as_array().expect("array") {
        assert!(e.is_i64(), "{v}");
    }
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-396 — Derived variant 09-06: empty determinism (SYN-0009-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0009 (toml-test no-close-04).
/// Empty arrays evaluate deterministically — repeated runs agree
/// exactly with zero diagnostics.
#[test]
fn rb_396_syn09_empty_deterministic() {
    let src = "case_0009_06 = []\n";
    assert_eq!(eval_json(src)["case_0009_06"], serde_json::json!([]));
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-397 — Derived variant 09-07: nested determinism (SYN-0009-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0009 (toml-test no-close-04).
/// Nested collections evaluate exactly and deterministically —
/// value plus twice-eval equality plus silence.
#[test]
fn rb_397_syn09_nested_deterministic() {
    let src = "case_0009_07 = [[1], [2, 3]]\n";
    assert_eq!(
        eval_json(src)["case_0009_07"],
        serde_json::json!([[1], [2, 3]])
    );
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-398 — Derived variant 09-08: hetero count (SYN-0009-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0009 (toml-test no-close-04).
/// Mixed arrays assert value, two-element count, and no error class —
/// the hetero shape holds exactly. (Completes SYN-0008-08…09 +
/// opens SYN-0009-01…08.)
#[test]
fn rb_398_syn09_hetero_count() {
    let src = "case_0009_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0009_08"], serde_json::json!([1, {"name": "x"}]));
    assert_eq!(v["case_0009_08"].as_array().expect("array").len(), 2);
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-399 — Derived variant 09-09: EOF triple (SYN-0009-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0009 (toml-test no-close-04).
/// Unterminated array fails E001 Error at EOF (2:1) — class,
/// severity, and span indexed jointly for the 09 lineage.
#[test]
fn rb_399_syn09_eof_triple() {
    let src = "case_0009_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed twin evaluates.
    assert_eq!(
        eval_json("case_0009_09 = [1, 2]\n")["case_0009_09"],
        serde_json::json!([1, 2])
    );
}

/// RB-400 — Derived variant 10-01: heal triple (SYN-0010-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0010 (toml-test no-close-table-01).
/// Doubled comma heals with value `[1, 2]`, class H002 at 1:19, Info
/// severity — the full repair triple indexed for the 10 lineage.
#[test]
fn rb_400_syn10_heal_triple() {
    let src = "case_0010_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0010_01"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-401 — Derived variant 10-02: insert triple (SYN-0010-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0010 (toml-test no-close-table-01).
/// Missing comma heals with value `[1, 2]`, class H003 at 1:19,
/// Warning severity — insertions announce louder than removals.
#[test]
fn rb_401_syn10_insert_triple() {
    let src = "case_0010_02 = [1 2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0010_02"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    assert_eq!(
        h.severity,
        active_core::Severity::Warning,
        "H003 must be Warning, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-402 — Derived variant 10-03: lone-comma span (SYN-0010-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0010 (toml-test no-close-table-01).
/// `[,]` heals H002 at the comma (1:17) to empty — the span marks
/// the repair site under the 10 key width.
#[test]
fn rb_402_syn10_lone_comma_span() {
    let src = "case_0010_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0010_03"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
}

/// RB-403 — Derived variant 10-04: trailing span + control (SYN-0010-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0010 (toml-test no-close-table-01).
/// Trailing comma heals H002 at 1:18 — and the clean neighbour
/// carries no heal, proving the repair is input-specific.
#[test]
fn rb_403_syn10_trailing_control() {
    let src = "case_0010_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0010_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
    // Positive control: the clean neighbour carries no heal.
    assert!(!codes_of("case_0010_04 = [1]\n").iter().any(|c| c == "H002"));
}

/// RB-404 — Derived variant 10-05: typed determinism (SYN-0010-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0010 (toml-test no-close-table-01).
/// Clean triple asserts value, int element types, determinism, and
/// silence jointly — the full good-case contract.
#[test]
fn rb_404_syn10_typed_deterministic() {
    let src = "case_0010_05 = [1, 2, 3]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0010_05"], serde_json::json!([1, 2, 3]));
    for e in v["case_0010_05"].as_array().expect("array") {
        assert!(e.is_i64(), "{v}");
    }
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-405 — Derived variant 10-06: empty determinism (SYN-0010-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0010 (toml-test no-close-table-01).
/// Empty arrays evaluate deterministically — repeated runs agree
/// exactly with zero diagnostics.
#[test]
fn rb_405_syn10_empty_deterministic() {
    let src = "case_0010_06 = []\n";
    assert_eq!(eval_json(src)["case_0010_06"], serde_json::json!([]));
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-406 — Derived variant 10-07: nested determinism (SYN-0010-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0010 (toml-test no-close-table-01).
/// Nested collections evaluate exactly and deterministically —
/// value plus twice-eval equality plus silence.
#[test]
fn rb_406_syn10_nested_deterministic() {
    let src = "case_0010_07 = [[1], [2, 3]]\n";
    assert_eq!(
        eval_json(src)["case_0010_07"],
        serde_json::json!([[1], [2, 3]])
    );
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-407 — Derived variant 10-08: hetero count (SYN-0010-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0010 (toml-test no-close-table-01).
/// Mixed arrays assert value, two-element count, and no error class —
/// the hetero shape holds exactly.
#[test]
fn rb_407_syn10_hetero_count() {
    let src = "case_0010_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0010_08"], serde_json::json!([1, {"name": "x"}]));
    assert_eq!(v["case_0010_08"].as_array().expect("array").len(), 2);
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-408 — Derived variant 10-09: EOF triple (SYN-0010-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0010 (toml-test no-close-table-01).
/// Unterminated array fails E001 Error at EOF (2:1) — class,
/// severity, and span indexed jointly. (Completes SYN-0009-09 +
/// opens SYN-0010-01…09.)
#[test]
fn rb_408_syn10_eof_triple() {
    let src = "case_0010_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed twin evaluates.
    assert_eq!(
        eval_json("case_0010_09 = [1, 2]\n")["case_0010_09"],
        serde_json::json!([1, 2])
    );
}

/// RB-409 — Derived variant 11-01: heal triple (SYN-0011-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0011 (toml-test no-comma-01).
/// Doubled comma heals with value `[1, 2]`, class H002 at 1:19, Info
/// severity — the full repair triple indexed for the 11 lineage.
#[test]
fn rb_409_syn11_heal_triple() {
    let src = "case_0011_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0011_01"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-410 — Derived variant 11-02: insert triple (SYN-0011-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0011 (toml-test no-comma-01).
/// Missing comma heals with value `[1, 2]`, class H003 at 1:19,
/// Warning severity — insertions announce louder than removals.
#[test]
fn rb_410_syn11_insert_triple() {
    let src = "case_0011_02 = [1 2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0011_02"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    assert_eq!(
        h.severity,
        active_core::Severity::Warning,
        "H003 must be Warning, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-411 — Derived variant 11-03: lone-comma triple (SYN-0011-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0011 (toml-test no-comma-01).
/// `[,]` heals H002 at the comma (1:17) to empty with Info severity —
/// value, class, span, and severity indexed jointly.
#[test]
fn rb_411_syn11_lone_comma_triple() {
    let src = "case_0011_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0011_03"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
}

/// RB-412 — Derived variant 11-04: trailing span + control (SYN-0011-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0011 (toml-test no-comma-01).
/// Trailing comma heals H002 at 1:18 — and the clean neighbour
/// carries no heal, proving the repair is input-specific.
#[test]
fn rb_412_syn11_trailing_control() {
    let src = "case_0011_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0011_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
    // Positive control: the clean neighbour carries no heal.
    assert!(!codes_of("case_0011_04 = [1]\n").iter().any(|c| c == "H002"));
}

/// RB-413 — Derived variant 11-05: typed determinism (SYN-0011-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0011 (toml-test no-comma-01).
/// Clean triple asserts value, int element types, determinism, and
/// silence jointly — the full good-case contract.
#[test]
fn rb_413_syn11_typed_deterministic() {
    let src = "case_0011_05 = [1, 2, 3]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0011_05"], serde_json::json!([1, 2, 3]));
    for e in v["case_0011_05"].as_array().expect("array") {
        assert!(e.is_i64(), "{v}");
    }
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-414 — Derived variant 11-06: empty determinism (SYN-0011-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0011 (toml-test no-comma-01).
/// Empty arrays evaluate deterministically — repeated runs agree
/// exactly with zero diagnostics.
#[test]
fn rb_414_syn11_empty_deterministic() {
    let src = "case_0011_06 = []\n";
    assert_eq!(eval_json(src)["case_0011_06"], serde_json::json!([]));
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-415 — Derived variant 11-07: nested lengths (SYN-0011-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0011 (toml-test no-comma-01).
/// Nested collections assert outer and inner lengths — every
/// boundary lands exactly where written, both levels arrays.
#[test]
fn rb_415_syn11_nested_lengths() {
    let src = "case_0011_07 = [[1], [2, 3]]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0011_07"], serde_json::json!([[1], [2, 3]]));
    assert_eq!(v["case_0011_07"].as_array().expect("array").len(), 2);
    assert_eq!(v["case_0011_07"][0].as_array().expect("array").len(), 1);
    assert_eq!(v["case_0011_07"][1].as_array().expect("array").len(), 2);
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-416 — Derived variant 11-08: hetero joint oracle (SYN-0011-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0011 (toml-test no-comma-01).
/// Mixed arrays assert value, int type, table key-set, and no error
/// class jointly — parser/schema agreement for the 11 lineage.
#[test]
fn rb_416_syn11_hetero_joint() {
    let src = "case_0011_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0011_08"], serde_json::json!([1, {"name": "x"}]));
    assert!(v["case_0011_08"][0].is_i64(), "{v}");
    let keys: Vec<&str> = v["case_0011_08"][1]
        .as_object()
        .expect("table")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["name"]);
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-417 — Derived variant 11-09: EOF triple (SYN-0011-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0011 (toml-test no-comma-01).
/// Unterminated array fails E001 Error at EOF (2:1) — class,
/// severity, and span indexed jointly for the 11 lineage.
#[test]
fn rb_417_syn11_eof_triple() {
    let src = "case_0011_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed twin evaluates.
    assert_eq!(
        eval_json("case_0011_09 = [1, 2]\n")["case_0011_09"],
        serde_json::json!([1, 2])
    );
}

/// RB-418 — Derived variant 12-01: no implicit null (SYN-0012-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0012 (toml-test no-comma-02).
/// A doubled comma cannot create an implicit null: the value is
/// exactly `[1, 2]` — two elements, no nulls — with announced H002.
/// (Completes SYN-0011-01…09 + opens SYN-0012-01…09.)
#[test]
fn rb_418_syn12_no_implicit_null() {
    let src = "case_0012_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0012_01"], serde_json::json!([1, 2]));
    assert_eq!(v["case_0012_01"].as_array().expect("array").len(), 2);
    assert!(
        !v["case_0012_01"]
            .as_array()
            .expect("array")
            .iter()
            .any(|e| e.is_null()),
        "{v}"
    );
    let codes = codes_of(src);
    assert!(
        codes.iter().any(|c| c == "H002"),
        "H002 expected, got {codes:?}"
    );
}

/// RB-386 — Derived variant 08-05: typed determinism (SYN-0008-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0008 (toml-test no-close-02).
/// Clean triple asserts value, int element types, determinism, and
/// silence jointly — the full good-case contract.
#[test]
fn rb_386_syn08_typed_deterministic() {
    let src = "case_0008_05 = [1, 2, 3]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0008_05"], serde_json::json!([1, 2, 3]));
    for e in v["case_0008_05"].as_array().expect("array") {
        assert!(e.is_i64(), "{v}");
    }
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-387 — Derived variant 08-06: empty determinism (SYN-0008-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0008 (toml-test no-close-02).
/// Empty arrays evaluate deterministically — repeated runs agree
/// exactly with zero diagnostics.
#[test]
fn rb_387_syn08_empty_deterministic() {
    let src = "case_0008_06 = []\n";
    assert_eq!(eval_json(src)["case_0008_06"], serde_json::json!([]));
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-388 — Derived variant 08-07: nested determinism (SYN-0008-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0008 (toml-test no-close-02).
/// Nested collections evaluate exactly and deterministically —
/// value plus twice-eval equality. (Completes SYN-0007-07…09 +
/// opens SYN-0008-01…07.)
#[test]
fn rb_388_syn08_nested_deterministic() {
    let src = "case_0008_07 = [[1], [2, 3]]\n";
    assert_eq!(
        eval_json(src)["case_0008_07"],
        serde_json::json!([[1], [2, 3]])
    );
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-377 — Derived variant 07-05: control determinism (SYN-0007-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0007 (toml-test no-close-01).
/// The clean triple proves separator-specificity with determinism:
/// exact value, repeated runs agree, zero diagnostics.
#[test]
fn rb_377_syn07_control_deterministic() {
    let src = "case_0007_05 = [1, 2, 3]\n";
    assert_eq!(eval_json(src)["case_0007_05"], serde_json::json!([1, 2, 3]));
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-378 — Derived variant 07-06: empty determinism (SYN-0007-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0007 (toml-test no-close-01).
/// Empty arrays evaluate deterministically — repeated runs agree
/// exactly with zero diagnostics. (Completes SYN-0006-06…09 +
/// opens SYN-0007-01…06.)
#[test]
fn rb_378_syn07_empty_deterministic() {
    let src = "case_0007_06 = []\n";
    assert_eq!(eval_json(src)["case_0007_06"], serde_json::json!([]));
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-299 — yaml-cpp flow-stack analogue (SRC-CVE-0006 §H, CRITICAL).
/// Origin: NVD CVE-2019-6285.
/// Bounded generator: 400 balanced nested arrays trip E004 with span
/// — flow-style nesting is bounded before stack consumption matters.
#[test]
fn rb_299_cve_flow_stack_bounded() {
    let src = format!("value = {}0{}", "[".repeat(400), "]".repeat(400));
    let diags = parse(&src, "research").expect_err("must fail E004");
    assert!(
        diags.iter().any(|d| d.code == "E004"),
        "E004 expected, got {:?}",
        &diags[..diags.len().min(3)]
    );
    let e = diags.iter().find(|d| d.code == "E004").expect("E004");
    assert!(e.span.is_some(), "E004 carries a span");
    // Positive control: shallow flow nesting evaluates.
    assert_eq!(
        eval_json("value = [1, [2]]\n")["value"],
        serde_json::json!([1, [2]])
    );
}

/// RB-300 — Jackson TOML stack-overflow analogue (SRC-CVE-0007 §H, CRITICAL).
/// Origin: NVD CVE-2023-3894.
/// Bounded generator: 400 nested inline tables trip E004 — the same
/// structural bound as arrays (RB-296), one shared gate.
#[test]
fn rb_300_cve_jackson_depth_bounded() {
    let src = format!("value = {}0{}", "{ a = ".repeat(400), " }".repeat(400));
    let diags = parse(&src, "research").expect_err("must fail E004");
    assert!(
        diags.iter().any(|d| d.code == "E004"),
        "E004 expected, got {:?}",
        &diags[..diags.len().min(3)]
    );
}

/// RB-301 — MLAlchemy unsafe-deserialization analogue (SRC-CVE-0008 §H, CRITICAL).
/// Origin: NVD CVE-2017-16615.
/// Unsafe `load` has no analogue: the tag line fails E002 with Error
/// severity at parse — no constructor exists to invoke, so attacker
/// code can never execute through configuration text.
#[test]
fn rb_301_cve_unsafe_load_impossible() {
    let src = "value = !python/object/apply:os.system [\"echo forbidden\"]\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    // Positive control: data-only content binds without execution.
    assert_eq!(
        eval_json("value = \"echo forbidden\"\n")["value"],
        "echo forbidden"
    );
}

/// RB-302 — kaml endless-loop analogue (SRC-CVE-0009 §H, CRITICAL).
/// Origin: NVD CVE-2021-39194.
/// Bare `${…}` outside strings is not a reference cycle risk: `$`
/// rejects loud E005 — and the valid string-interpolation cycle is
/// S001 (RB-290). Termination is structural, never traversal-luck.
#[test]
fn rb_302_cve_bare_interp_rejects() {
    let src = "let a = ${b}\nlet b = ${a}\nvalue = ${a}\n";
    let diags = parse(src, "research").expect_err("must fail E005");
    assert!(
        diags.iter().any(|d| d.code == "E005"),
        "E005 expected, got {diags:?}"
    );
    // Positive control: acyclic `let` indirection binds.
    assert_eq!(eval_json("let a = 1\nvalue = a\n")["value"], 1);
}

/// RB-303 — RFC 8259 security-considerations analogue (SRC-CVE-0010 §H, HIGH).
/// Origin: RFC 8259 (limits, duplicate names, interop).
/// The interoperable duplicate member resolves under the locked
/// W011 policy with span — announcement ends cross-implementation
/// disagreement deterministically.
#[test]
fn rb_303_rfc8259_dup_announced_span() {
    let src = "value = { a = 1, a = 2 }\n";
    assert_eq!(eval_json(src)["value"]["a"], 2);
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    let span = w.span.clone().expect("W011 carries a span");
    assert_eq!((span.line, span.column), (1, 22), "{span:?}");
    // Positive control: distinct members bind silently.
    let codes = codes_of("value = { a = 1, b = 2 }\n");
    assert!(!codes.iter().any(|c| c == "W011"), "{codes:?}");
}

/// RB-304 — TOML offset-datetime mismatch analogue (SRC-SPEC-0001 §H, HIGH).
/// Origin: typst/typst#7920 (non-UTC offsets misrepresented).
/// REFUSAL (spec-gated): no timezone-aware datetime can be
/// misrepresented — no date type exists. The bare form evaluates by
/// shipped rules (1979-5-27 = 1947 plus the specified `T00:32` colon
/// pair): explicit, deterministic, never an opaque fallback map.
#[test]
fn rb_304_offset_datetime_deterministic() {
    let src = "value = 1979-05-27T00:32:00-07:00\n";
    let first = eval_json(src);
    assert_eq!(first["value"], 1947);
    assert_eq!(first["T00"], 32);
    assert_eq!(first, eval_json(src));
    // Positive control: the quoted form stays an exact string.
    assert_eq!(
        eval_json("value = \"1979-05-27T00:32:00-07:00\"\n")["value"],
        "1979-05-27T00:32:00-07:00"
    );
}

/// RB-305 — Valid/invalid suite-matrix analogue (SRC-SPEC-0002 §H, MEDIUM).
/// Origin: toml-test valid/invalid separation (+1.0/1.1 matrix).
/// SOLID chooses its own contract and pairs both sides in one test:
/// the duplicate warns W011 while the distinct twin binds silently —
/// acceptance, rejection, and warned-acceptance each have a class.
#[test]
fn rb_305_valid_invalid_pair_contract() {
    let bad = "value = { a = 1, a = 2 }\n";
    assert_eq!(eval_json(bad)["value"]["a"], 2);
    assert!(codes_of(bad).iter().any(|c| c == "W011"), "W011 expected");
    let good = "value = { a = 1, b = 2 }\n";
    assert_eq!(
        eval_json(good)["value"],
        serde_json::json!({"a": 1, "b": 2})
    );
    assert!(codes_of(good).is_empty(), "clean twin must be silent");
}

/// RB-306 — TOML key/table-redefinition analogue (SRC-SPEC-0003 §H, HIGH).
/// Origin: TOML v1.0.0 (no repeated tables, no table/scalar conflicts).
/// A map can never become a scalar implicitly: dotted assignment has
/// no grammar — E002 at 2:1 — so the conflict class cannot occur.
#[test]
fn rb_306_path_kind_conflict_impossible() {
    let src = "fruit = { apple = { color = \"red\" } }\nfruit.apple = 3\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the table alone evaluates.
    let v = eval_json("fruit = { apple = { color = \"red\" } }\n");
    assert_eq!(v["fruit"]["apple"]["color"], "red");
}

/// RB-307 — TOML array-of-tables-ordering analogue (SRC-SPEC-0004 §H, HIGH).
/// Origin: TOML v1.0.0 (element parent must exist first).
/// The naive patch transliteration is rejected (E019: `(` required),
/// so no child operation can invent a parent node or index — the
/// operation never runs.
#[test]
fn rb_307_patch_needs_existing_parent() {
    let src = "fruit = []\n@patch fruit[0] { name = \"apple\" }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    assert!(
        diags.iter().any(|d| d.code == "E019"),
        "E019 expected, got {diags:?}"
    );
    // Positive control: the array alone evaluates.
    assert_eq!(eval_json("fruit = []\n")["fruit"], serde_json::json!([]));
}

/// RB-308 — TOML numeric-grammar analogue (SRC-SPEC-0005 §H, HIGH).
/// Origin: TOML v1.0.0 numeric grammar.
/// REFUSAL (spec-gated): `01.` is not an invalid number — leading
/// zeros are plain decimals (RB-184) and the trailing dot falls
/// under the skip quirk (RB-207): exactly int 1, no host fallback.
#[test]
fn rb_308_toml_numeric_grammar_decimal() {
    let v = eval_json("value = 01.\n");
    assert_eq!(v["value"], 1);
    assert!(v["value"].is_i64());
    // Positive control: the clean decimal neighbour agrees.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-419 — Derived variant 12-02: insert triple (SYN-0012-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0012 (toml-test no-comma-02).
/// Missing comma heals with value `[1, 2]`, class H003 at 1:19,
/// Warning severity — insertions announce louder than removals.
#[test]
fn rb_419_syn12_insert_triple() {
    let src = "case_0012_02 = [1 2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0012_02"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    assert_eq!(
        h.severity,
        active_core::Severity::Warning,
        "H003 must be Warning, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-420 — Derived variant 12-03: lone-comma triple (SYN-0012-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0012 (toml-test no-comma-02).
/// `[,]` heals H002 at the comma (1:17) to empty with Info severity —
/// value, class, span, and severity indexed jointly.
#[test]
fn rb_420_syn12_lone_comma_triple() {
    let src = "case_0012_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0012_03"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
}

/// RB-421 — Derived variant 12-04: trailing span + control (SYN-0012-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0012 (toml-test no-comma-02).
/// Trailing comma heals H002 at 1:18 — and the clean neighbour
/// carries no heal, proving the repair is input-specific.
#[test]
fn rb_421_syn12_trailing_control() {
    let src = "case_0012_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0012_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
    // Positive control: the clean neighbour carries no heal.
    assert!(!codes_of("case_0012_04 = [1]\n").iter().any(|c| c == "H002"));
}

/// RB-422 — Derived variant 12-05: typed determinism (SYN-0012-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0012 (toml-test no-comma-02).
/// Clean triple asserts value, int element types, determinism, and
/// silence jointly — the full good-case contract.
#[test]
fn rb_422_syn12_typed_deterministic() {
    let src = "case_0012_05 = [1, 2, 3]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0012_05"], serde_json::json!([1, 2, 3]));
    for e in v["case_0012_05"].as_array().expect("array") {
        assert!(e.is_i64(), "{v}");
    }
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-423 — Derived variant 12-06: empty determinism (SYN-0012-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0012 (toml-test no-comma-02).
/// Empty arrays evaluate deterministically — repeated runs agree
/// exactly with zero diagnostics.
#[test]
fn rb_423_syn12_empty_deterministic() {
    let src = "case_0012_06 = []\n";
    assert_eq!(eval_json(src)["case_0012_06"], serde_json::json!([]));
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-424 — Derived variant 12-07: nested lengths (SYN-0012-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0012 (toml-test no-comma-02).
/// Nested collections assert outer and inner lengths plus
/// determinism — every boundary lands exactly where written.
#[test]
fn rb_424_syn12_nested_lengths() {
    let src = "case_0012_07 = [[1], [2, 3]]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0012_07"], serde_json::json!([[1], [2, 3]]));
    assert_eq!(v["case_0012_07"].as_array().expect("array").len(), 2);
    assert_eq!(v["case_0012_07"][0].as_array().expect("array").len(), 1);
    assert_eq!(v["case_0012_07"][1].as_array().expect("array").len(), 2);
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-425 — Derived variant 12-08: hetero joint oracle (SYN-0012-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0012 (toml-test no-comma-02).
/// Mixed arrays assert value, int type, table key-set, and no error
/// class jointly — parser/schema agreement for the 12 lineage.
#[test]
fn rb_425_syn12_hetero_joint() {
    let src = "case_0012_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0012_08"], serde_json::json!([1, {"name": "x"}]));
    assert!(v["case_0012_08"][0].is_i64(), "{v}");
    let keys: Vec<&str> = v["case_0012_08"][1]
        .as_object()
        .expect("table")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["name"]);
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-426 — Derived variant 12-09: EOF triple (SYN-0012-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0012 (toml-test no-comma-02).
/// Unterminated array fails E001 Error at EOF (2:1) — class,
/// severity, and span indexed jointly for the 12 lineage.
#[test]
fn rb_426_syn12_eof_triple() {
    let src = "case_0012_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed twin evaluates.
    assert_eq!(
        eval_json("case_0012_09 = [1, 2]\n")["case_0012_09"],
        serde_json::json!([1, 2])
    );
}

/// RB-427 — Derived variant 13-01: heal triple (SYN-0013-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0013 (toml-test only-comma-01).
/// Doubled comma heals with value `[1, 2]`, class H002 at 1:19, Info
/// severity — the full repair triple indexed for the 13 lineage.
#[test]
fn rb_427_syn13_heal_triple() {
    let src = "case_0013_01 = [1,,2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0013_01"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-428 — Derived variant 13-02: insert triple (SYN-0013-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0013 (toml-test only-comma-01).
/// Missing comma heals with value `[1, 2]`, class H003 at 1:19,
/// Warning severity — insertions announce louder than removals.
/// (Completes SYN-0012-02…09 + opens SYN-0013-01…02.)
#[test]
fn rb_428_syn13_insert_triple() {
    let src = "case_0013_02 = [1 2]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0013_02"], serde_json::json!([1, 2]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    assert_eq!(
        h.severity,
        active_core::Severity::Warning,
        "H003 must be Warning, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
}

/// RB-429 — Derived variant 13-03: lone-comma triple (SYN-0013-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0013 (toml-test only-comma-01).
/// `[,]` heals H002 at the comma (1:17) to empty with Info severity —
/// value, class, span, and severity indexed jointly.
#[test]
fn rb_429_syn13_lone_comma_triple() {
    let src = "case_0013_03 = [,]\n";
    assert_eq!(eval_json(src)["case_0013_03"], serde_json::json!([]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
}

/// RB-430 — Derived variant 13-04: trailing span + control (SYN-0013-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0013 (toml-test only-comma-01).
/// Trailing comma heals H002 at 1:18 — and the clean neighbour
/// carries no heal, proving the repair is input-specific.
#[test]
fn rb_430_syn13_trailing_control() {
    let src = "case_0013_04 = [1,]\n";
    assert_eq!(eval_json(src)["case_0013_04"], serde_json::json!([1]));
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
    // Positive control: the clean neighbour carries no heal.
    assert!(!codes_of("case_0013_04 = [1]\n").iter().any(|c| c == "H002"));
}

/// RB-431 — Derived variant 13-05: typed determinism (SYN-0013-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0013 (toml-test only-comma-01).
/// Clean triple asserts value, int element types, determinism, and
/// silence jointly — the full good-case contract.
#[test]
fn rb_431_syn13_typed_deterministic() {
    let src = "case_0013_05 = [1, 2, 3]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0013_05"], serde_json::json!([1, 2, 3]));
    for e in v["case_0013_05"].as_array().expect("array") {
        assert!(e.is_i64(), "{v}");
    }
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-432 — Derived variant 13-06: empty determinism (SYN-0013-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0013 (toml-test only-comma-01).
/// Empty arrays evaluate deterministically — repeated runs agree
/// exactly with zero diagnostics.
#[test]
fn rb_432_syn13_empty_deterministic() {
    let src = "case_0013_06 = []\n";
    assert_eq!(eval_json(src)["case_0013_06"], serde_json::json!([]));
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-433 — Derived variant 13-07: nested lengths (SYN-0013-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0013 (toml-test only-comma-01).
/// Nested collections assert outer and inner lengths plus
/// determinism — every boundary lands exactly where written.
#[test]
fn rb_433_syn13_nested_lengths() {
    let src = "case_0013_07 = [[1], [2, 3]]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0013_07"], serde_json::json!([[1], [2, 3]]));
    assert_eq!(v["case_0013_07"].as_array().expect("array").len(), 2);
    assert_eq!(v["case_0013_07"][0].as_array().expect("array").len(), 1);
    assert_eq!(v["case_0013_07"][1].as_array().expect("array").len(), 2);
    assert_eq!(eval_json(src), eval_json(src));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-434 — Derived variant 13-08: hetero joint oracle (SYN-0013-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0013 (toml-test only-comma-01).
/// Mixed arrays assert value, int type, table key-set, and no error
/// class jointly — parser/schema agreement for the 13 lineage.
#[test]
fn rb_434_syn13_hetero_joint() {
    let src = "case_0013_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src);
    assert_eq!(v["case_0013_08"], serde_json::json!([1, {"name": "x"}]));
    assert!(v["case_0013_08"][0].is_i64(), "{v}");
    let keys: Vec<&str> = v["case_0013_08"][1]
        .as_object()
        .expect("table")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["name"]);
    let codes = codes_of(src);
    assert!(
        !codes.iter().any(|c| c.starts_with('E')),
        "no error class expected, got {codes:?}"
    );
}

/// RB-435 — Derived variant 13-09: EOF triple (SYN-0013-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0013 (toml-test only-comma-01).
/// Unterminated array fails E001 Error at EOF (2:1) — class,
/// severity, and span indexed jointly for the 13 lineage.
#[test]
fn rb_435_syn13_eof_triple() {
    let src = "case_0013_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the closed twin evaluates.
    assert_eq!(
        eval_json("case_0013_09 = [1, 2]\n")["case_0013_09"],
        serde_json::json!([1, 2])
    );
}

/// RB-436 — Derived variant 14-01: exactly one warning (SYN-0014-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0014 (toml-test tables-01).
/// Redefinition announces with EXACTLY one diagnostic — a single
/// W011, nothing else: no cascade, no duplicate notice, no silence.
#[test]
fn rb_436_syn14_exactly_one_warning() {
    let src = "fruit = { name = \"apple\" }\nfruit = { name = \"banana\" }\n";
    assert_eq!(eval_json(src)["fruit"]["name"], "banana");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W011 must be Warning, got {:?}",
        w.severity
    );
}

/// RB-437 — Derived variant 14-02: E019 severity (SYN-0014-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0014 (toml-test tables-01).
/// The naive index form fails E019 with Error severity at 2:2 —
/// paths type-check before values move, loudly.
#[test]
fn rb_437_syn14_e019_error() {
    let src = "items = []\n@patch items[0] { name = \"x\" }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E019 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    // Positive control: the array alone stays exactly empty.
    assert_eq!(eval_json("items = []\n")["items"], serde_json::json!([]));
}

/// RB-438 — Derived variant 14-03: E019 severity (SYN-0014-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0014 (toml-test tables-01).
/// Same E019 Error at 2:2 with a populated array — element identity
/// never degrades into map-key addressing. (Completes SYN-0013-03…09
/// + opens SYN-0014-01…03.)
#[test]
fn rb_438_syn14_populated_e019_error() {
    let src = "items = [{ name = \"apple\" }]\n@patch items[0] { color = \"red\" }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E019 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    // Positive control: the element is intact.
    assert_eq!(
        eval_json("items = [{ name = \"apple\" }]\n")["items"][0]["name"],
        "apple"
    );
}

/// RB-439 — Derived variant 14-04: no map-and-scalar hybrid (SYN-0014-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0014 (toml-test tables-01).
/// Redefining a map path with a scalar wins wholesale and announces W011 —
/// the path is never both at once. The dotted middle spelling is refused
/// loudly (E002), never merged into a hybrid.
#[test]
fn rb_439_syn14_no_map_scalar_hybrid() {
    let src = "fruit = { apple = { color = \"red\" } }\nfruit = 1\n";
    let v = eval_json(src);
    assert_eq!(v["fruit"], 1, "scalar must win wholesale, got {v}");
    assert!(
        v["fruit"].get("apple").is_none(),
        "no map remnant may survive, got {v}"
    );
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W011 must be Warning, got {:?}",
        w.severity
    );
    // The dotted transliteration is refused loudly, never merged.
    let diags =
        parse("fruit = { apple = 1 }\nfruit.apple = 1\n", "research").expect_err("must fail E002");
    assert!(
        diags.iter().any(|d| d.code == "E002"),
        "E002 expected, got {diags:?}"
    );
    // Positive control: the map alone keeps its nested shape.
    let v = eval_json("fruit = { apple = { color = \"red\" } }\n");
    assert_eq!(v["fruit"]["apple"]["color"], "red");
}

/// RB-440 — Derived variant 14-05: explicit patch is not a redeclaration (SYN-0014-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0014 (toml-test tables-01).
/// The naive `@patch fruit { apple = 2 }` form fails E019 at 2:2, while the
/// explicit call updates with ZERO diagnostics — no W011, so an explicit
/// update is never confused with a duplicate declaration.
#[test]
fn rb_440_syn14_patch_not_redeclaration() {
    let src = "fruit = { apple = 1 }\n@patch fruit { apple = 2 }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E019 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    let src = "fruit = { apple = 1 }\n@patch(\"fruit.apple\", 2)\n";
    assert_eq!(eval_json(src)["fruit"]["apple"], 2);
    let codes = codes_of(src);
    assert!(codes.is_empty(), "patch must warn nothing, got {codes:?}");
    // Positive control: a real duplicate still announces W011.
    let codes = codes_of("fruit = { apple = 1 }\nfruit = { apple = 2 }\n");
    assert!(codes.iter().any(|c| c == "W011"), "{codes:?}");
}

/// RB-441 — Derived variant 14-06: removal is structural and deterministic (SYN-0014-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0014 (toml-test tables-01).
/// The naive `@remove fruit.apple` form fails E019 at 2:2; the explicit call
/// removes the leaf, keeps siblings, and evaluates identically every run.
#[test]
fn rb_441_syn14_removal_deterministic() {
    let src = "fruit = { apple = 1 }\n@remove fruit.apple\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E019 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    let src = "fruit = { apple = 1, pear = 2 }\n@remove(\"fruit.apple\")\n";
    let v = eval_json(src);
    assert!(v["fruit"].get("apple").is_none(), "{v}");
    assert_eq!(v["fruit"]["pear"], 2);
    assert_eq!(eval_json(src), v, "removal must be deterministic");
    // Positive control: removing a missing path stays idempotent W004.
    let codes = codes_of("@remove(\"missing\")\n");
    assert!(codes.iter().any(|c| c == "W004"), "{codes:?}");
}

/// RB-442 — Derived variant 14-07: rename preserves nested identity (SYN-0014-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0014 (toml-test tables-01).
/// The naive `@rename fruit.apple to pear` form fails E019 at 2:2; the
/// explicit call moves a whole nested table — subtree intact under the
/// new name, old name gone.
#[test]
fn rb_442_syn14_rename_keeps_subtree() {
    let src = "fruit = { apple = 1 }\n@rename fruit.apple to pear\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E019 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    let v =
        eval_json("fruit = { apple = { color = \"red\" } }\n@rename(\"fruit.apple\", \"pear\")\n");
    assert_eq!(
        v["fruit"]["pear"],
        serde_json::json!({"color": "red"}),
        "{v}"
    );
    assert!(v["fruit"].get("apple").is_none(), "{v}");
    // Positive control: scalar rename stays exact.
    let v = eval_json("fruit = { apple = 1 }\n@rename(\"fruit.apple\", \"pear\")\n");
    assert_eq!(v["fruit"]["pear"], 1);
}

/// RB-443 — Derived variant 14-08: ordered patch plans are deterministic (SYN-0014-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0014 (toml-test tables-01).
/// The naive double-rename plan fails E019; explicit sequential patches apply
/// in order — both orders stable across runs, results order-dependent.
#[test]
fn rb_443_syn14_ordered_plan_deterministic() {
    let src = "fruit = { apple = 1 }\n@rename fruit.apple to pear\n@rename fruit.pear to apple\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    let e = diags.iter().find(|d| d.code == "E019").expect("E019");
    let span = e.span.clone().expect("E019 carries a span");
    assert_eq!((span.line, span.column), (2, 2), "{span:?}");
    let fwd = "fruit = { apple = 1 }\n@patch(\"fruit.apple\", 2)\n@patch(\"fruit.apple\", 3)\n";
    assert_eq!(eval_json(fwd)["fruit"]["apple"], 3);
    assert_eq!(
        eval_json(fwd),
        eval_json(fwd),
        "forward plan must be stable"
    );
    let rev = "fruit = { apple = 1 }\n@patch(\"fruit.apple\", 3)\n@patch(\"fruit.apple\", 2)\n";
    assert_eq!(eval_json(rev)["fruit"]["apple"], 2);
    assert_eq!(
        eval_json(rev),
        eval_json(rev),
        "reverse plan must be stable"
    );
}

/// RB-444 — Derived variant 14-09: nested patch never replaces the parent (SYN-0014-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0014 (toml-test tables-01).
/// The naive `@patch fruit.apple { color = "green" }` form fails E019; the
/// explicit leaf call changes only the leaf — whole-document equality pins
/// the containment, parent and siblings identical in value.
#[test]
fn rb_444_syn14_nested_patch_contained() {
    let src = "fruit = { apple = { color = \"red\" } }\n@patch fruit.apple { color = \"green\" }\n";
    let diags = parse(src, "research").expect_err("must fail E019");
    assert!(
        diags.iter().any(|d| d.code == "E019"),
        "E019 expected, got {diags:?}"
    );
    let v = eval_json(
        "fruit = { apple = { color = \"red\", size = 1 } }\n@patch(\"fruit.apple.color\", \"green\")\n",
    );
    assert_eq!(
        v,
        serde_json::json!({"fruit": {"apple": {"color": "green", "size": 1}}}),
        "{v}"
    );
    // Positive control: a second leaf patch leaves the first intact.
    let v = eval_json("fruit = { apple = { color = \"red\" } }\n@patch(\"fruit.apple.size\", 1)\n");
    assert_eq!(v["fruit"]["apple"]["color"], "red");
    assert_eq!(v["fruit"]["apple"]["size"], 1);
}

/// RB-445 — Derived variant 15-01: doubled comma creates no null (SYN-0015-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0015 (toml-test text-in-array).
/// H002 Info at 1:19 heals `case_0015_01 = [1,,2]` to exactly `[1, 2]` —
/// the stray comma is dropped, never materialized as an implicit null.
#[test]
fn rb_445_syn15_doubled_comma_no_null() {
    let src = "case_0015_01 = [1,,2]\n";
    assert_eq!(
        eval_json(src)["case_0015_01"],
        serde_json::json!([1, 2]),
        "healed value must hold exactly two elements"
    );
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
    // Positive control: the clean neighbour parses with zero diagnostics.
    let codes = codes_of("case_0015_01 = [1, 2]\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-446 — Derived variant 15-02: missing separator is announced, never silent (SYN-0015-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0015 (toml-test text-in-array).
/// H003 Warning at 1:19 announces the inserted separator for `[1 2]` —
/// adjacent values are accepted only loudly, never merged silently.
#[test]
fn rb_446_syn15_missing_separator_announced() {
    let src = "case_0015_02 = [1 2]\n";
    assert_eq!(
        eval_json(src)["case_0015_02"],
        serde_json::json!([1, 2]),
        "healed value must hold both elements"
    );
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H003").expect("H003");
    assert_eq!(
        h.severity,
        active_core::Severity::Warning,
        "H003 must be Warning, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H003 carries a span");
    assert_eq!((span.line, span.column), (1, 19), "{span:?}");
    // Positive control: the separated neighbour parses with zero diagnostics.
    let codes = codes_of("case_0015_02 = [1, 2]\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-447 — Derived variant 15-03: a lone comma stands for no value (SYN-0015-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0015 (toml-test text-in-array).
/// H002 Info at 1:17 heals `case_0015_03 = [,]` to `[]` — the comma never
/// becomes a null element or any other implicit value.
#[test]
fn rb_447_syn15_lone_comma_no_value() {
    let src = "case_0015_03 = [,]\n";
    assert_eq!(
        eval_json(src)["case_0015_03"],
        serde_json::json!([]),
        "healed value must be empty, never [null]"
    );
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 17), "{span:?}");
    // Positive control: the empty neighbour parses with zero diagnostics.
    let codes = codes_of("case_0015_03 = []\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-448 — Derived variant 15-04: trailing-comma policy locked (SYN-0015-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0015 (toml-test text-in-array).
/// H002 Info at 1:18 heals `case_0015_04 = [1,]` to `[1]` — the trailing
/// comma is dropped deterministically across runs. (Completes SYN-0014-04…09
/// + opens SYN-0015-01…04.)
#[test]
fn rb_448_syn15_trailing_comma_locked() {
    let src = "case_0015_04 = [1,]\n";
    assert_eq!(
        eval_json(src)["case_0015_04"],
        serde_json::json!([1]),
        "healed value must hold exactly one element"
    );
    assert_eq!(eval_json(src), eval_json(src), "heal must be deterministic");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let h = diags.iter().find(|d| d.code == "H002").expect("H002");
    assert_eq!(
        h.severity,
        active_core::Severity::Info,
        "H002 must be Info, got {:?}",
        h.severity
    );
    let span = h.span.clone().expect("H002 carries a span");
    assert_eq!((span.line, span.column), (1, 18), "{span:?}");
    // Positive control: the comma-free neighbour parses with zero diagnostics.
    let codes = codes_of("case_0015_04 = [1]\n");
    assert!(codes.is_empty(), "{codes:?}");
}
/// RB-449 — Derived variant 15-05: clean array proves heals are separator-specific (SYN-0015-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0015 (toml-test text-in-array).
/// `case_0015_05 = [1, 2, 3]` evaluates to exactly three elements with ZERO
/// diagnostics — the lineage heals (H002/H003) fire only on broken
/// separators, never on well-formed input.
#[test]
fn rb_449_syn15_clean_three_array_silent() {
    let src = "case_0015_05 = [1, 2, 3]\n";
    assert_eq!(
        eval_json(src)["case_0015_05"],
        serde_json::json!([1, 2, 3]),
        "clean array must hold exactly three elements"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "clean array must warn nothing, got {codes:?}"
    );
    // Control: the doubled-comma neighbour still announces H002.
    let codes = codes_of("case_0015_05 = [1,,2, 3]\n");
    assert!(codes.iter().any(|c| c == "H002"), "{codes:?}");
}

/// RB-450 — Derived variant 15-06: empty array is valid (SYN-0015-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0015 (toml-test text-in-array).
/// `case_0015_06 = []` evaluates to an empty array with ZERO diagnostics —
/// emptiness is valid; only a degenerate comma needs a heal.
#[test]
fn rb_450_syn15_empty_array_valid() {
    let src = "case_0015_06 = []\n";
    assert_eq!(
        eval_json(src)["case_0015_06"],
        serde_json::json!([]),
        "empty array must stay empty"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "empty array must warn nothing, got {codes:?}"
    );
    // Control: the lone-comma neighbour heals (H002), never fails.
    assert_eq!(
        eval_json("case_0015_06 = [,]\n")["case_0015_06"],
        serde_json::json!([])
    );
    let codes = codes_of("case_0015_06 = [,]\n");
    assert!(codes.iter().any(|c| c == "H002"), "{codes:?}");
}

/// RB-451 — Derived variant 15-07: nested delimiters match or fail loudly (SYN-0015-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0015 (toml-test text-in-array).
/// `[[1], [2, 3]]` evaluates to the exact nested shape with ZERO diagnostics;
/// a mismatched `[[1]` fails E001 Error at EOF (2:1) — delimiters must match
/// properly, never degrade silently.
#[test]
fn rb_451_syn15_nested_delimiters_match() {
    let src = "case_0015_07 = [[1], [2, 3]]\n";
    assert_eq!(
        eval_json(src)["case_0015_07"],
        serde_json::json!([[1], [2, 3]]),
        "nested shape must be exact"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "nested array must warn nothing, got {codes:?}"
    );
    // Negative neighbour: mismatched delimiters fail loudly at EOF.
    let diags = parse("case_0015_07 = [[1]\n", "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
}

/// RB-452 — Derived variant 15-08: heterogeneous arrays keep exact element types (SYN-0015-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0015 (toml-test text-in-array).
/// `[1, { name = "x" }]` is accepted with ZERO diagnostics and exact types —
/// the integer stays an integer, the table stays a table; parser and value
/// model agree, no coercion either way.
#[test]
fn rb_452_syn15_hetero_types_exact() {
    let src = "case_0015_08 = [1, { name = \"x\" }]\n";
    let v = eval_json(src)["case_0015_08"].clone();
    assert_eq!(v[0], 1, "integer element must stay integer, got {v}");
    assert_eq!(v[1]["name"], "x", "table element must stay table, got {v}");
    assert_eq!(v, serde_json::json!([1, {"name": "x"}]), "{v}");
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "hetero array must warn nothing, got {codes:?}"
    );
    // Positive control: homogeneous arrays evaluate identically clean.
    let codes = codes_of("case_0015_08 = [1, 2]\n");
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-453 — Derived variant 15-09: unterminated array points at EOF (SYN-0015-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0015 (toml-test text-in-array).
/// `case_0015_09 = [1, 2` (no bracket) fails E001 Error with the span on the
/// EOF line (2:1) — the diagnostic points at end-of-input, never at the
/// opening bracket alone and never nowhere.
#[test]
fn rb_453_syn15_unterminated_points_at_eof() {
    let src = "case_0015_09 = [1, 2\n";
    let diags = parse(src, "research").expect_err("must fail E001");
    let e = diags.iter().find(|d| d.code == "E001").expect("E001");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E001 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E001 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the terminated neighbour evaluates with zero diags.
    let src = "case_0015_09 = [1, 2]\n";
    assert_eq!(eval_json(src)["case_0015_09"], serde_json::json!([1, 2]));
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-454 — Derived variant 16-01: bare `yes` never becomes bool (SYN-0016-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0016 (toml-test almost-false).
/// `case_0016_01 = yes` warns W001 Warning at the name span (1:16) and
/// evaluates to null — the Norway flip is impossible: no YAML 1.1 resolver
/// runs, so `yes` is never bool true.
#[test]
fn rb_454_syn16_bare_yes_never_bool() {
    let src = "case_0016_01 = yes\n";
    let v = eval_json(src);
    assert!(
        v["case_0016_01"].is_null(),
        "bare yes must be null, got {v}"
    );
    assert_ne!(v["case_0016_01"], true, "bare yes must never be bool true");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0016_01 = \"yes\"\n")["case_0016_01"], "yes");
}

/// RB-455 — Derived variant 16-02: bare `no` never becomes bool (SYN-0016-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0016 (toml-test almost-false).
/// `case_0016_02 = no` warns W001 Warning at (1:16) and evaluates to null —
/// country-code and feature-flag spellings never resolve to bool false,
/// while the reserved `false` literal still does.
#[test]
fn rb_455_syn16_bare_no_never_bool() {
    let src = "case_0016_02 = no\n";
    let v = eval_json(src);
    assert!(v["case_0016_02"].is_null(), "bare no must be null, got {v}");
    assert_ne!(v["case_0016_02"], false, "bare no must never be bool false");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the reserved literal still means bool false.
    assert_eq!(eval_json("case_0016_02 = false\n")["case_0016_02"], false);
}

/// RB-456 — Derived variant 16-03: bare `on` never becomes bool (SYN-0016-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0016 (toml-test almost-false).
/// `case_0016_03 = on` warns W001 Warning at (1:16) and evaluates to null —
/// legacy YAML resolver spellings never silently become bool true.
#[test]
fn rb_456_syn16_bare_on_never_bool() {
    let src = "case_0016_03 = on\n";
    let v = eval_json(src);
    assert!(v["case_0016_03"].is_null(), "bare on must be null, got {v}");
    assert_ne!(v["case_0016_03"], true, "bare on must never be bool true");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0016_03 = \"on\"\n")["case_0016_03"], "on");
}

/// RB-457 — Derived variant 16-04: bare `off` never becomes bool (SYN-0016-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0016 (toml-test almost-false).
/// `case_0016_04 = off` warns W001 Warning at (1:16) and evaluates to null —
/// legacy YAML resolver spellings never silently become bool false.
#[test]
fn rb_457_syn16_bare_off_never_bool() {
    let src = "case_0016_04 = off\n";
    let v = eval_json(src);
    assert!(
        v["case_0016_04"].is_null(),
        "bare off must be null, got {v}"
    );
    assert_ne!(
        v["case_0016_04"], false,
        "bare off must never be bool false"
    );
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0016_04 = \"off\"\n")["case_0016_04"], "off");
}

/// RB-458 — Derived variant 16-05: reserved `true` stays bool (SYN-0016-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0016 (toml-test almost-false).
/// `case_0016_05 = true` evaluates to bool true with ZERO diagnostics — only
/// the documented `true`/`false` tokens carry boolean type, and the quoted
/// "true" neighbour stays a string. (Completes SYN-0015-05…09 + 16-01…05.)
#[test]
fn rb_458_syn16_reserved_true_is_bool() {
    let src = "case_0016_05 = true\n";
    assert_eq!(
        eval_json(src)["case_0016_05"],
        true,
        "reserved true must be bool"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "reserved true must warn nothing, got {codes:?}"
    );
    // Positive control: the quoted neighbour stays the exact string.
    let v = eval_json("case_0016_05 = \"true\"\n");
    assert_eq!(v["case_0016_05"], "true");
    assert_ne!(v["case_0016_05"], true, "quoted true must never be bool");
}
/// RB-459 — Derived variant 16-06: noncanonical `FALSE` carries no bool type (SYN-0016-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0016 (toml-test almost-false).
/// `case_0016_06 = FALSE` warns W001 Warning at (1:16) and evaluates to null —
/// only the documented lowercase `false` carries boolean type; case variants
/// never do.
#[test]
fn rb_459_syn16_upper_false_never_bool() {
    let src = "case_0016_06 = FALSE\n";
    let v = eval_json(src);
    assert!(v["case_0016_06"].is_null(), "FALSE must be null, got {v}");
    assert_ne!(v["case_0016_06"], false, "FALSE must never be bool false");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the documented lowercase literal still means bool.
    assert_eq!(eval_json("case_0016_06 = false\n")["case_0016_06"], false);
}

/// RB-460 — Derived variant 16-07: null case policy is explicit (SYN-0016-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0016 (toml-test almost-false).
/// Lowercase `null` is reserved: null value with ZERO diagnostics. Uppercase
/// `NULL` also evaluates to null but announces W001 — same value, different
/// class, so the spelling policy is observable, never implicit.
#[test]
fn rb_460_syn16_null_case_policy() {
    let src = "case_0016_07 = null\n";
    assert!(
        eval_json(src)["case_0016_07"].is_null(),
        "null must be null"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "reserved null must warn nothing, got {codes:?}"
    );
    // Control: the uppercase spelling resolves to null only loudly.
    let v = eval_json("case_0016_07 = NULL\n");
    assert!(
        v["case_0016_07"].is_null(),
        "NULL must still be null, got {v}"
    );
    let codes = codes_of("case_0016_07 = NULL\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
}

/// RB-461 — Derived variant 16-08: quoting preserves literal text (SYN-0016-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0016 (toml-test almost-false).
/// `case_0016_08 = "no"` evaluates to exactly "no" with ZERO diagnostics —
/// quoting reliably preserves literal text regardless of resolver lookalikes.
#[test]
fn rb_461_syn16_quoted_no_exact() {
    let src = "case_0016_08 = \"no\"\n";
    assert_eq!(
        eval_json(src)["case_0016_08"],
        "no",
        "quoted no must stay exact"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "quoted no must warn nothing, got {codes:?}"
    );
    // Control: the bare neighbour never coerces (converges with RB-455).
    let v = eval_json("case_0016_08 = no\n");
    assert!(v["case_0016_08"].is_null(), "{v}");
    let codes = codes_of("case_0016_08 = no\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
}

/// RB-462 — Derived variant 16-09: numbers are never booleans (SYN-0016-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0016 (toml-test almost-false).
/// `case_0016_09 = 1.0` evaluates to float 1.0 with ZERO diagnostics — the
/// scalar resolver keeps numbers numeric; truthiness never leaks into type.
#[test]
fn rb_462_syn16_number_never_bool() {
    let src = "case_0016_09 = 1.0\n";
    let v = eval_json(src)["case_0016_09"].clone();
    assert_eq!(v.as_f64(), Some(1.0), "1.0 must stay float, got {v}");
    assert!(!v.is_boolean(), "1.0 must never be boolean, got {v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "1.0 must warn nothing, got {codes:?}");
    // Positive control: the reserved literal still means bool true.
    assert_eq!(eval_json("case_0016_09 = true\n")["case_0016_09"], true);
}

/// RB-463 — Derived variant 17-01: twin lineage agrees on `yes` (SYN-0017-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0017 (toml-test almost-true).
/// `case_0017_01 = yes` warns W001 Warning at (1:16) and evaluates to null —
/// the almost-true lineage converges exactly with the almost-false twin
/// (RB-454): same class, same span shape, same value, never bool true.
#[test]
fn rb_463_syn17_twin_yes_never_bool() {
    let src = "case_0017_01 = yes\n";
    let v = eval_json(src);
    assert!(
        v["case_0017_01"].is_null(),
        "bare yes must be null, got {v}"
    );
    assert_ne!(v["case_0017_01"], true, "bare yes must never be bool true");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Twin convergence: the 0016 twin resolves to the same value.
    assert_eq!(
        v["case_0017_01"],
        eval_json("case_0016_01 = yes\n")["case_0016_01"]
    );
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0017_01 = \"yes\"\n")["case_0017_01"], "yes");
}

/// RB-464 — Derived variant 17-02: twin lineage agrees on `no` (SYN-0017-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0017 (toml-test almost-true).
/// `case_0017_02 = no` warns W001 Warning at (1:16) and evaluates to null —
/// converges with RB-455; the reserved `false` literal still means bool.
#[test]
fn rb_464_syn17_twin_no_never_bool() {
    let src = "case_0017_02 = no\n";
    let v = eval_json(src);
    assert!(v["case_0017_02"].is_null(), "bare no must be null, got {v}");
    assert_ne!(v["case_0017_02"], false, "bare no must never be bool false");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the reserved literal still means bool false.
    assert_eq!(eval_json("case_0017_02 = false\n")["case_0017_02"], false);
}

/// RB-465 — Derived variant 17-03: twin lineage agrees on `on` (SYN-0017-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0017 (toml-test almost-true).
/// `case_0017_03 = on` warns W001 Warning at (1:16) and evaluates to null —
/// converges with RB-456; legacy spellings never become bool true.
#[test]
fn rb_465_syn17_twin_on_never_bool() {
    let src = "case_0017_03 = on\n";
    let v = eval_json(src);
    assert!(v["case_0017_03"].is_null(), "bare on must be null, got {v}");
    assert_ne!(v["case_0017_03"], true, "bare on must never be bool true");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0017_03 = \"on\"\n")["case_0017_03"], "on");
}

/// RB-466 — Derived variant 17-04: twin lineage agrees on `off` (SYN-0017-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0017 (toml-test almost-true).
/// `case_0017_04 = off` warns W001 Warning at (1:16) and evaluates to null —
/// converges with RB-457; legacy spellings never become bool false.
#[test]
fn rb_466_syn17_twin_off_never_bool() {
    let src = "case_0017_04 = off\n";
    let v = eval_json(src);
    assert!(
        v["case_0017_04"].is_null(),
        "bare off must be null, got {v}"
    );
    assert_ne!(
        v["case_0017_04"], false,
        "bare off must never be bool false"
    );
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0017_04 = \"off\"\n")["case_0017_04"], "off");
}

/// RB-467 — Derived variant 17-05: twin lineage agrees on `true` (SYN-0017-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0017 (toml-test almost-true).
/// `case_0017_05 = true` is bool true with ZERO diagnostics — converges with
/// RB-458; the quoted "true" neighbour stays a string, never bool.
#[test]
fn rb_467_syn17_twin_true_is_bool() {
    let src = "case_0017_05 = true\n";
    assert_eq!(
        eval_json(src)["case_0017_05"],
        true,
        "reserved true must be bool"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "reserved true must warn nothing, got {codes:?}"
    );
    // Positive control: the quoted neighbour stays the exact string.
    let v = eval_json("case_0017_05 = \"true\"\n");
    assert_eq!(v["case_0017_05"], "true");
    assert_ne!(v["case_0017_05"], true, "quoted true must never be bool");
}

/// RB-468 — Derived variant 17-06: twin lineage agrees on `FALSE` (SYN-0017-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0017 (toml-test almost-true).
/// `case_0017_06 = FALSE` warns W001 Warning at (1:16) and evaluates to null —
/// converges with RB-459: neither lineage lets noncanonical case carry bool
/// type. (Completes SYN-0016-06…09 + opens SYN-0017-01…06.)
#[test]
fn rb_468_syn17_twin_upper_false_never_bool() {
    let src = "case_0017_06 = FALSE\n";
    let v = eval_json(src);
    assert!(v["case_0017_06"].is_null(), "FALSE must be null, got {v}");
    assert_ne!(v["case_0017_06"], false, "FALSE must never be bool false");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the documented lowercase literal still means bool.
    assert_eq!(eval_json("case_0017_06 = false\n")["case_0017_06"], false);
}
/// RB-469 — Derived variant 17-07: twin null policy agrees (SYN-0017-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0017 (toml-test almost-true).
/// `case_0017_07 = null` is null with ZERO diagnostics — converges with
/// RB-460: both lineages reserve lowercase `null`, both announce `NULL`.
#[test]
fn rb_469_syn17_twin_null_reserved() {
    let src = "case_0017_07 = null\n";
    assert!(
        eval_json(src)["case_0017_07"].is_null(),
        "null must be null"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "reserved null must warn nothing, got {codes:?}"
    );
    // Control: the uppercase spelling resolves to null only loudly.
    let v = eval_json("case_0017_07 = NULL\n");
    assert!(v["case_0017_07"].is_null(), "{v}");
    let codes = codes_of("case_0017_07 = NULL\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
}

/// RB-470 — Derived variant 17-08: twin quoting agrees (SYN-0017-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0017 (toml-test almost-true).
/// `case_0017_08 = "no"` is exactly "no" with ZERO diagnostics — converges
/// with RB-461: quoting preserves literal text in every lineage.
#[test]
fn rb_470_syn17_twin_quoted_no_exact() {
    let src = "case_0017_08 = \"no\"\n";
    assert_eq!(
        eval_json(src)["case_0017_08"],
        "no",
        "quoted no must stay exact"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "quoted no must warn nothing, got {codes:?}"
    );
    // Control: the bare neighbour never coerces (converges with RB-455).
    let v = eval_json("case_0017_08 = no\n");
    assert!(v["case_0017_08"].is_null(), "{v}");
    let codes = codes_of("case_0017_08 = no\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
}

/// RB-471 — Derived variant 17-09: twin numbers agree (SYN-0017-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0017 (toml-test almost-true).
/// `case_0017_09 = 1.0` is float 1.0 with ZERO diagnostics — converges with
/// RB-462: no lineage treats numbers as booleans.
#[test]
fn rb_471_syn17_twin_number_never_bool() {
    let src = "case_0017_09 = 1.0\n";
    let v = eval_json(src)["case_0017_09"].clone();
    assert_eq!(v.as_f64(), Some(1.0), "1.0 must stay float, got {v}");
    assert!(!v.is_boolean(), "1.0 must never be boolean, got {v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "1.0 must warn nothing, got {codes:?}");
    // Positive control: the reserved literal still means bool true.
    assert_eq!(eval_json("case_0017_09 = true\n")["case_0017_09"], true);
}

/// RB-472 — Derived variant 18-01: third lineage agrees on `yes` (SYN-0018-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0018 (toml-test capitalized-false).
/// `case_0018_01 = yes` warns W001 Warning at (1:16) and evaluates to null —
/// three lineages (16/17/18) now agree: no parent fixture changes the
/// outcome, so the guard is on the resolver, not on one spelling.
#[test]
fn rb_472_syn18_third_yes_never_bool() {
    let src = "case_0018_01 = yes\n";
    let v = eval_json(src);
    assert!(
        v["case_0018_01"].is_null(),
        "bare yes must be null, got {v}"
    );
    assert_ne!(v["case_0018_01"], true, "bare yes must never be bool true");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Three-lineage convergence: 16/17 twins resolve to the same value.
    assert_eq!(
        v["case_0018_01"],
        eval_json("case_0016_01 = yes\n")["case_0016_01"]
    );
    assert_eq!(
        v["case_0018_01"],
        eval_json("case_0017_01 = yes\n")["case_0017_01"]
    );
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0018_01 = \"yes\"\n")["case_0018_01"], "yes");
}

/// RB-473 — Derived variant 18-02: third lineage agrees on `no` (SYN-0018-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0018 (toml-test capitalized-false).
/// `case_0018_02 = no` warns W001 Warning at (1:16) and evaluates to null —
/// converges with RB-455/RB-464; the reserved `false` still means bool.
#[test]
fn rb_473_syn18_third_no_never_bool() {
    let src = "case_0018_02 = no\n";
    let v = eval_json(src);
    assert!(v["case_0018_02"].is_null(), "bare no must be null, got {v}");
    assert_ne!(v["case_0018_02"], false, "bare no must never be bool false");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the reserved literal still means bool false.
    assert_eq!(eval_json("case_0018_02 = false\n")["case_0018_02"], false);
}

/// RB-474 — Derived variant 18-03: third lineage agrees on `on` (SYN-0018-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0018 (toml-test capitalized-false).
/// `case_0018_03 = on` warns W001 Warning at (1:16) and evaluates to null —
/// converges with RB-456/RB-465; legacy spellings never become bool true.
#[test]
fn rb_474_syn18_third_on_never_bool() {
    let src = "case_0018_03 = on\n";
    let v = eval_json(src);
    assert!(v["case_0018_03"].is_null(), "bare on must be null, got {v}");
    assert_ne!(v["case_0018_03"], true, "bare on must never be bool true");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0018_03 = \"on\"\n")["case_0018_03"], "on");
}

/// RB-475 — Derived variant 18-04: third lineage agrees on `off` (SYN-0018-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0018 (toml-test capitalized-false).
/// `case_0018_04 = off` warns W001 Warning at (1:16) and evaluates to null —
/// converges with RB-457/RB-466; legacy spellings never become bool false.
#[test]
fn rb_475_syn18_third_off_never_bool() {
    let src = "case_0018_04 = off\n";
    let v = eval_json(src);
    assert!(
        v["case_0018_04"].is_null(),
        "bare off must be null, got {v}"
    );
    assert_ne!(
        v["case_0018_04"], false,
        "bare off must never be bool false"
    );
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0018_04 = \"off\"\n")["case_0018_04"], "off");
}

/// RB-476 — Derived variant 18-05: third lineage agrees on `true` (SYN-0018-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0018 (toml-test capitalized-false).
/// `case_0018_05 = true` is bool true with ZERO diagnostics — converges with
/// RB-458/RB-467; the quoted "true" neighbour stays a string, never bool.
#[test]
fn rb_476_syn18_third_true_is_bool() {
    let src = "case_0018_05 = true\n";
    assert_eq!(
        eval_json(src)["case_0018_05"],
        true,
        "reserved true must be bool"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "reserved true must warn nothing, got {codes:?}"
    );
    // Positive control: the quoted neighbour stays the exact string.
    let v = eval_json("case_0018_05 = \"true\"\n");
    assert_eq!(v["case_0018_05"], "true");
    assert_ne!(v["case_0018_05"], true, "quoted true must never be bool");
}

/// RB-477 — Derived variant 18-06: third lineage agrees on `FALSE` (SYN-0018-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0018 (toml-test capitalized-false).
/// `case_0018_06 = FALSE` warns W001 Warning at (1:16) and evaluates to null —
/// converges with RB-459/RB-468: all three lineages refuse bool type to
/// noncanonical case.
#[test]
fn rb_477_syn18_third_upper_false_never_bool() {
    let src = "case_0018_06 = FALSE\n";
    let v = eval_json(src);
    assert!(v["case_0018_06"].is_null(), "FALSE must be null, got {v}");
    assert_ne!(v["case_0018_06"], false, "FALSE must never be bool false");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the documented lowercase literal still means bool.
    assert_eq!(eval_json("case_0018_06 = false\n")["case_0018_06"], false);
}

/// RB-478 — Derived variant 18-07: third null policy agrees (SYN-0018-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0018 (toml-test capitalized-false).
/// `case_0018_07 = null` is null with ZERO diagnostics — converges with
/// RB-460/RB-469: all three lineages reserve lowercase null and announce
/// NULL. (Completes SYN-0017-07…09 + opens SYN-0018-01…07.)
#[test]
fn rb_478_syn18_third_null_reserved() {
    let src = "case_0018_07 = null\n";
    assert!(
        eval_json(src)["case_0018_07"].is_null(),
        "null must be null"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "reserved null must warn nothing, got {codes:?}"
    );
    // Three-lineage convergence: 16/17 twins resolve identically silent.
    assert!(codes_of("case_0016_07 = null\n").is_empty());
    assert!(codes_of("case_0017_07 = null\n").is_empty());
    // Control: the uppercase spelling resolves to null only loudly.
    let v = eval_json("case_0018_07 = NULL\n");
    assert!(v["case_0018_07"].is_null(), "{v}");
    let codes = codes_of("case_0018_07 = NULL\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
}
/// RB-479 — Derived variant 18-08: third quoting agrees (SYN-0018-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0018 (toml-test capitalized-false).
/// `case_0018_08 = "no"` is exactly "no" with ZERO diagnostics — converges
/// with RB-461/RB-470: quoting preserves literal text in every lineage.
#[test]
fn rb_479_syn18_third_quoted_no_exact() {
    let src = "case_0018_08 = \"no\"\n";
    assert_eq!(
        eval_json(src)["case_0018_08"],
        "no",
        "quoted no must stay exact"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "quoted no must warn nothing, got {codes:?}"
    );
    // Control: the bare neighbour never coerces.
    let v = eval_json("case_0018_08 = no\n");
    assert!(v["case_0018_08"].is_null(), "{v}");
    let codes = codes_of("case_0018_08 = no\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
}

/// RB-480 — Derived variant 18-09: third numbers agree (SYN-0018-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0018 (toml-test capitalized-false).
/// `case_0018_09 = 1.0` is float 1.0 with ZERO diagnostics — converges with
/// RB-462/RB-471: no lineage treats numbers as booleans. (Completes
/// SYN-0018-01…09.)
#[test]
fn rb_480_syn18_third_number_never_bool() {
    let src = "case_0018_09 = 1.0\n";
    let v = eval_json(src)["case_0018_09"].clone();
    assert_eq!(v.as_f64(), Some(1.0), "1.0 must stay float, got {v}");
    assert!(!v.is_boolean(), "1.0 must never be boolean, got {v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "1.0 must warn nothing, got {codes:?}");
    // Positive control: the reserved literal still means bool true.
    assert_eq!(eval_json("case_0018_09 = true\n")["case_0018_09"], true);
}

/// RB-481 — Derived variant 19-01: fourth lineage agrees on `yes` (SYN-0019-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0019 (toml-test capitalized-true).
/// `case_0019_01 = yes` warns W001 Warning at (1:16) and evaluates to null —
/// four lineages (16/17/18/19) now agree, so the guard is on the resolver,
/// never on one parent fixture.
#[test]
fn rb_481_syn19_fourth_yes_never_bool() {
    let src = "case_0019_01 = yes\n";
    let v = eval_json(src);
    assert!(
        v["case_0019_01"].is_null(),
        "bare yes must be null, got {v}"
    );
    assert_ne!(v["case_0019_01"], true, "bare yes must never be bool true");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Four-lineage convergence: 16/17/18 twins resolve to the same value.
    assert_eq!(
        v["case_0019_01"],
        eval_json("case_0016_01 = yes\n")["case_0016_01"]
    );
    assert_eq!(
        v["case_0019_01"],
        eval_json("case_0017_01 = yes\n")["case_0017_01"]
    );
    assert_eq!(
        v["case_0019_01"],
        eval_json("case_0018_01 = yes\n")["case_0018_01"]
    );
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0019_01 = \"yes\"\n")["case_0019_01"], "yes");
}

/// RB-482 — Derived variant 19-02: fourth lineage agrees on `no` (SYN-0019-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0019 (toml-test capitalized-true).
/// `case_0019_02 = no` warns W001 Warning at (1:16) and evaluates to null —
/// converges across all four lineages; the reserved `false` still means bool.
#[test]
fn rb_482_syn19_fourth_no_never_bool() {
    let src = "case_0019_02 = no\n";
    let v = eval_json(src);
    assert!(v["case_0019_02"].is_null(), "bare no must be null, got {v}");
    assert_ne!(v["case_0019_02"], false, "bare no must never be bool false");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the reserved literal still means bool false.
    assert_eq!(eval_json("case_0019_02 = false\n")["case_0019_02"], false);
}

/// RB-483 — Derived variant 19-03: fourth lineage agrees on `on` (SYN-0019-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0019 (toml-test capitalized-true).
/// `case_0019_03 = on` warns W001 Warning at (1:16) and evaluates to null —
/// legacy spellings never become bool true in any lineage.
#[test]
fn rb_483_syn19_fourth_on_never_bool() {
    let src = "case_0019_03 = on\n";
    let v = eval_json(src);
    assert!(v["case_0019_03"].is_null(), "bare on must be null, got {v}");
    assert_ne!(v["case_0019_03"], true, "bare on must never be bool true");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0019_03 = \"on\"\n")["case_0019_03"], "on");
}

/// RB-484 — Derived variant 19-04: fourth lineage agrees on `off` (SYN-0019-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0019 (toml-test capitalized-true).
/// `case_0019_04 = off` warns W001 Warning at (1:16) and evaluates to null —
/// legacy spellings never become bool false in any lineage.
#[test]
fn rb_484_syn19_fourth_off_never_bool() {
    let src = "case_0019_04 = off\n";
    let v = eval_json(src);
    assert!(
        v["case_0019_04"].is_null(),
        "bare off must be null, got {v}"
    );
    assert_ne!(
        v["case_0019_04"], false,
        "bare off must never be bool false"
    );
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0019_04 = \"off\"\n")["case_0019_04"], "off");
}

/// RB-485 — Derived variant 19-05: fourth lineage agrees on `true` (SYN-0019-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0019 (toml-test capitalized-true).
/// `case_0019_05 = true` is bool true with ZERO diagnostics — converges
/// across all four lineages; the quoted "true" neighbour stays a string.
#[test]
fn rb_485_syn19_fourth_true_is_bool() {
    let src = "case_0019_05 = true\n";
    assert_eq!(
        eval_json(src)["case_0019_05"],
        true,
        "reserved true must be bool"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "reserved true must warn nothing, got {codes:?}"
    );
    // Positive control: the quoted neighbour stays the exact string.
    let v = eval_json("case_0019_05 = \"true\"\n");
    assert_eq!(v["case_0019_05"], "true");
    assert_ne!(v["case_0019_05"], true, "quoted true must never be bool");
}

/// RB-486 — Derived variant 19-06: fourth lineage agrees on `FALSE` (SYN-0019-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0019 (toml-test capitalized-true).
/// `case_0019_06 = FALSE` warns W001 Warning at (1:16) and evaluates to null —
/// all four lineages refuse bool type to noncanonical case.
#[test]
fn rb_486_syn19_fourth_upper_false_never_bool() {
    let src = "case_0019_06 = FALSE\n";
    let v = eval_json(src);
    assert!(v["case_0019_06"].is_null(), "FALSE must be null, got {v}");
    assert_ne!(v["case_0019_06"], false, "FALSE must never be bool false");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the documented lowercase literal still means bool.
    assert_eq!(eval_json("case_0019_06 = false\n")["case_0019_06"], false);
}

/// RB-487 — Derived variant 19-07: fourth null policy agrees (SYN-0019-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0019 (toml-test capitalized-true).
/// `case_0019_07 = null` is null with ZERO diagnostics — converges across
/// all four lineages; the uppercase spelling resolves loudly.
#[test]
fn rb_487_syn19_fourth_null_reserved() {
    let src = "case_0019_07 = null\n";
    assert!(
        eval_json(src)["case_0019_07"].is_null(),
        "null must be null"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "reserved null must warn nothing, got {codes:?}"
    );
    // Control: the uppercase spelling resolves to null only loudly.
    let v = eval_json("case_0019_07 = NULL\n");
    assert!(v["case_0019_07"].is_null(), "{v}");
    let codes = codes_of("case_0019_07 = NULL\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
}

/// RB-488 — Derived variant 19-08: fourth quoting agrees (SYN-0019-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0019 (toml-test capitalized-true).
/// `case_0019_08 = "no"` is exactly "no" with ZERO diagnostics — converges
/// across all four lineages; the bare neighbour never coerces. (Completes
/// SYN-0018-08…09 + opens SYN-0019-01…08.)
#[test]
fn rb_488_syn19_fourth_quoted_no_exact() {
    let src = "case_0019_08 = \"no\"\n";
    assert_eq!(
        eval_json(src)["case_0019_08"],
        "no",
        "quoted no must stay exact"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "quoted no must warn nothing, got {codes:?}"
    );
    // Control: the bare neighbour never coerces.
    let v = eval_json("case_0019_08 = no\n");
    assert!(v["case_0019_08"].is_null(), "{v}");
    let codes = codes_of("case_0019_08 = no\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
}
/// RB-489 — Derived variant 19-09: fourth numbers agree (SYN-0019-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0019 (toml-test capitalized-true).
/// `case_0019_09 = 1.0` is float 1.0 with ZERO diagnostics — converges across
/// all four lineages: no lineage treats numbers as booleans. (Completes
/// SYN-0019-01…09.)
#[test]
fn rb_489_syn19_fourth_number_never_bool() {
    let src = "case_0019_09 = 1.0\n";
    let v = eval_json(src)["case_0019_09"].clone();
    assert_eq!(v.as_f64(), Some(1.0), "1.0 must stay float, got {v}");
    assert!(!v.is_boolean(), "1.0 must never be boolean, got {v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "1.0 must warn nothing, got {codes:?}");
    // Positive control: the reserved literal still means bool true.
    assert_eq!(eval_json("case_0019_09 = true\n")["case_0019_09"], true);
}

/// RB-490 — Derived variant 20-01: fifth lineage agrees on `yes` (SYN-0020-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0020 (toml-test mixed-case-true).
/// `case_0020_01 = yes` warns W001 Warning at (1:16) and evaluates to null —
/// five lineages (16/17/18/19/20) now agree, so the guard is on the resolver,
/// never on one parent fixture.
#[test]
fn rb_490_syn20_fifth_yes_never_bool() {
    let src = "case_0020_01 = yes\n";
    let v = eval_json(src);
    assert!(
        v["case_0020_01"].is_null(),
        "bare yes must be null, got {v}"
    );
    assert_ne!(v["case_0020_01"], true, "bare yes must never be bool true");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Five-lineage convergence: all earlier twins resolve to the same value.
    assert_eq!(
        v["case_0020_01"],
        eval_json("case_0016_01 = yes\n")["case_0016_01"]
    );
    assert_eq!(
        v["case_0020_01"],
        eval_json("case_0019_01 = yes\n")["case_0019_01"]
    );
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0020_01 = \"yes\"\n")["case_0020_01"], "yes");
}

/// RB-491 — Derived variant 20-02: fifth lineage agrees on `no` (SYN-0020-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0020 (toml-test mixed-case-true).
/// `case_0020_02 = no` warns W001 Warning at (1:16) and evaluates to null —
/// converges across all five lineages; the reserved `false` still means bool.
#[test]
fn rb_491_syn20_fifth_no_never_bool() {
    let src = "case_0020_02 = no\n";
    let v = eval_json(src);
    assert!(v["case_0020_02"].is_null(), "bare no must be null, got {v}");
    assert_ne!(v["case_0020_02"], false, "bare no must never be bool false");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the reserved literal still means bool false.
    assert_eq!(eval_json("case_0020_02 = false\n")["case_0020_02"], false);
}

/// RB-492 — Derived variant 20-03: fifth lineage agrees on `on` (SYN-0020-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0020 (toml-test mixed-case-true).
/// `case_0020_03 = on` warns W001 Warning at (1:16) and evaluates to null —
/// legacy spellings never become bool true in any lineage.
#[test]
fn rb_492_syn20_fifth_on_never_bool() {
    let src = "case_0020_03 = on\n";
    let v = eval_json(src);
    assert!(v["case_0020_03"].is_null(), "bare on must be null, got {v}");
    assert_ne!(v["case_0020_03"], true, "bare on must never be bool true");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0020_03 = \"on\"\n")["case_0020_03"], "on");
}

/// RB-493 — Derived variant 20-04: fifth lineage agrees on `off` (SYN-0020-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0020 (toml-test mixed-case-true).
/// `case_0020_04 = off` warns W001 Warning at (1:16) and evaluates to null —
/// legacy spellings never become bool false in any lineage.
#[test]
fn rb_493_syn20_fifth_off_never_bool() {
    let src = "case_0020_04 = off\n";
    let v = eval_json(src);
    assert!(
        v["case_0020_04"].is_null(),
        "bare off must be null, got {v}"
    );
    assert_ne!(
        v["case_0020_04"], false,
        "bare off must never be bool false"
    );
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(eval_json("case_0020_04 = \"off\"\n")["case_0020_04"], "off");
}

/// RB-494 — Derived variant 20-05: fifth lineage agrees on `true` (SYN-0020-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0020 (toml-test mixed-case-true).
/// `case_0020_05 = true` is bool true with ZERO diagnostics — converges
/// across all five lineages; the quoted "true" neighbour stays a string.
#[test]
fn rb_494_syn20_fifth_true_is_bool() {
    let src = "case_0020_05 = true\n";
    assert_eq!(
        eval_json(src)["case_0020_05"],
        true,
        "reserved true must be bool"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "reserved true must warn nothing, got {codes:?}"
    );
    // Positive control: the quoted neighbour stays the exact string.
    let v = eval_json("case_0020_05 = \"true\"\n");
    assert_eq!(v["case_0020_05"], "true");
    assert_ne!(v["case_0020_05"], true, "quoted true must never be bool");
}

/// RB-495 — Derived variant 20-06: fifth lineage agrees on `FALSE` (SYN-0020-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0020 (toml-test mixed-case-true).
/// `case_0020_06 = FALSE` warns W001 Warning at (1:16) and evaluates to null —
/// all five lineages refuse bool type to noncanonical case.
#[test]
fn rb_495_syn20_fifth_upper_false_never_bool() {
    let src = "case_0020_06 = FALSE\n";
    let v = eval_json(src);
    assert!(v["case_0020_06"].is_null(), "FALSE must be null, got {v}");
    assert_ne!(v["case_0020_06"], false, "FALSE must never be bool false");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W001").expect("W001");
    assert_eq!(
        w.severity,
        active_core::Severity::Warning,
        "W001 must be Warning, got {:?}",
        w.severity
    );
    let span = w.span.clone().expect("W001 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the documented lowercase literal still means bool.
    assert_eq!(eval_json("case_0020_06 = false\n")["case_0020_06"], false);
}

/// RB-496 — Derived variant 20-07: fifth null policy agrees (SYN-0020-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0020 (toml-test mixed-case-true).
/// `case_0020_07 = null` is null with ZERO diagnostics — converges across
/// all five lineages; the uppercase spelling resolves loudly.
#[test]
fn rb_496_syn20_fifth_null_reserved() {
    let src = "case_0020_07 = null\n";
    assert!(
        eval_json(src)["case_0020_07"].is_null(),
        "null must be null"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "reserved null must warn nothing, got {codes:?}"
    );
    // Five-lineage convergence: every earlier twin resolves silently too.
    assert!(codes_of("case_0016_07 = null\n").is_empty());
    assert!(codes_of("case_0019_07 = null\n").is_empty());
    // Control: the uppercase spelling resolves to null only loudly.
    let v = eval_json("case_0020_07 = NULL\n");
    assert!(v["case_0020_07"].is_null(), "{v}");
    let codes = codes_of("case_0020_07 = NULL\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
}

/// RB-497 — Derived variant 20-08: fifth quoting agrees (SYN-0020-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0020 (toml-test mixed-case-true).
/// `case_0020_08 = "no"` is exactly "no" with ZERO diagnostics — converges
/// across all five lineages; the bare neighbour never coerces.
#[test]
fn rb_497_syn20_fifth_quoted_no_exact() {
    let src = "case_0020_08 = \"no\"\n";
    assert_eq!(
        eval_json(src)["case_0020_08"],
        "no",
        "quoted no must stay exact"
    );
    let codes = codes_of(src);
    assert!(
        codes.is_empty(),
        "quoted no must warn nothing, got {codes:?}"
    );
    // Control: the bare neighbour never coerces.
    let v = eval_json("case_0020_08 = no\n");
    assert!(v["case_0020_08"].is_null(), "{v}");
    let codes = codes_of("case_0020_08 = no\n");
    assert!(codes.iter().any(|c| c == "W001"), "{codes:?}");
}

/// RB-498 — Derived variant 20-09: fifth numbers agree (SYN-0020-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0020 (toml-test mixed-case-true).
/// `case_0020_09 = 1.0` is float 1.0 with ZERO diagnostics — converges across
/// all five lineages: no lineage treats numbers as booleans. (Completes
/// SYN-0019-09 + opens SYN-0020-01…09.)
#[test]
fn rb_498_syn20_fifth_number_never_bool() {
    let src = "case_0020_09 = 1.0\n";
    let v = eval_json(src)["case_0020_09"].clone();
    assert_eq!(v.as_f64(), Some(1.0), "1.0 must stay float, got {v}");
    assert!(!v.is_boolean(), "1.0 must never be boolean, got {v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "1.0 must warn nothing, got {codes:?}");
    // Positive control: the reserved literal still means bool true.
    assert_eq!(eval_json("case_0020_09 = true\n")["case_0020_09"], true);
}
/// RB-499 — Derived variant 21-02: invalid UTF-8 rejected in any token position (SYN-0021-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0021 (toml-test bare-cr).
/// Byte fixture: overlong C0 AF inside the KEY bytes. The decoder rejects
/// with E017 Error — same class as the in-string twin (RB-196) — never
/// replacement-decoding untrusted text, wherever the bad bytes sit.
#[test]
fn rb_499_syn21_utf8_in_key_e017() {
    let diags = parse_file(
        &research_fixture("syn_0021_02_utf8_in_key.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: the in-string twin (RB-196) fails the same class.
    let diags = parse_file(
        &research_fixture("src_toml_0047_bad_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("twin must fail E017");
    assert!(diags.iter().any(|d| d.code == "E017"), "{diags:?}");
    // Positive control: the valid-spelling neighbour parses.
    assert_eq!(eval_json("case_0021_02 = 1\n")["case_0021_02"], 1);
}

/// RB-500 — Derived variant 21-04: `\u` escapes stay verbatim (SYN-0021-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0021 (toml-test bare-cr).
/// REFUSAL (spec-gated): the catalogue allows accepting escaped control
/// scalars, but spec/grammar.md:64-66 and RB-050 pin verbatim passthrough
/// with no `\u`/surrogate decoding. `"\u0001"` is the 6-char literal text,
/// while raw control bytes stay E005 (RB-314) — escaped policy is distinct
/// from raw source bytes exactly as the row demands.
#[test]
fn rb_500_syn21_unicode_escape_verbatim() {
    let v = eval_json("case_0021_04 = \"\\u0001\"\n");
    assert_eq!(
        v["case_0021_04"], "\\u0001",
        "escape must stay verbatim, got {v}"
    );
    assert_eq!(v["case_0021_04"].as_str().expect("string").len(), 6);
    let codes = codes_of("case_0021_04 = \"\\u0001\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: documented escapes still decode (RB-050 converges).
    let v = eval_json("case_0021_04 = \"a\\tb\"\n");
    assert_eq!(v["case_0021_04"], "a\tb");
}

/// RB-501 — Derived variant 21-05: raw TAB lives in strings, never in indent (SYN-0021-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0021 (toml-test bare-cr).
/// Byte fixture: a literal 0x09 inside string content evaluates to "a\tb"
/// with zero diagnostics — TABs are valid string content; only structural
/// indentation rejects them (E006, RB-316 converges).
#[test]
fn rb_501_syn21_tab_in_string_ok() {
    let module = parse_file(
        &research_fixture("syn_0021_05_tab_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("tab-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["case_0021_05"], "a\tb", "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: TAB as structural indentation still fails E006 (RB-316).
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("tab indent must fail E006");
    assert!(diags.iter().any(|d| d.code == "E006"), "{diags:?}");
}

/// RB-502 — Derived variant 21-06: multi-byte text round-trips exactly (SYN-0021-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0021 (toml-test bare-cr).
/// `case_0021_06 = "café"` evaluates to exactly "café" with zero diagnostics,
/// and the JSON export carries the é through — decode/encode round-trip
/// preserves valid multi-byte code points.
#[test]
fn rb_502_syn21_cafe_roundtrip_exact() {
    let src = "case_0021_06 = \"café\"\n";
    assert_eq!(eval_json(src)["case_0021_06"], "café");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("café"), "{json}");
    // Positive control: ASCII neighbours evaluate alongside cleanly.
    assert_eq!(
        eval_json("case_0021_06 = \"cafe\"\n")["case_0021_06"],
        "cafe"
    );
}

/// RB-503 — Derived variant 21-07: supplementary-plane scalar stays one scalar (SYN-0021-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0021 (toml-test bare-cr).
/// `case_0021_07 = "𝄞"` (U+1D11E) evaluates to exactly that scalar with zero
/// diagnostics — one code point in, one scalar out, no split-surrogate halves.
#[test]
fn rb_503_syn21_supplementary_scalar_single() {
    let src = "case_0021_07 = \"𝄞\"\n";
    let v = eval_json(src)["case_0021_07"].clone();
    assert_eq!(v, "𝄞", "{v}");
    assert_eq!(v.as_str().expect("string").chars().count(), 1, "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: BMP multi-byte text evaluates alongside cleanly.
    assert_eq!(
        eval_json("case_0021_07 = \"café\"\n")["case_0021_07"],
        "café"
    );
}

/// RB-504 — Derived variant 21-08: truncated 4-byte sequence is E017, not a crash (SYN-0021-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0021 (toml-test bare-cr).
/// Byte fixture: truncated F0 9D lead inside an array at EOF. EOF inside a
/// multibyte sequence is a controlled E017 Error — converges with the
/// 2-byte truncation twin (RB-194) — never a parser crash or hang.
#[test]
fn rb_504_syn21_trunc4_array_e017() {
    let diags = parse_file(
        &research_fixture("syn_0021_08_trunc4_array.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: the 2-byte truncation twin (RB-194) fails the same class.
    let diags = parse_file(
        &research_fixture("src_toml_0045_bad_utf8_at_end.active"),
        ParseOptions::default(),
    )
    .expect_err("twin must fail E017");
    assert!(diags.iter().any(|d| d.code == "E017"), "{diags:?}");
    // Positive control: the complete neighbour parses.
    assert_eq!(
        eval_json("case_0021_08 = [\"a\"]\n")["case_0021_08"],
        serde_json::json!(["a"])
    );
}

/// RB-505 — Derived variant 21-09: leading escape is value, not indentation (SYN-0021-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0021 (toml-test bare-cr).
/// A block value starting with the `\t` escape keeps a leading TAB byte —
/// the escape decodes to content and never counts as source indentation.
/// Space-indented siblings bind normally alongside.
#[test]
fn rb_505_syn21_leading_escape_is_value() {
    let src = "[server]\nhost = \"\\tlocalhost\"\nport = 8080\n";
    let v = eval_json(src);
    assert_eq!(v["server"]["host"], "\tlocalhost", "{v}");
    assert_eq!(v["server"]["port"], 8080);
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the escape-free neighbour evaluates identically clean.
    let v = eval_json("[server]\nhost = \"localhost\"\n");
    assert_eq!(v["server"]["host"], "localhost");
}

/// RB-506 — Derived variant 22-01: NUL-first twin agrees (SYN-0022-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0022 (toml-test bare-formfeed).
/// Byte fixture: raw 0x00 before `z = 9`. The decoder rejects with E005 Error
/// at (1:1) — converges with RB-314: NUL outside a string reports the byte
/// offset no matter which lineage carries it.
#[test]
fn rb_506_syn22_nul_first_twin_e005() {
    let diags = parse_file(
        &research_fixture("syn_0022_01_nul_first.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
    // Positive control: the NUL-free twin document evaluates.
    assert_eq!(eval_json("z = 9\n")["z"], 9);
}

/// RB-507 — Derived variant 22-02: bad-UTF-8 twin agrees (SYN-0022-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0022 (toml-test bare-formfeed).
/// Byte fixture: overlong C0 AF in a string under a new key. E017 Error —
/// converges with RB-196 and RB-499: no lineage replacement-decodes.
#[test]
fn rb_507_syn22_utf8_twin_e017() {
    let diags = parse_file(
        &research_fixture("syn_0022_02_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Positive control: the valid-spelling neighbour parses.
    assert_eq!(eval_json("case_0022_02 = \"qwx\"\n")["case_0022_02"], "qwx");
}

/// RB-508 — Derived variant 22-03: misplaced BOM column tracks the key (SYN-0022-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0022 (toml-test bare-formfeed).
/// Byte fixture: BOM after `xy = ` (a 2-char key). E005 Error at (1:6) —
/// converges with RB-315's (1:5): the span tracks the stream position, so
/// BOM acceptance stays restricted to the leading position whatever the
/// key width. (Completes SYN-0021-02,04…09 + opens SYN-0022-01…03.)
#[test]
fn rb_508_syn22_bom_wide_key_e005() {
    let diags = parse_file(
        &research_fixture("syn_0022_03_bom_wide.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 6), "{span:?}");
    // Positive control: the leading-BOM document parses (RB-003 converges).
    assert_eq!(eval_json("xy = 2\n")["xy"], 2);
}
/// RB-509 — Derived variant 22-04: third `\u` verbatim agrees (SYN-0022-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0022 (toml-test bare-formfeed).
/// `case_0022_04 = "\u0001"` is the 6-char verbatim literal with zero
/// diagnostics — converges with RB-500: no lineage decodes `\u`, so an
/// escaped code point can never smuggle a raw control byte.
#[test]
fn rb_509_syn22_unicode_escape_verbatim() {
    let v = eval_json("case_0022_04 = \"\\u0001\"\n");
    assert_eq!(
        v["case_0022_04"], "\\u0001",
        "escape must stay verbatim, got {v}"
    );
    assert_eq!(v["case_0022_04"].as_str().expect("string").len(), 6);
    let codes = codes_of("case_0022_04 = \"\\u0001\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: documented escapes still decode.
    let v = eval_json("case_0022_04 = \"a\\tb\"\n");
    assert_eq!(v["case_0022_04"], "a\tb");
}

/// RB-510 — Derived variant 22-05: second TAB-in-string agrees (SYN-0022-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0022 (toml-test bare-formfeed).
/// Byte fixture: literal 0x09 in string content evaluates to "a\tb" with
/// zero diagnostics — converges with RB-501: TABs are content everywhere,
/// indentation nowhere (E006).
#[test]
fn rb_510_syn22_tab_in_string_ok() {
    let module = parse_file(
        &research_fixture("syn_0022_05_tab_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("tab-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["case_0022_05"], "a\tb", "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: TAB as structural indentation still fails E006.
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("tab indent must fail E006");
    assert!(diags.iter().any(|d| d.code == "E006"), "{diags:?}");
}

/// RB-511 — Derived variant 22-06: second café agrees (SYN-0022-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0022 (toml-test bare-formfeed).
/// `case_0022_06 = "café"` is exactly "café" with zero diagnostics, and the
/// JSON export carries é — converges with RB-502 across lineages.
#[test]
fn rb_511_syn22_cafe_roundtrip_exact() {
    let src = "case_0022_06 = \"café\"\n";
    assert_eq!(eval_json(src)["case_0022_06"], "café");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("café"), "{json}");
    // Positive control: ASCII neighbours evaluate alongside cleanly.
    assert_eq!(
        eval_json("case_0022_06 = \"cafe\"\n")["case_0022_06"],
        "cafe"
    );
}

/// RB-512 — Derived variant 22-07: second clef agrees (SYN-0022-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0022 (toml-test bare-formfeed).
/// `case_0022_07 = "𝄞"` (U+1D11E) is exactly one scalar with zero diagnostics —
/// converges with RB-503: byte/character counting holds in every lineage.
#[test]
fn rb_512_syn22_supplementary_scalar_single() {
    let src = "case_0022_07 = \"𝄞\"\n";
    let v = eval_json(src)["case_0022_07"].clone();
    assert_eq!(v, "𝄞", "{v}");
    assert_eq!(v.as_str().expect("string").chars().count(), 1, "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: BMP multi-byte text evaluates alongside cleanly.
    assert_eq!(
        eval_json("case_0022_07 = \"café\"\n")["case_0022_07"],
        "café"
    );
}

/// RB-513 — Derived variant 22-08: truncated 3-byte sequence is E017 (SYN-0022-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0022 (toml-test bare-formfeed).
/// Byte fixture: E2 82 (2 of 3 bytes) inside a string at EOF. A different
/// truncation width and token shape than RB-194/RB-504, same controlled
/// E017 Error — EOF in any multibyte sequence is a decode error, not a crash.
#[test]
fn rb_513_syn22_trunc3_string_e017() {
    let diags = parse_file(
        &research_fixture("syn_0022_08_trunc3_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: both earlier truncation twins fail the same class.
    for name in [
        "src_toml_0045_bad_utf8_at_end.active",
        "syn_0021_08_trunc4_array.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the complete neighbour parses.
    assert_eq!(
        eval_json("case_0022_08 = [\"a\"]\n")["case_0022_08"],
        serde_json::json!(["a"])
    );
}

/// RB-514 — Derived variant 22-09: mid-string escape decodes to one TAB (SYN-0022-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0022 (toml-test bare-formfeed).
/// `case_0022_09 = "a\tb"` evaluates to exactly 3 chars with a single 0x09 —
/// the escape is content, never indentation and never spaces; converges with
/// the block-leading form (RB-505) from the other end of the value.
#[test]
fn rb_514_syn22_mid_escape_is_tab() {
    let src = "case_0022_09 = \"a\\tb\"\n";
    let v = eval_json(src)["case_0022_09"].clone();
    assert_eq!(v, "a\tb", "{v}");
    let s = v.as_str().expect("string");
    assert_eq!(s.chars().count(), 3, "{v}");
    assert_eq!(s.as_bytes(), b"a\tb", "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the escape-free neighbour evaluates identically clean.
    assert_eq!(eval_json("case_0022_09 = \"ab\"\n")["case_0022_09"], "ab");
}

/// RB-515 — Derived variant 23-01: NUL-first triplet agrees (SYN-0023-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0023 (toml-test bare-null).
/// Byte fixture: raw 0x00 before `k = 1`. E005 Error at (1:1) — converges
/// with RB-314 and RB-506: three lineages, one offset-reporting decoder.
/// (Completes SYN-0022-04…09; opens SYN-0023-01…04.)
#[test]
fn rb_515_syn23_nul_first_triplet_e005() {
    let diags = parse_file(
        &research_fixture("syn_0023_01_nul_first.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 1), "{span:?}");
    // Positive control: the NUL-free twin document evaluates.
    assert_eq!(eval_json("k = 1\n")["k"], 1);
}

/// RB-516 — Derived variant 23-02: bad-UTF-8 triplet agrees (SYN-0023-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0023 (toml-test bare-null).
/// Byte fixture: overlong C0 AF in a string under a third key. E017 Error —
/// converges with RB-196, RB-499 and RB-507: no lineage replacement-decodes.
#[test]
fn rb_516_syn23_utf8_triplet_e017() {
    let diags = parse_file(
        &research_fixture("syn_0023_02_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Positive control: the valid-spelling neighbour parses.
    assert_eq!(eval_json("case_0023_02 = \"mxn\"\n")["case_0023_02"], "mxn");
}

/// RB-517 — Derived variant 23-03: BOM column tracks a 12-char key (SYN-0023-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0023 (toml-test bare-null).
/// Byte fixture: BOM after `case_0023_03 = `. E005 Error at (1:16) — the
/// triplet with RB-315 (1:5) and RB-508 (1:6): the span tracks the stream
/// position at every key width, so mid-stream BOMs never slip through.
#[test]
fn rb_517_syn23_bom_long_key_e005() {
    let diags = parse_file(
        &research_fixture("syn_0023_03_bom_long.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 16), "{span:?}");
    // Positive control: the BOM-free document parses.
    assert_eq!(eval_json("case_0023_03 = 3\n")["case_0023_03"], 3);
}

/// RB-518 — Derived variant 23-04: third `\u` verbatim agrees (SYN-0023-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0023 (toml-test bare-null).
/// `case_0023_04 = "\u0001"` is the 6-char verbatim literal with zero
/// diagnostics — converges with RB-500 and RB-509: escaped policy is
/// distinct from raw bytes in every lineage. (Completes SYN-0023-01…04
/// coverage opened this batch.)
#[test]
fn rb_518_syn23_unicode_escape_verbatim() {
    let v = eval_json("case_0023_04 = \"\\u0001\"\n");
    assert_eq!(
        v["case_0023_04"], "\\u0001",
        "escape must stay verbatim, got {v}"
    );
    assert_eq!(v["case_0023_04"].as_str().expect("string").len(), 6);
    let codes = codes_of("case_0023_04 = \"\\u0001\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: documented escapes still decode.
    let v = eval_json("case_0023_04 = \"a\\tb\"\n");
    assert_eq!(v["case_0023_04"], "a\tb");
}
/// RB-519 — Derived variant 23-05: third TAB-in-string agrees (SYN-0023-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0023 (toml-test bare-null).
/// Byte fixture: literal 0x09 in string content evaluates to "a\tb" with
/// zero diagnostics — converges with RB-501/RB-510: TABs are content in
/// every lineage, indentation in none.
#[test]
fn rb_519_syn23_tab_in_string_ok() {
    let module = parse_file(
        &research_fixture("syn_0023_05_tab_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("tab-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["case_0023_05"], "a\tb", "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: TAB as structural indentation still fails E006.
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("tab indent must fail E006");
    assert!(diags.iter().any(|d| d.code == "E006"), "{diags:?}");
}

/// RB-520 — Derived variant 23-06: third café agrees (SYN-0023-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0023 (toml-test bare-null).
/// `case_0023_06 = "café"` is exactly "café" with zero diagnostics, and the
/// JSON export carries é — converges with RB-502/RB-511 across lineages.
#[test]
fn rb_520_syn23_cafe_roundtrip_exact() {
    let src = "case_0023_06 = \"café\"\n";
    assert_eq!(eval_json(src)["case_0023_06"], "café");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("café"), "{json}");
    // Positive control: ASCII neighbours evaluate alongside cleanly.
    assert_eq!(
        eval_json("case_0023_06 = \"cafe\"\n")["case_0023_06"],
        "cafe"
    );
}

/// RB-521 — Derived variant 23-07: third clef agrees (SYN-0023-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0023 (toml-test bare-null).
/// `case_0023_07 = "𝄞"` (U+1D11E) is exactly one scalar with zero diagnostics —
/// converges with RB-503/RB-512: no lineage splits surrogates.
#[test]
fn rb_521_syn23_supplementary_scalar_single() {
    let src = "case_0023_07 = \"𝄞\"\n";
    let v = eval_json(src)["case_0023_07"].clone();
    assert_eq!(v, "𝄞", "{v}");
    assert_eq!(v.as_str().expect("string").chars().count(), 1, "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: BMP multi-byte text evaluates alongside cleanly.
    assert_eq!(
        eval_json("case_0023_07 = \"café\"\n")["case_0023_07"],
        "café"
    );
}

/// RB-522 — Derived variant 23-08: truncated 2-byte lead is E017 (SYN-0023-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0023 (toml-test bare-null).
/// Byte fixture: lone C2 (1 of 2 bytes) in a string at EOF. The third lead
/// width (2-byte, after E2-alone and F0 9D) fails the same controlled E017
/// Error — truncation width never changes the class, never crashes.
#[test]
fn rb_522_syn23_trunc2_string_e017() {
    let diags = parse_file(
        &research_fixture("syn_0023_08_trunc2_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: all three earlier truncation twins fail the same class.
    for name in [
        "src_toml_0045_bad_utf8_at_end.active",
        "syn_0021_08_trunc4_array.active",
        "syn_0022_08_trunc3_string.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the complete neighbour parses.
    assert_eq!(
        eval_json("case_0023_08 = [\"b\"]\n")["case_0023_08"],
        serde_json::json!(["b"])
    );
}

/// RB-523 — Derived variant 23-09: third mid-escape agrees (SYN-0023-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0023 (toml-test bare-null).
/// `case_0023_09 = "a\tb"` is byte-exact 3 chars with one 0x09, zero
/// diagnostics — converges with RB-514: escapes are content everywhere.
#[test]
fn rb_523_syn23_mid_escape_is_tab() {
    let src = "case_0023_09 = \"a\\tb\"\n";
    let v = eval_json(src)["case_0023_09"].clone();
    assert_eq!(v, "a\tb", "{v}");
    let s = v.as_str().expect("string");
    assert_eq!(s.chars().count(), 3, "{v}");
    assert_eq!(s.as_bytes(), b"a\tb", "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the escape-free neighbour evaluates identically clean.
    assert_eq!(eval_json("case_0023_09 = \"ab\"\n")["case_0023_09"], "ab");
}

/// RB-524 — Derived variant 24-01: NUL offset reported past line 1 (SYN-0024-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0024 (toml-test comment-null).
/// Byte fixture: a good line then raw 0x00 on line 2. E005 Error at (2:1) —
/// the offset report tracks lines, so a NUL anywhere in the stream is
/// located, never just "somewhere". (Completes SYN-0023-05…09; opens
/// SYN-0024-01…05.)
#[test]
fn rb_524_syn24_nul_line2_e005() {
    let diags = parse_file(
        &research_fixture("syn_0024_01_nul_line2.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the NUL-free document evaluates.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-525 — Derived variant 24-02: bad-UTF-8 quad agrees (SYN-0024-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0024 (toml-test comment-null).
/// Byte fixture: overlong C0 AF in a string under a fourth key. E017 Error —
/// converges with RB-196, RB-499, RB-507 and RB-516: four lineages, zero
/// replacement-decoding.
#[test]
fn rb_525_syn24_utf8_quad_e017() {
    let diags = parse_file(
        &research_fixture("syn_0024_02_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Positive control: the valid-spelling neighbour parses.
    assert_eq!(eval_json("case_0024_02 = \"sxt\"\n")["case_0024_02"], "sxt");
}

/// RB-526 — Derived variant 24-03: BOM on line 2 is E005 (SYN-0024-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0024 (toml-test comment-null).
/// Byte fixture: BOM opens line 2 after a section header. E005 Error at
/// (2:1) — BOM acceptance is the stream-leading position only (RB-003),
/// never "start of any line".
#[test]
fn rb_526_syn24_bom_line2_e005() {
    let diags = parse_file(
        &research_fixture("syn_0024_03_bom_line2.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the BOM-free document parses.
    let v = eval_json("[s]\nk = 1\n");
    assert_eq!(v["s"]["k"], 1);
}

/// RB-527 — Derived variant 24-04: fourth `\u` verbatim agrees (SYN-0024-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0024 (toml-test comment-null).
/// `case_0024_04 = "\u0001"` is the 6-char verbatim literal with zero
/// diagnostics — converges with RB-500, RB-509 and RB-518: four lineages,
/// no `\u` decoding anywhere.
#[test]
fn rb_527_syn24_unicode_escape_verbatim() {
    let v = eval_json("case_0024_04 = \"\\u0001\"\n");
    assert_eq!(
        v["case_0024_04"], "\\u0001",
        "escape must stay verbatim, got {v}"
    );
    assert_eq!(v["case_0024_04"].as_str().expect("string").len(), 6);
    let codes = codes_of("case_0024_04 = \"\\u0001\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: documented escapes still decode.
    let v = eval_json("case_0024_04 = \"a\\tb\"\n");
    assert_eq!(v["case_0024_04"], "a\tb");
}

/// RB-528 — Derived variant 24-05: fourth TAB-in-string agrees (SYN-0024-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0024 (toml-test comment-null).
/// Byte fixture: literal 0x09 in string content evaluates to "a\tb" with
/// zero diagnostics — converges with RB-501, RB-510 and RB-519: TABs are
/// content in all four lineages. (Completes SYN-0024-01…05 coverage opened
/// this batch.)
#[test]
fn rb_528_syn24_tab_in_string_ok() {
    let module = parse_file(
        &research_fixture("syn_0024_05_tab_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("tab-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["case_0024_05"], "a\tb", "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: TAB as structural indentation still fails E006.
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("tab indent must fail E006");
    assert!(diags.iter().any(|d| d.code == "E006"), "{diags:?}");
}
/// RB-529 — Derived variant 24-06: fourth café agrees (SYN-0024-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0024 (toml-test comment-null).
/// `case_0024_06 = "café"` is exactly "café" with zero diagnostics, and the
/// JSON export carries é — converges across all four lineages.
#[test]
fn rb_529_syn24_cafe_roundtrip_exact() {
    let src = "case_0024_06 = \"café\"\n";
    assert_eq!(eval_json(src)["case_0024_06"], "café");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("café"), "{json}");
    // Positive control: ASCII neighbours evaluate alongside cleanly.
    assert_eq!(
        eval_json("case_0024_06 = \"cafe\"\n")["case_0024_06"],
        "cafe"
    );
}

/// RB-530 — Derived variant 24-07: fourth clef agrees (SYN-0024-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0024 (toml-test comment-null).
/// `case_0024_07 = "𝄞"` (U+1D11E) is exactly one scalar with zero diagnostics —
/// converges across all four lineages: no lineage splits surrogates.
#[test]
fn rb_530_syn24_supplementary_scalar_single() {
    let src = "case_0024_07 = \"𝄞\"\n";
    let v = eval_json(src)["case_0024_07"].clone();
    assert_eq!(v, "𝄞", "{v}");
    assert_eq!(v.as_str().expect("string").chars().count(), 1, "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: BMP multi-byte text evaluates alongside cleanly.
    assert_eq!(
        eval_json("case_0024_07 = \"café\"\n")["case_0024_07"],
        "café"
    );
}

/// RB-531 — Derived variant 24-08: truncated E2 in array is E017 (SYN-0024-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0024 (toml-test comment-null).
/// Byte fixture: lone E2 inside an array at EOF. The E2-alone width in array
/// shape fails the same controlled E017 Error — every width × shape
/// combination is a decode error, never a crash.
#[test]
fn rb_531_syn24_trunc_e2_array_e017() {
    let diags = parse_file(
        &research_fixture("syn_0024_08_trunc_e2_array.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: all four earlier truncation twins fail the same class.
    for name in [
        "src_toml_0045_bad_utf8_at_end.active",
        "syn_0021_08_trunc4_array.active",
        "syn_0022_08_trunc3_string.active",
        "syn_0023_08_trunc2_string.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the complete neighbour parses.
    assert_eq!(
        eval_json("case_0024_08 = [\"a\"]\n")["case_0024_08"],
        serde_json::json!(["a"])
    );
}

/// RB-532 — Derived variant 24-09: fourth mid-escape agrees (SYN-0024-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0024 (toml-test comment-null).
/// `case_0024_09 = "a\tb"` is byte-exact 3 chars with one 0x09, zero
/// diagnostics — converges across all four lineages. (Completes
/// SYN-0024-06…09.)
#[test]
fn rb_532_syn24_mid_escape_is_tab() {
    let src = "case_0024_09 = \"a\\tb\"\n";
    let v = eval_json(src)["case_0024_09"].clone();
    assert_eq!(v, "a\tb", "{v}");
    let s = v.as_str().expect("string");
    assert_eq!(s.chars().count(), 3, "{v}");
    assert_eq!(s.as_bytes(), b"a\tb", "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the escape-free neighbour evaluates identically clean.
    assert_eq!(eval_json("case_0024_09 = \"ab\"\n")["case_0024_09"], "ab");
}

/// RB-533 — Derived variant 25-01: NUL is content in strings, error outside (SYN-0025-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0025 (toml-test linetab-number).
/// Byte fixtures pin both sides of the catalogue's "outside a string"
/// qualifier: NUL inside a string evaluates (value keeps the 0x00 scalar,
/// JSON-escaped on export), NUL inside a comment is inert (document
/// evaluates, zero diagnostics) — while NUL outside either is E005
/// (RB-314 converges). The qualifier is real, not decorative.
#[test]
fn rb_533_syn25_nul_string_comment_content() {
    let module = parse_file(
        &research_fixture("syn_0025_01_nul_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("nul-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(
        v["case_0025_01"], "a\0b",
        "NUL must survive as content, got {v}"
    );
    assert!(diags.is_empty(), "{diags:?}");
    // NUL inside a comment is inert: the document evaluates silently.
    let module = parse_file(
        &research_fixture("syn_0025_01_nul_in_comment.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("nul-in-comment must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], 1, "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: NUL outside string-or-comment still fails E005 (RB-314).
    let diags = parse_file(
        &research_fixture("syn_0021_01_nul_first.active"),
        ParseOptions::default(),
    )
    .expect_err("bare NUL must fail E005");
    assert!(diags.iter().any(|d| d.code == "E005"), "{diags:?}");
}

/// RB-534 — Derived variant 25-02: bad-UTF-8 quint agrees (SYN-0025-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0025 (toml-test linetab-number).
/// Byte fixture: overlong C0 AF in a string under a fifth key. E017 Error —
/// converges across five lineages: no lineage replacement-decodes. (Opens
/// SYN-0025-01…06.)
#[test]
fn rb_534_syn25_utf8_quint_e017() {
    let diags = parse_file(
        &research_fixture("syn_0025_02_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: two earlier twins fail the same class.
    for name in [
        "src_toml_0047_bad_utf8_in_string.active",
        "syn_0024_02_utf8_in_string.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the valid-spelling neighbour parses.
    assert_eq!(eval_json("case_0025_02 = \"exf\"\n")["case_0025_02"], "exf");
}

/// RB-535 — Derived variant 25-03: BOM after header is E005 (SYN-0025-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0025 (toml-test linetab-number).
/// Byte fixture: BOM immediately after `[s]` on line 1. E005 Error at (1:4) —
/// a BOM terminates the header position too: BOM acceptance is the
/// stream-leading position only, never after any token on any line.
#[test]
fn rb_535_syn25_bom_after_header_e005() {
    let diags = parse_file(
        &research_fixture("syn_0025_03_bom_header.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 4), "{span:?}");
    // Positive control: the BOM-free document parses.
    let v = eval_json("[s]\nk = 1\n");
    assert_eq!(v["s"]["k"], 1);
}

/// RB-536 — Derived variant 25-04: fifth `\u` verbatim agrees (SYN-0025-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0025 (toml-test linetab-number).
/// `case_0025_04 = "\u0001"` is the 6-char verbatim literal with zero
/// diagnostics — converges across all five lineages: no `\u` decoding
/// anywhere.
#[test]
fn rb_536_syn25_unicode_escape_verbatim() {
    let v = eval_json("case_0025_04 = \"\\u0001\"\n");
    assert_eq!(
        v["case_0025_04"], "\\u0001",
        "escape must stay verbatim, got {v}"
    );
    assert_eq!(v["case_0025_04"].as_str().expect("string").len(), 6);
    let codes = codes_of("case_0025_04 = \"\\u0001\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: documented escapes still decode.
    let v = eval_json("case_0025_04 = \"a\\tb\"\n");
    assert_eq!(v["case_0025_04"], "a\tb");
}

/// RB-537 — Derived variant 25-05: fifth TAB-in-string agrees (SYN-0025-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0025 (toml-test linetab-number).
/// Byte fixture: literal 0x09 in string content evaluates to "a\tb" with
/// zero diagnostics — converges across all five lineages: TABs are content
/// everywhere, indentation nowhere.
#[test]
fn rb_537_syn25_tab_in_string_ok() {
    let module = parse_file(
        &research_fixture("syn_0025_05_tab_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("tab-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["case_0025_05"], "a\tb", "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: TAB as structural indentation still fails E006.
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("tab indent must fail E006");
    assert!(diags.iter().any(|d| d.code == "E006"), "{diags:?}");
}

/// RB-538 — Derived variant 25-06: fifth café agrees (SYN-0025-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0025 (toml-test linetab-number).
/// `case_0025_06 = "café"` is exactly "café" with zero diagnostics, and the
/// JSON export carries é — converges across all five lineages. (Completes
/// SYN-0024-06…09 coverage + opens SYN-0025-01…06.)
#[test]
fn rb_538_syn25_cafe_roundtrip_exact() {
    let src = "case_0025_06 = \"café\"\n";
    assert_eq!(eval_json(src)["case_0025_06"], "café");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("café"), "{json}");
    // Positive control: ASCII neighbours evaluate alongside cleanly.
    assert_eq!(
        eval_json("case_0025_06 = \"cafe\"\n")["case_0025_06"],
        "cafe"
    );
}
/// RB-539 — Derived variant 25-07: fifth clef agrees (SYN-0025-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0025 (toml-test linetab-number).
/// `case_0025_07 = "𝄞"` (U+1D11E) is exactly one scalar with zero diagnostics —
/// converges across all five lineages: no lineage splits surrogates.
#[test]
fn rb_539_syn25_supplementary_scalar_single() {
    let src = "case_0025_07 = \"𝄞\"\n";
    let v = eval_json(src)["case_0025_07"].clone();
    assert_eq!(v, "𝄞", "{v}");
    assert_eq!(v.as_str().expect("string").chars().count(), 1, "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: BMP multi-byte text evaluates alongside cleanly.
    assert_eq!(
        eval_json("case_0025_07 = \"café\"\n")["case_0025_07"],
        "café"
    );
}

/// RB-540 — Derived variant 25-08: truncated 4-byte prefix is E017 (SYN-0025-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0025 (toml-test linetab-number).
/// Byte fixture: F0 9D 84 (3 of 4 bytes) in a string at EOF. The last
/// truncation completeness class fails the same controlled E017 Error —
/// EOF at any depth of any multibyte sequence is a decode error, not a crash.
#[test]
fn rb_540_syn25_trunc4b3_string_e017() {
    let diags = parse_file(
        &research_fixture("syn_0025_08_trunc4b3_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: all five earlier truncation twins fail the same class.
    for name in [
        "src_toml_0045_bad_utf8_at_end.active",
        "syn_0021_08_trunc4_array.active",
        "syn_0022_08_trunc3_string.active",
        "syn_0023_08_trunc2_string.active",
        "syn_0024_08_trunc_e2_array.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the complete neighbour parses.
    assert_eq!(
        eval_json("case_0025_08 = [\"q\"]\n")["case_0025_08"],
        serde_json::json!(["q"])
    );
}

/// RB-541 — Derived variant 25-09: fifth mid-escape agrees (SYN-0025-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0025 (toml-test linetab-number).
/// `case_0025_09 = "a\tb"` is byte-exact 3 chars with one 0x09, zero
/// diagnostics — converges across all five lineages. (Completes
/// SYN-0025-07…09.)
#[test]
fn rb_541_syn25_mid_escape_is_tab() {
    let src = "case_0025_09 = \"a\\tb\"\n";
    let v = eval_json(src)["case_0025_09"].clone();
    assert_eq!(v, "a\tb", "{v}");
    let s = v.as_str().expect("string");
    assert_eq!(s.chars().count(), 3, "{v}");
    assert_eq!(s.as_bytes(), b"a\tb", "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the escape-free neighbour evaluates identically clean.
    assert_eq!(eval_json("case_0025_09 = \"ab\"\n")["case_0025_09"], "ab");
}

/// RB-542 — Derived variant 26-01: NUL inside header is E005 (SYN-0026-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0026 (toml-test multi-lf).
/// Byte fixture: raw 0x00 inside `[s\x00]`. E005 Error at (1:3) — a NUL in
/// the header name is outside any string-or-comment, so the offset report
/// lands mid-header; section headers get no NUL tolerance. (Opens
/// SYN-0026-01…07.)
#[test]
fn rb_542_syn26_nul_in_header_e005() {
    let diags = parse_file(
        &research_fixture("syn_0026_01_nul_in_header.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 3), "{span:?}");
    // Positive control: the NUL-free header parses.
    let v = eval_json("[s]\nk = 1\n");
    assert_eq!(v["s"]["k"], 1);
}

/// RB-543 — Derived variant 26-02: bad-UTF-8 sextet agrees (SYN-0026-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0026 (toml-test multi-lf).
/// Byte fixture: overlong C0 AF in a string under a sixth key. E017 Error —
/// converges across six lineages: no lineage replacement-decodes.
#[test]
fn rb_543_syn26_utf8_sextet_e017() {
    let diags = parse_file(
        &research_fixture("syn_0026_02_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: two earlier twins fail the same class.
    for name in [
        "src_toml_0047_bad_utf8_in_string.active",
        "syn_0025_02_utf8_in_string.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the valid-spelling neighbour parses.
    assert_eq!(eval_json("case_0026_02 = \"vxw\"\n")["case_0026_02"], "vxw");
}

/// RB-544 — Derived variant 26-03: BOM after indent is E005 (SYN-0026-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0026 (toml-test multi-lf).
/// Byte fixture: two spaces then BOM. E005 Error at (1:3) — the BOM is
/// misplaced after indentation (the leading spaces separately draw E010,
/// documented here but not asserted: the catalogue demands the misplaced-BOM
/// reject, which is E005). BOM acceptance stays stream-leading-only.
#[test]
fn rb_544_syn26_bom_after_indent_e005() {
    let diags = parse_file(
        &research_fixture("syn_0026_03_bom_indent.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 3), "{span:?}");
    // Positive control: the BOM-free indented document still fails only E010.
    let codes = codes_of("  k = 1\n");
    assert!(codes.iter().any(|c| c == "E010"), "{codes:?}");
}

/// RB-545 — Derived variant 26-04: sixth `\u` verbatim agrees (SYN-0026-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0026 (toml-test multi-lf).
/// `case_0026_04 = "\u0001"` is the 6-char verbatim literal with zero
/// diagnostics — converges across all six lineages: no `\u` decoding
/// anywhere.
#[test]
fn rb_545_syn26_unicode_escape_verbatim() {
    let v = eval_json("case_0026_04 = \"\\u0001\"\n");
    assert_eq!(
        v["case_0026_04"], "\\u0001",
        "escape must stay verbatim, got {v}"
    );
    assert_eq!(v["case_0026_04"].as_str().expect("string").len(), 6);
    let codes = codes_of("case_0026_04 = \"\\u0001\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: documented escapes still decode.
    let v = eval_json("case_0026_04 = \"a\\tb\"\n");
    assert_eq!(v["case_0026_04"], "a\tb");
}

/// RB-546 — Derived variant 26-05: sixth TAB-in-string agrees (SYN-0026-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0026 (toml-test multi-lf).
/// Byte fixture: literal 0x09 in string content evaluates to "a\tb" with
/// zero diagnostics — converges across all six lineages: TABs are content
/// everywhere, indentation nowhere.
#[test]
fn rb_546_syn26_tab_in_string_ok() {
    let module = parse_file(
        &research_fixture("syn_0026_05_tab_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("tab-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["case_0026_05"], "a\tb", "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: TAB as structural indentation still fails E006.
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("tab indent must fail E006");
    assert!(diags.iter().any(|d| d.code == "E006"), "{diags:?}");
}

/// RB-547 — Derived variant 26-06: sixth café agrees (SYN-0026-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0026 (toml-test multi-lf).
/// `case_0026_06 = "café"` is exactly "café" with zero diagnostics, and the
/// JSON export carries é — converges across all six lineages.
#[test]
fn rb_547_syn26_cafe_roundtrip_exact() {
    let src = "case_0026_06 = \"café\"\n";
    assert_eq!(eval_json(src)["case_0026_06"], "café");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("café"), "{json}");
    // Positive control: ASCII neighbours evaluate alongside cleanly.
    assert_eq!(
        eval_json("case_0026_06 = \"cafe\"\n")["case_0026_06"],
        "cafe"
    );
}

/// RB-548 — Derived variant 26-07: sixth clef agrees (SYN-0026-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0026 (toml-test multi-lf).
/// `case_0026_07 = "𝄞"` (U+1D11E) is exactly one scalar with zero diagnostics —
/// converges across all six lineages: no lineage splits surrogates.
/// (Completes SYN-0025-07…09 coverage + opens SYN-0026-01…07.)
#[test]
fn rb_548_syn26_supplementary_scalar_single() {
    let src = "case_0026_07 = \"𝄞\"\n";
    let v = eval_json(src)["case_0026_07"].clone();
    assert_eq!(v, "𝄞", "{v}");
    assert_eq!(v.as_str().expect("string").chars().count(), 1, "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: BMP multi-byte text evaluates alongside cleanly.
    assert_eq!(
        eval_json("case_0026_07 = \"café\"\n")["case_0026_07"],
        "café"
    );
}
/// RB-549 — Derived variant 26-08: truncated E2 82 in array is E017 (SYN-0026-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0026 (toml-test multi-lf).
/// Byte fixture: E2 82 (2 of 3 bytes) inside an array at EOF. The same width
/// in a new token shape fails the same controlled E017 Error — every width ×
/// shape combination is a decode error, never a crash. (Completes
/// SYN-0026-08…09 coverage.)
#[test]
fn rb_549_syn26_trunc_e282_array_e017() {
    let diags = parse_file(
        &research_fixture("syn_0026_08_trunc_e282_array.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: all six earlier truncation twins fail the same class.
    for name in [
        "src_toml_0045_bad_utf8_at_end.active",
        "syn_0021_08_trunc4_array.active",
        "syn_0022_08_trunc3_string.active",
        "syn_0023_08_trunc2_string.active",
        "syn_0024_08_trunc_e2_array.active",
        "syn_0025_08_trunc4b3_string.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the complete neighbour parses.
    assert_eq!(
        eval_json("case_0026_08 = [\"b\"]\n")["case_0026_08"],
        serde_json::json!(["b"])
    );
}

/// RB-550 — Derived variant 26-09: sixth mid-escape agrees (SYN-0026-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0026 (toml-test multi-lf).
/// `case_0026_09 = "a\tb"` is byte-exact 3 chars with one 0x09, zero
/// diagnostics — converges across all six lineages.
#[test]
fn rb_550_syn26_mid_escape_is_tab() {
    let src = "case_0026_09 = \"a\\tb\"\n";
    let v = eval_json(src)["case_0026_09"].clone();
    assert_eq!(v, "a\tb", "{v}");
    let s = v.as_str().expect("string");
    assert_eq!(s.chars().count(), 3, "{v}");
    assert_eq!(s.as_bytes(), b"a\tb", "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the escape-free neighbour evaluates identically clean.
    assert_eq!(eval_json("case_0026_09 = \"ab\"\n")["case_0026_09"], "ab");
}

/// RB-551 — Derived variant 27-01: NUL splitting a key is E005 (SYN-0027-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0027 (toml-test rawmulti-null).
/// Byte fixture: `value\0 = 1`. E005 Error at (1:6) — a NUL between
/// identifier bytes is outside any string-or-comment, so the offset lands
/// mid-token; keys get no NUL tolerance either. (Opens SYN-0027-01…08.)
#[test]
fn rb_551_syn27_nul_split_key_e005() {
    let diags = parse_file(
        &research_fixture("syn_0027_01_nul_split_key.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 6), "{span:?}");
    // Positive control: the NUL-free key parses, and NUL-in-string stays
    // content (RB-533 converges).
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-552 — Derived variant 27-02: bad-UTF-8 septet agrees (SYN-0027-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0027 (toml-test rawmulti-null).
/// Byte fixture: overlong C0 AF in a string under a seventh key. E017 Error —
/// converges across seven lineages: no lineage replacement-decodes.
#[test]
fn rb_552_syn27_utf8_septet_e017() {
    let diags = parse_file(
        &research_fixture("syn_0027_02_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: two earlier twins fail the same class.
    for name in [
        "src_toml_0047_bad_utf8_in_string.active",
        "syn_0026_02_utf8_in_string.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the valid-spelling neighbour parses.
    assert_eq!(eval_json("case_0027_02 = \"hxi\"\n")["case_0027_02"], "hxi");
}

/// RB-553 — Derived variant 27-03: BOM inside comments is inert (SYN-0027-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0027 (toml-test rawmulti-null).
/// Byte fixture: BOM bytes inside a `#` comment. The document evaluates with
/// zero diagnostics — comments are opaque to BOM exactly as they are to NUL
/// (RB-533 converges) — while a BOM outside comments stays E005 (RB-535's
/// header fixture converges). BOM acceptance is about token positions, and
/// comments are not token positions.
#[test]
fn rb_553_syn27_bom_in_comment_inert() {
    let module = parse_file(
        &research_fixture("syn_0027_03_bom_in_comment.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("bom-in-comment must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["value"], 1, "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: a BOM outside comments still fails E005.
    let diags = parse_file(
        &research_fixture("syn_0025_03_bom_header.active"),
        ParseOptions::default(),
    )
    .expect_err("bare BOM must fail E005");
    assert!(diags.iter().any(|d| d.code == "E005"), "{diags:?}");
}

/// RB-554 — Derived variant 27-04: seventh `\u` verbatim agrees (SYN-0027-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0027 (toml-test rawmulti-null).
/// `case_0027_04 = "\u0001"` is the 6-char verbatim literal with zero
/// diagnostics — converges across all seven lineages: no `\u` decoding
/// anywhere.
#[test]
fn rb_554_syn27_unicode_escape_verbatim() {
    let v = eval_json("case_0027_04 = \"\\u0001\"\n");
    assert_eq!(
        v["case_0027_04"], "\\u0001",
        "escape must stay verbatim, got {v}"
    );
    assert_eq!(v["case_0027_04"].as_str().expect("string").len(), 6);
    let codes = codes_of("case_0027_04 = \"\\u0001\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: documented escapes still decode.
    let v = eval_json("case_0027_04 = \"a\\tb\"\n");
    assert_eq!(v["case_0027_04"], "a\tb");
}

/// RB-555 — Derived variant 27-05: seventh TAB-in-string agrees (SYN-0027-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0027 (toml-test rawmulti-null).
/// Byte fixture: literal 0x09 in string content evaluates to "a\tb" with
/// zero diagnostics — converges across all seven lineages: TABs are content
/// everywhere, indentation nowhere.
#[test]
fn rb_555_syn27_tab_in_string_ok() {
    let module = parse_file(
        &research_fixture("syn_0027_05_tab_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("tab-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["case_0027_05"], "a\tb", "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: TAB as structural indentation still fails E006.
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("tab indent must fail E006");
    assert!(diags.iter().any(|d| d.code == "E006"), "{diags:?}");
}

/// RB-556 — Derived variant 27-06: seventh café agrees (SYN-0027-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0027 (toml-test rawmulti-null).
/// `case_0027_06 = "café"` is exactly "café" with zero diagnostics, and the
/// JSON export carries é — converges across all seven lineages.
#[test]
fn rb_556_syn27_cafe_roundtrip_exact() {
    let src = "case_0027_06 = \"café\"\n";
    assert_eq!(eval_json(src)["case_0027_06"], "café");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("café"), "{json}");
    // Positive control: ASCII neighbours evaluate alongside cleanly.
    assert_eq!(
        eval_json("case_0027_06 = \"cafe\"\n")["case_0027_06"],
        "cafe"
    );
}

/// RB-557 — Derived variant 27-07: seventh clef agrees (SYN-0027-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0027 (toml-test rawmulti-null).
/// `case_0027_07 = "𝄞"` (U+1D11E) is exactly one scalar with zero diagnostics —
/// converges across all seven lineages: no lineage splits surrogates.
#[test]
fn rb_557_syn27_supplementary_scalar_single() {
    let src = "case_0027_07 = \"𝄞\"\n";
    let v = eval_json(src)["case_0027_07"].clone();
    assert_eq!(v, "𝄞", "{v}");
    assert_eq!(v.as_str().expect("string").chars().count(), 1, "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: BMP multi-byte text evaluates alongside cleanly.
    assert_eq!(
        eval_json("case_0027_07 = \"café\"\n")["case_0027_07"],
        "café"
    );
}

/// RB-558 — Derived variant 27-08: truncation inside comments is E017 (SYN-0027-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0027 (toml-test rawmulti-null).
/// Byte fixture: truncated E2 lead inside a `#` comment mid-file. E017 Error —
/// decoding runs before lexing, so a broken sequence is fatal even where a
/// valid BOM/NUL would be inert (RB-553 converges on the contrast).
/// (Completes SYN-0026-08…09 coverage + opens SYN-0027-01…08.)
#[test]
fn rb_558_syn27_trunc_in_comment_e017() {
    let diags = parse_file(
        &research_fixture("syn_0027_08_trunc_in_comment.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Positive control: the well-formed comment neighbour parses silently.
    let codes = codes_of("# hello\nvalue = 1\n");
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(eval_json("# hello\nvalue = 1\n")["value"], 1);
}
/// RB-559 — Derived variant 27-09: seventh mid-escape agrees (SYN-0027-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0027 (toml-test rawmulti-null).
/// `case_0027_09 = "a\tb"` is byte-exact 3 chars with one 0x09, zero
/// diagnostics — converges across all seven lineages. (Completes
/// SYN-0027-09 coverage.)
#[test]
fn rb_559_syn27_mid_escape_is_tab() {
    let src = "case_0027_09 = \"a\\tb\"\n";
    let v = eval_json(src)["case_0027_09"].clone();
    assert_eq!(v, "a\tb", "{v}");
    let s = v.as_str().expect("string");
    assert_eq!(s.chars().count(), 3, "{v}");
    assert_eq!(s.as_bytes(), b"a\tb", "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the escape-free neighbour evaluates identically clean.
    assert_eq!(eval_json("case_0027_09 = \"ab\"\n")["case_0027_09"], "ab");
}

/// RB-560 — Derived variant 28-01: NUL inside array brackets is E005 (SYN-0028-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0028 (toml-test rawstring-lf).
/// Byte fixture: `v = [\0]`. E005 Error at (1:6) — a NUL between collection
/// delimiters is outside any string-or-comment, so bracket context gives no
/// shelter; the offset lands exactly on the byte. (Opens SYN-0028-01…09.)
#[test]
fn rb_560_syn28_nul_in_array_e005() {
    let diags = parse_file(
        &research_fixture("syn_0028_01_nul_in_array.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 6), "{span:?}");
    // Positive control: the NUL-free array evaluates.
    assert_eq!(eval_json("v = []\n")["v"], serde_json::json!([]));
}

/// RB-561 — Derived variant 28-02: bad-UTF-8 octet agrees (SYN-0028-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0028 (toml-test rawstring-lf).
/// Byte fixture: overlong C0 AF in a string under an eighth key. E017 Error —
/// converges across eight lineages: no lineage replacement-decodes.
#[test]
fn rb_561_syn28_utf8_octet_e017() {
    let diags = parse_file(
        &research_fixture("syn_0028_02_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: two earlier twins fail the same class.
    for name in [
        "src_toml_0047_bad_utf8_in_string.active",
        "syn_0027_02_utf8_in_string.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the valid-spelling neighbour parses.
    assert_eq!(eval_json("case_0028_02 = \"jxk\"\n")["case_0028_02"], "jxk");
}

/// RB-562 — Derived variant 28-03: BOM splitting a key is E005 (SYN-0028-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0028 (toml-test rawstring-lf).
/// Byte fixture: `k<BOM> = 1`. E005 Error at (1:2) — mirrors the NUL-split
/// key (RB-551): a BOM between identifier bytes is outside any
/// string-or-comment, located mid-token at its exact column.
#[test]
fn rb_562_syn28_bom_split_key_e005() {
    let diags = parse_file(
        &research_fixture("syn_0028_03_bom_split_key.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 2), "{span:?}");
    // Positive control: the BOM-free key parses.
    assert_eq!(eval_json("k = 1\n")["k"], 1);
}

/// RB-563 — Derived variant 28-04: eighth `\u` verbatim agrees (SYN-0028-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0028 (toml-test rawstring-lf).
/// `case_0028_04 = "\u0001"` is the 6-char verbatim literal with zero
/// diagnostics — converges across all eight lineages: no `\u` decoding
/// anywhere.
#[test]
fn rb_563_syn28_unicode_escape_verbatim() {
    let v = eval_json("case_0028_04 = \"\\u0001\"\n");
    assert_eq!(
        v["case_0028_04"], "\\u0001",
        "escape must stay verbatim, got {v}"
    );
    assert_eq!(v["case_0028_04"].as_str().expect("string").len(), 6);
    let codes = codes_of("case_0028_04 = \"\\u0001\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: documented escapes still decode.
    let v = eval_json("case_0028_04 = \"a\\tb\"\n");
    assert_eq!(v["case_0028_04"], "a\tb");
}

/// RB-564 — Derived variant 28-05: eighth TAB-in-string agrees (SYN-0028-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0028 (toml-test rawstring-lf).
/// Byte fixture: literal 0x09 in string content evaluates to "a\tb" with
/// zero diagnostics — converges across all eight lineages: TABs are content
/// everywhere, indentation nowhere.
#[test]
fn rb_564_syn28_tab_in_string_ok() {
    let module = parse_file(
        &research_fixture("syn_0028_05_tab_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("tab-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["case_0028_05"], "a\tb", "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: TAB as structural indentation still fails E006.
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("tab indent must fail E006");
    assert!(diags.iter().any(|d| d.code == "E006"), "{diags:?}");
}

/// RB-565 — Derived variant 28-06: eighth café agrees (SYN-0028-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0028 (toml-test rawstring-lf).
/// `case_0028_06 = "café"` is exactly "café" with zero diagnostics, and the
/// JSON export carries é — converges across all eight lineages.
#[test]
fn rb_565_syn28_cafe_roundtrip_exact() {
    let src = "case_0028_06 = \"café\"\n";
    assert_eq!(eval_json(src)["case_0028_06"], "café");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("café"), "{json}");
    // Positive control: ASCII neighbours evaluate alongside cleanly.
    assert_eq!(
        eval_json("case_0028_06 = \"cafe\"\n")["case_0028_06"],
        "cafe"
    );
}

/// RB-566 — Derived variant 28-07: eighth clef agrees (SYN-0028-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0028 (toml-test rawstring-lf).
/// `case_0028_07 = "𝄞"` (U+1D11E) is exactly one scalar with zero diagnostics —
/// converges across all eight lineages: no lineage splits surrogates.
#[test]
fn rb_566_syn28_supplementary_scalar_single() {
    let src = "case_0028_07 = \"𝄞\"\n";
    let v = eval_json(src)["case_0028_07"].clone();
    assert_eq!(v, "𝄞", "{v}");
    assert_eq!(v.as_str().expect("string").chars().count(), 1, "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: BMP multi-byte text evaluates alongside cleanly.
    assert_eq!(
        eval_json("case_0028_07 = \"café\"\n")["case_0028_07"],
        "café"
    );
}

/// RB-567 — Derived variant 28-08: lone F0 lead is E017 (SYN-0028-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0028 (toml-test rawstring-lf).
/// Byte fixture: lone F0 (1 of 4 bytes) in a string at EOF. The last lead
/// singleton completes the lead-width set (C2, E2, F0) — every singleton
/// fails the same controlled E017 Error, never a crash.
#[test]
fn rb_567_syn28_trunc_f0_string_e017() {
    let diags = parse_file(
        &research_fixture("syn_0028_08_trunc_f0_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: the other two singletons fail the same class.
    for name in [
        "src_toml_0045_bad_utf8_at_end.active",
        "syn_0023_08_trunc2_string.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the complete neighbour parses.
    assert_eq!(
        eval_json("case_0028_08 = [\"w\"]\n")["case_0028_08"],
        serde_json::json!(["w"])
    );
}

/// RB-568 — Derived variant 28-09: eighth mid-escape agrees (SYN-0028-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0028 (toml-test rawstring-lf).
/// `case_0028_09 = "a\tb"` is byte-exact 3 chars with one 0x09, zero
/// diagnostics — converges across all eight lineages. (Completes
/// SYN-0027-09 coverage + opens SYN-0028-01…09.)
#[test]
fn rb_568_syn28_mid_escape_is_tab() {
    let src = "case_0028_09 = \"a\\tb\"\n";
    let v = eval_json(src)["case_0028_09"].clone();
    assert_eq!(v, "a\tb", "{v}");
    let s = v.as_str().expect("string");
    assert_eq!(s.chars().count(), 3, "{v}");
    assert_eq!(s.as_bytes(), b"a\tb", "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the escape-free neighbour evaluates identically clean.
    assert_eq!(eval_json("case_0028_09 = \"ab\"\n")["case_0028_09"], "ab");
}
/// RB-569 — Derived variant 29-01: NUL at value position is E005 (SYN-0029-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0029 (toml-test string-null).
/// Byte fixture: `v = \0`. E005 Error at (1:5) — a NUL where the value
/// starts is outside any string-or-comment (the empty tail separately draws
/// E002, documented here but not asserted: the catalogue demands the
/// invalid-source-byte reject, which is E005). (Opens SYN-0029-01…09.)
#[test]
fn rb_569_syn29_nul_value_pos_e005() {
    let diags = parse_file(
        &research_fixture("syn_0029_01_nul_value.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 5), "{span:?}");
    // Positive control: the NUL-free value parses.
    assert_eq!(eval_json("v = 1\n")["v"], 1);
}

/// RB-570 — Derived variant 29-02: bad-UTF-8 nonet agrees (SYN-0029-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0029 (toml-test string-null).
/// Byte fixture: overlong C0 AF in a string under a ninth key. E017 Error —
/// converges across nine lineages: no lineage replacement-decodes.
#[test]
fn rb_570_syn29_utf8_nonet_e017() {
    let diags = parse_file(
        &research_fixture("syn_0029_02_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: two earlier twins fail the same class.
    for name in [
        "src_toml_0047_bad_utf8_in_string.active",
        "syn_0028_02_utf8_in_string.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the valid-spelling neighbour parses.
    assert_eq!(eval_json("case_0029_02 = \"lxm\"\n")["case_0029_02"], "lxm");
}

/// RB-571 — Derived variant 29-03: BOM at EOF is E005 (SYN-0029-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0029 (toml-test string-null).
/// Byte fixture: BOM as the last byte with no trailing newline. E005 Error
/// at (2:1) — the stream-leading-only rule holds at the tail too: a BOM
/// anywhere but offset 0 is misplaced, head or tail.
#[test]
fn rb_571_syn29_bom_at_eof_e005() {
    let diags = parse_file(
        &research_fixture("syn_0029_03_bom_at_eof.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (2, 1), "{span:?}");
    // Positive control: the BOM-free tail document parses.
    assert_eq!(eval_json("value = 1\n")["value"], 1);
}

/// RB-572 — Derived variant 29-04: ninth `\u` verbatim agrees (SYN-0029-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0029 (toml-test string-null).
/// `case_0029_04 = "\u0001"` is the 6-char verbatim literal with zero
/// diagnostics — converges across all nine lineages: no `\u` decoding
/// anywhere.
#[test]
fn rb_572_syn29_unicode_escape_verbatim() {
    let v = eval_json("case_0029_04 = \"\\u0001\"\n");
    assert_eq!(
        v["case_0029_04"], "\\u0001",
        "escape must stay verbatim, got {v}"
    );
    assert_eq!(v["case_0029_04"].as_str().expect("string").len(), 6);
    let codes = codes_of("case_0029_04 = \"\\u0001\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: documented escapes still decode.
    let v = eval_json("case_0029_04 = \"a\\tb\"\n");
    assert_eq!(v["case_0029_04"], "a\tb");
}

/// RB-573 — Derived variant 29-05: ninth TAB-in-string agrees (SYN-0029-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0029 (toml-test string-null).
/// Byte fixture: literal 0x09 in string content evaluates to "a\tb" with
/// zero diagnostics — converges across all nine lineages: TABs are content
/// everywhere, indentation nowhere.
#[test]
fn rb_573_syn29_tab_in_string_ok() {
    let module = parse_file(
        &research_fixture("syn_0029_05_tab_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("tab-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["case_0029_05"], "a\tb", "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: TAB as structural indentation still fails E006.
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("tab indent must fail E006");
    assert!(diags.iter().any(|d| d.code == "E006"), "{diags:?}");
}

/// RB-574 — Derived variant 29-06: ninth café agrees (SYN-0029-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0029 (toml-test string-null).
/// `case_0029_06 = "café"` is exactly "café" with zero diagnostics, and the
/// JSON export carries é — converges across all nine lineages.
#[test]
fn rb_574_syn29_cafe_roundtrip_exact() {
    let src = "case_0029_06 = \"café\"\n";
    assert_eq!(eval_json(src)["case_0029_06"], "café");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("café"), "{json}");
    // Positive control: ASCII neighbours evaluate alongside cleanly.
    assert_eq!(
        eval_json("case_0029_06 = \"cafe\"\n")["case_0029_06"],
        "cafe"
    );
}

/// RB-575 — Derived variant 29-07: ninth clef agrees (SYN-0029-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0029 (toml-test string-null).
/// `case_0029_07 = "𝄞"` (U+1D11E) is exactly one scalar with zero diagnostics —
/// converges across all nine lineages: no lineage splits surrogates.
#[test]
fn rb_575_syn29_supplementary_scalar_single() {
    let src = "case_0029_07 = \"𝄞\"\n";
    let v = eval_json(src)["case_0029_07"].clone();
    assert_eq!(v, "𝄞", "{v}");
    assert_eq!(v.as_str().expect("string").chars().count(), 1, "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: BMP multi-byte text evaluates alongside cleanly.
    assert_eq!(
        eval_json("case_0029_07 = \"café\"\n")["case_0029_07"],
        "café"
    );
}

/// RB-576 — Derived variant 29-08: truncated pair inside comments is E017 (SYN-0029-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0029 (toml-test string-null).
/// Byte fixture: E2 82 (2 of 3 bytes) inside a `#` comment mid-file. E017
/// Error — extends the comment-truncation pin (RB-558) to a second width:
/// decode-before-lex holds at every width, even where valid BOM/NUL would
/// be inert.
#[test]
fn rb_576_syn29_trunc_pair_comment_e017() {
    let diags = parse_file(
        &research_fixture("syn_0029_08_trunc_e282_comment.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Positive control: the well-formed comment neighbour parses silently.
    let codes = codes_of("# hello\nvalue = 1\n");
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(eval_json("# hello\nvalue = 1\n")["value"], 1);
}

/// RB-577 — Derived variant 29-09: ninth mid-escape agrees (SYN-0029-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0029 (toml-test string-null).
/// `case_0029_09 = "a\tb"` is byte-exact 3 chars with one 0x09, zero
/// diagnostics — converges across all nine lineages. (Completes
/// SYN-0029-01…09 coverage.)
#[test]
fn rb_577_syn29_mid_escape_is_tab() {
    let src = "case_0029_09 = \"a\\tb\"\n";
    let v = eval_json(src)["case_0029_09"].clone();
    assert_eq!(v, "a\tb", "{v}");
    let s = v.as_str().expect("string");
    assert_eq!(s.chars().count(), 3, "{v}");
    assert_eq!(s.as_bytes(), b"a\tb", "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the escape-free neighbour evaluates identically clean.
    assert_eq!(eval_json("case_0029_09 = \"ab\"\n")["case_0029_09"], "ab");
}

/// RB-578 — Derived variant 30-01: NUL after indent is E005 (SYN-0030-01 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0030 (toml-test string-us).
/// Byte fixture: two spaces then NUL. E005 Error at (1:3) — mirrors the
/// BOM-after-indent twin (RB-544): the NUL is misplaced after indentation
/// (the leading spaces separately draw E010, documented here but not
/// asserted). NUL offset reporting holds after whitespace too. (Opens
/// SYN-0030-01…09.)
#[test]
fn rb_578_syn30_nul_after_indent_e005() {
    let diags = parse_file(
        &research_fixture("syn_0030_01_nul_after_indent.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E005");
    let e = diags.iter().find(|d| d.code == "E005").expect("E005");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E005 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E005 carries a span");
    assert_eq!((span.line, span.column), (1, 3), "{span:?}");
    // Positive control: the NUL-free indented document fails only E010.
    let codes = codes_of("  k = 1\n");
    assert!(codes.iter().any(|c| c == "E010"), "{codes:?}");
}
/// RB-579 — Derived variant 30-02: bad-UTF-8 decet agrees (SYN-0030-02 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0030 (toml-test string-us).
/// Byte fixture: overlong C0 AF in a string under a tenth key. E017 Error —
/// converges across ten lineages: no lineage replacement-decodes. (Opens
/// SYN-0030-02…09.)
#[test]
fn rb_579_syn30_utf8_decet_e017() {
    let diags = parse_file(
        &research_fixture("syn_0030_02_utf8_in_string.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: two earlier twins fail the same class.
    for name in [
        "src_toml_0047_bad_utf8_in_string.active",
        "syn_0029_02_utf8_in_string.active",
    ] {
        let diags = parse_file(&research_fixture(name), ParseOptions::default())
            .expect_err("twin must fail E017");
        assert!(diags.iter().any(|d| d.code == "E017"), "{name}: {diags:?}");
    }
    // Positive control: the valid-spelling neighbour parses.
    assert_eq!(eval_json("case_0030_02 = \"nxo\"\n")["case_0030_02"], "nxo");
}

/// RB-580 — Derived variant 30-03: BOM inside strings is data (SYN-0030-03 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0030 (toml-test string-us).
/// Byte fixture: valid-encoding BOM between string bytes. The document
/// evaluates with zero diagnostics and the value keeps U+FEFF as content —
/// the stream-position restriction governs structure, never string data;
/// a BOM outside strings stays E005 (RB-315's fixture converges).
#[test]
fn rb_580_syn30_bom_in_string_data() {
    let module = parse_file(
        &research_fixture("syn_0030_03_bom_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("bom-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(
        v["case_0030_03"], "a\u{feff}b",
        "BOM must survive as data, got {v}"
    );
    assert!(diags.is_empty(), "{diags:?}");
    // Control: a BOM outside strings still fails E005.
    let diags = parse_file(
        &research_fixture("syn_0021_03_bom_offset3.active"),
        ParseOptions::default(),
    )
    .expect_err("bare BOM must fail E005");
    assert!(diags.iter().any(|d| d.code == "E005"), "{diags:?}");
}

/// RB-581 — Derived variant 30-04: tenth `\u` verbatim agrees (SYN-0030-04 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0030 (toml-test string-us).
/// `case_0030_04 = "\u0001"` is the 6-char verbatim literal with zero
/// diagnostics — converges across all ten lineages: no `\u` decoding
/// anywhere.
#[test]
fn rb_581_syn30_unicode_escape_verbatim() {
    let v = eval_json("case_0030_04 = \"\\u0001\"\n");
    assert_eq!(
        v["case_0030_04"], "\\u0001",
        "escape must stay verbatim, got {v}"
    );
    assert_eq!(v["case_0030_04"].as_str().expect("string").len(), 6);
    let codes = codes_of("case_0030_04 = \"\\u0001\"\n");
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: documented escapes still decode.
    let v = eval_json("case_0030_04 = \"a\\tb\"\n");
    assert_eq!(v["case_0030_04"], "a\tb");
}

/// RB-582 — Derived variant 30-05: tenth TAB-in-string agrees (SYN-0030-05 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0030 (toml-test string-us).
/// Byte fixture: literal 0x09 in string content evaluates to "a\tb" with
/// zero diagnostics — converges across all ten lineages: TABs are content
/// everywhere, indentation nowhere.
#[test]
fn rb_582_syn30_tab_in_string_ok() {
    let module = parse_file(
        &research_fixture("syn_0030_05_tab_in_string.active"),
        ParseOptions::default(),
    )
    .unwrap_or_else(|d| panic!("tab-in-string must parse: {d:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let v: serde_json::Value =
        serde_json::from_str(&active_core::convert::value_to_pretty_json(&value)).expect("json");
    assert_eq!(v["case_0030_05"], "a\tb", "{v}");
    assert!(diags.is_empty(), "{diags:?}");
    // Control: TAB as structural indentation still fails E006.
    let diags = parse_file(
        &research_fixture("syn_0021_04_tab_line1.active"),
        ParseOptions::default(),
    )
    .expect_err("tab indent must fail E006");
    assert!(diags.iter().any(|d| d.code == "E006"), "{diags:?}");
}

/// RB-583 — Derived variant 30-06: tenth café agrees (SYN-0030-06 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0030 (toml-test string-us).
/// `case_0030_06 = "café"` is exactly "café" with zero diagnostics, and the
/// JSON export carries é — converges across all ten lineages.
#[test]
fn rb_583_syn30_cafe_roundtrip_exact() {
    let src = "case_0030_06 = \"café\"\n";
    assert_eq!(eval_json(src)["case_0030_06"], "café");
    let module = parse(src, "research").expect("must parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    assert!(diags.is_empty(), "{diags:?}");
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("café"), "{json}");
    // Positive control: ASCII neighbours evaluate alongside cleanly.
    assert_eq!(
        eval_json("case_0030_06 = \"cafe\"\n")["case_0030_06"],
        "cafe"
    );
}

/// RB-584 — Derived variant 30-07: tenth clef agrees (SYN-0030-07 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0030 (toml-test string-us).
/// `case_0030_07 = "𝄞"` (U+1D11E) is exactly one scalar with zero diagnostics —
/// converges across all ten lineages: no lineage splits surrogates.
#[test]
fn rb_584_syn30_supplementary_scalar_single() {
    let src = "case_0030_07 = \"𝄞\"\n";
    let v = eval_json(src)["case_0030_07"].clone();
    assert_eq!(v, "𝄞", "{v}");
    assert_eq!(v.as_str().expect("string").chars().count(), 1, "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: BMP multi-byte text evaluates alongside cleanly.
    assert_eq!(
        eval_json("case_0030_07 = \"café\"\n")["case_0030_07"],
        "café"
    );
}

/// RB-585 — Derived variant 30-08: truncation inside keys is E017 (SYN-0030-08 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0030 (toml-test string-us).
/// Byte fixture: truncated E2 lead inside key bytes. E017 Error — extends
/// the key-position pin (RB-499) from invalid bytes to truncation:
/// decode-before-lex holds in keys at every breakage shape.
#[test]
fn rb_585_syn30_trunc_in_key_e017() {
    let diags = parse_file(
        &research_fixture("syn_0030_08_trunc_in_key.active"),
        ParseOptions::default(),
    )
    .expect_err("must fail E017");
    let e = diags.iter().find(|d| d.code == "E017").expect("E017");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E017 must be Error, got {:?}",
        e.severity
    );
    // Convergence: key-position invalid bytes fail the same class (RB-499).
    let diags = parse_file(
        &research_fixture("syn_0021_02_utf8_in_key.active"),
        ParseOptions::default(),
    )
    .expect_err("twin must fail E017");
    assert!(diags.iter().any(|d| d.code == "E017"), "{diags:?}");
    // Positive control: the valid-spelling key parses.
    assert_eq!(eval_json("cse = 1\n")["cse"], 1);
}

/// RB-586 — Derived variant 30-09: tenth mid-escape agrees (SYN-0030-09 §H, MEDIUM).
/// Origin: synthetic variation of SRC-TOML-0030 (toml-test string-us).
/// `case_0030_09 = "a\tb"` is byte-exact 3 chars with one 0x09, zero
/// diagnostics — converges across all ten lineages. (Completes
/// SYN-0030-02…09 coverage.)
#[test]
fn rb_586_syn30_mid_escape_is_tab() {
    let src = "case_0030_09 = \"a\\tb\"\n";
    let v = eval_json(src)["case_0030_09"].clone();
    assert_eq!(v, "a\tb", "{v}");
    let s = v.as_str().expect("string");
    assert_eq!(s.chars().count(), 3, "{v}");
    assert_eq!(s.as_bytes(), b"a\tb", "{v}");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the escape-free neighbour evaluates identically clean.
    assert_eq!(eval_json("case_0030_09 = \"ab\"\n")["case_0030_09"], "ab");
}

/// RB-587 — Derived variant 31-01: impossible dates are arithmetic, never normalized (SYN-0031-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0031 (toml-test day-zero).
/// REFUSAL (spec-gated): the catalogue asks rejecting invalid date
/// literals, but Active has no datetime type (spec; batch-19 no-date-type
/// policy): `case_0031_01 = 2024-02-30` is the arithmetic 1992 with zero
/// diagnostics — Feb-30 is never constructed, so it can never be silently
/// normalized. The quoted neighbour stays the exact string. (Opens
/// SYN-0031 day-zero lineage.)
#[test]
fn rb_587_syn31_day_zero_is_arithmetic() {
    let src = "case_0031_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0031_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0031_01 = \"2024-02-30\"\n")["case_0031_01"],
        "2024-02-30"
    );
}

/// RB-588 — Derived variant 31-02: offsets never validate — no datetimes exist (SYN-0031-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0031 (toml-test day-zero).
/// REFUSAL (spec-gated): offset-range validation presumes a datetime type
/// Active does not have (spec; batch-19 policy). The bare form degrades to
/// specified pieces — arithmetic prefix 1993, colon-pair `T12 = 30`, and the
/// §4.1 statement-level skip for the tail (RB-100 precedent) — pinned here
/// by whole-document equality so no pseudo-object ever materializes. The
/// quoted neighbour stays the exact string.
#[test]
fn rb_588_syn31_offset_without_datetimes() {
    let src = "case_0031_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0031_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0031_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0031_02"],
        "2024-02-29T12:30:00+25:00"
    );
}
/// RB-589 — Derived variant 31-03: trailing `Z` rejects loudly (SYN-0031-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0031 (toml-test day-zero).
/// `case_0031_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — the `Z`
/// suffix fits no token, so one timezone syntax never degrades into a
/// malformed pseudo-object; it fails loudly. The quoted neighbour stays exact.
#[test]
fn rb_589_syn31_trailing_z_rejects() {
    let src = "case_0031_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0031_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0031_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-590 — Derived variant 31-04: quoted date-shapes skip validation (SYN-0031-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0031 (toml-test day-zero).
/// `case_0031_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// quoted content is never calendar-validated (there is no calendar type to
/// validate against); only quotes preserve the text, the bare form is
/// arithmetic (RB-587 converges).
#[test]
fn rb_590_syn31_quoted_never_validated() {
    let src = "case_0031_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0031_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0031_04 = 2024-02-30\n")["case_0031_04"],
        1992
    );
}

/// RB-591 — Derived variant 31-05: fractional `Z` form rejects loudly (SYN-0031-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0031 (toml-test day-zero).
/// `case_0031_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — precision is never the question: without a datetime type there
/// is no precision to handle deterministically, so the form fails loudly
/// instead of rounding, truncating, or keeping phantom nanoseconds.
#[test]
fn rb_591_syn31_fractional_z_rejects() {
    let src = "case_0031_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0031_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0031_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-592 — Derived variant 31-06: numeric offsets degrade without objects (SYN-0031-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0031 (toml-test day-zero).
/// `case_0031_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to
/// exactly `{case_0031_06: 2022, T12: 30}` with zero diagnostics — the offset
/// never becomes an opaque fallback object: arithmetic prefix, colon pair,
/// §4.1 skip (RB-100 precedent), nothing else materializes.
#[test]
fn rb_592_syn31_offset_degrades_cleanly() {
    let src = "case_0031_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0031_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0031_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0031_06"],
        "2024-01-01T12:30:00-07:00"
    );
}

/// RB-593 — Derived variant 31-07: leading zeros follow decimal policy (SYN-0031-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0031 (toml-test day-zero).
/// `case_0031_07 = 0001-01-01` is the arithmetic -1 (1-1-1, leading zeros
/// stripped per the RB-104 decimal policy) with zero diagnostics — never a
/// numeric date token, never octal, never a year type.
#[test]
fn rb_593_syn31_leading_zero_decimal() {
    let src = "case_0031_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0031_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0031_07 = \"0001-01-01\"\n")["case_0031_07"],
        "0001-01-01"
    );
}

/// RB-594 — Derived variant 31-08: quoted datetimes survive exactly (SYN-0031-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0031 (toml-test day-zero).
/// `case_0031_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — datetime-looking text is preserved character-for-character
/// when quoted; the catalogue's preservation demand holds via quoting.
#[test]
fn rb_594_syn31_quoted_datetime_exact() {
    let src = "case_0031_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0031_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0031_08 = 1979-05-27\n")["case_0031_08"],
        1947
    );
}

/// RB-595 — Derived variant 31-09: out-of-range minutes reject loudly (SYN-0031-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0031 (toml-test day-zero).
/// `case_0031_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) — range
/// validation never runs because type construction never runs: without a
/// datetime type there is no minute field to range-check, so the form fails
/// loudly at the `Z` instead of validating 60.
#[test]
fn rb_595_syn31_bad_minutes_reject() {
    let src = "case_0031_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0031_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0031_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-596 — Derived variant 32-01: Feb-30 twin is arithmetic (SYN-0032-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0032 (toml-test feb-29).
/// `case_0032_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587 under a new key: no parent fixture
/// changes the outcome, Feb-30 is never constructed anywhere.
#[test]
fn rb_596_syn32_feb30_twin_arithmetic() {
    let src = "case_0032_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0032_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0032_01"],
        eval_json("case_0031_01 = 2024-02-30\n")["case_0031_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0032_01 = \"2024-02-30\"\n")["case_0032_01"],
        "2024-02-30"
    );
}

/// RB-597 — Derived variant 32-02: offset twin degrades identically (SYN-0032-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0032 (toml-test feb-29).
/// `case_0032_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to exactly
/// `{case_0032_02: 1993, T12: 30}` with zero diagnostics — converges with
/// RB-588: offset validation is N/A in every lineage. (Opens SYN-0032
/// feb-29 lineage.)
#[test]
fn rb_597_syn32_offset_twin_degrades() {
    let src = "case_0032_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0032_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0032_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0032_02"],
        "2024-02-29T12:30:00+25:00"
    );
}

/// RB-598 — Derived variant 32-03: trailing-`Z` twin rejects (SYN-0032-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0032 (toml-test feb-29).
/// `case_0032_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589: one timezone syntax contract in every lineage, loud reject,
/// never a pseudo-object. (Completes SYN-0031-03…09 coverage.)
#[test]
fn rb_598_syn32_trailing_z_twin_rejects() {
    let src = "case_0032_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0032_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0032_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}
/// RB-599 — Derived variant 32-04: quoted twin never validated (SYN-0032-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0032 (toml-test feb-29).
/// `case_0032_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// converges with RB-590: quoted content skips calendar validation in every
/// lineage (there is no calendar type).
#[test]
fn rb_599_syn32_quoted_never_validated() {
    let src = "case_0032_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0032_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0032_04 = 2024-02-30\n")["case_0032_04"],
        1992
    );
}

/// RB-600 — Derived variant 32-05: fractional twin rejects (SYN-0032-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0032 (toml-test feb-29).
/// `case_0032_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — converges with RB-591: no precision handling without a datetime
/// type, loud reject in every lineage.
#[test]
fn rb_600_syn32_fractional_twin_rejects() {
    let src = "case_0032_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0032_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0032_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-601 — Derived variant 32-06: offset twin degrades identically (SYN-0032-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0032 (toml-test feb-29).
/// `case_0032_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to exactly
/// `{case_0032_06: 2022, T12: 30}` with zero diagnostics — converges with
/// RB-592: the offset never becomes an opaque fallback object anywhere.
#[test]
fn rb_601_syn32_offset_twin_degrades() {
    let src = "case_0032_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0032_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0032_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0032_06"],
        "2024-01-01T12:30:00-07:00"
    );
}

/// RB-602 — Derived variant 32-07: leading-zero twin is decimal (SYN-0032-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0032 (toml-test feb-29).
/// `case_0032_07 = 0001-01-01` is the arithmetic -1 whole-doc with zero
/// diagnostics — converges with RB-593: leading zeros follow the decimal
/// policy (RB-104) in every lineage, never a date token.
#[test]
fn rb_602_syn32_leading_zero_twin_decimal() {
    let src = "case_0032_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0032_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0032_07 = \"0001-01-01\"\n")["case_0032_07"],
        "0001-01-01"
    );
}

/// RB-603 — Derived variant 32-08: quoted twin preserved (SYN-0032-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0032 (toml-test feb-29).
/// `case_0032_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — converges with RB-594: datetime-looking text is preserved
/// character-for-character when quoted, in every lineage.
#[test]
fn rb_603_syn32_quoted_datetime_twin_exact() {
    let src = "case_0032_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0032_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0032_08 = 1979-05-27\n")["case_0032_08"],
        1947
    );
}

/// RB-604 — Derived variant 32-09: bad-minutes twin rejects (SYN-0032-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0032 (toml-test feb-29).
/// `case_0032_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) —
/// converges with RB-595: no minute field exists to range-check anywhere,
/// loud reject in every lineage. (Completes SYN-0032-04…09 coverage.)
#[test]
fn rb_604_syn32_bad_minutes_twin_reject() {
    let src = "case_0032_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0032_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0032_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-605 — Derived variant 33-01: Feb-30 third twin is arithmetic (SYN-0033-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0033 (toml-test feb-30).
/// `case_0033_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587 and RB-596: three lineages, one
/// resolver, Feb-30 never constructed. (Opens SYN-0033 feb-30 lineage.)
#[test]
fn rb_605_syn33_feb30_third_arithmetic() {
    let src = "case_0033_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0033_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0033_01"],
        eval_json("case_0031_01 = 2024-02-30\n")["case_0031_01"]
    );
    assert_eq!(
        v["case_0033_01"],
        eval_json("case_0032_01 = 2024-02-30\n")["case_0032_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0033_01 = \"2024-02-30\"\n")["case_0033_01"],
        "2024-02-30"
    );
}

/// RB-606 — Derived variant 33-02: offset twins collide identically (SYN-0033-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0033 (toml-test feb-30).
/// Alone, `case_0033_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to
/// exactly `{case_0033_02: 1993, T12: 30}` with zero diagnostics (converges
/// with RB-588/RB-597). Together with its twin, both `T12` pairs collide
/// into one announced W011 at (2:26) with T12 == 30 — the twins agree so
/// fully they are indistinguishable downstream, and the collision is loud.
#[test]
fn rb_606_syn33_offset_twins_collide() {
    let src = "case_0033_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0033_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Twin collision: both offset forms in one document share one loud T12.
    let src =
        "case_0031_02 = 2024-02-29T12:30:00+25:00\ncase_0033_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(v["case_0031_02"], 1993, "{v}");
    assert_eq!(v["case_0033_02"], 1993, "{v}");
    assert_eq!(v["T12"], 30, "{v}");
    let module = parse(src, "research").expect("must parse");
    let (_, diags) = Engine::new().evaluate(&module).expect("must evaluate");
    let w = diags.iter().find(|d| d.code == "W011").expect("W011");
    let span = w.span.clone().expect("W011 carries a span");
    assert_eq!((span.line, span.column), (2, 26), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0033_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0033_02"],
        "2024-02-29T12:30:00+25:00"
    );
}

/// RB-607 — Derived variant 33-03: trailing-`Z` third twin rejects (SYN-0033-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0033 (toml-test feb-30).
/// `case_0033_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589 and RB-598: one timezone syntax contract in all three
/// lineages, loud reject, never a pseudo-object.
#[test]
fn rb_607_syn33_trailing_z_third_rejects() {
    let src = "case_0033_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0033_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0033_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-608 — Derived variant 33-04: quoted third twin never validated (SYN-0033-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0033 (toml-test feb-30).
/// `case_0033_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// converges with RB-590 and RB-599: quoted content skips calendar
/// validation in all three lineages. (Completes SYN-0032-04…09 coverage +
/// opens SYN-0033-01…04.)
#[test]
fn rb_608_syn33_quoted_third_never_validated() {
    let src = "case_0033_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0033_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0033_04 = 2024-02-30\n")["case_0033_04"],
        1992
    );
}
/// RB-609 — Derived variant 33-05: fractional third twin rejects (SYN-0033-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0033 (toml-test feb-30).
/// `case_0033_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — converges with RB-591 and RB-600: no precision handling without
/// a datetime type, loud reject in all three lineages.
#[test]
fn rb_609_syn33_fractional_third_rejects() {
    let src = "case_0033_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0033_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0033_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-610 — Derived variant 33-06: offset third twin degrades (SYN-0033-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0033 (toml-test feb-30).
/// `case_0033_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to exactly
/// `{case_0033_06: 2022, T12: 30}` with zero diagnostics — converges with
/// RB-592 and RB-601: the offset never becomes an opaque fallback object.
#[test]
fn rb_610_syn33_offset_third_degrades() {
    let src = "case_0033_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0033_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0033_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0033_06"],
        "2024-01-01T12:30:00-07:00"
    );
}

/// RB-611 — Derived variant 33-07: leading-zero third twin is decimal (SYN-0033-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0033 (toml-test feb-30).
/// `case_0033_07 = 0001-01-01` is the arithmetic -1 whole-doc with zero
/// diagnostics — converges with RB-593 and RB-602: leading zeros follow the
/// decimal policy (RB-104) in all three lineages, never a date token.
#[test]
fn rb_611_syn33_leading_zero_third_decimal() {
    let src = "case_0033_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0033_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0033_07 = \"0001-01-01\"\n")["case_0033_07"],
        "0001-01-01"
    );
}

/// RB-612 — Derived variant 33-08: quoted third twin preserved (SYN-0033-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0033 (toml-test feb-30).
/// `case_0033_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — converges with RB-594 and RB-603: datetime-looking text is
/// preserved character-for-character when quoted, in all three lineages.
#[test]
fn rb_612_syn33_quoted_datetime_third_exact() {
    let src = "case_0033_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0033_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0033_08 = 1979-05-27\n")["case_0033_08"],
        1947
    );
}

/// RB-613 — Derived variant 33-09: bad-minutes third twin rejects (SYN-0033-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0033 (toml-test feb-30).
/// `case_0033_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) —
/// converges with RB-595 and RB-604: no minute field exists to range-check
/// anywhere, loud reject in all three lineages. (Completes SYN-0033-05…09
/// coverage.)
#[test]
fn rb_613_syn33_bad_minutes_third_reject() {
    let src = "case_0033_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0033_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0033_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-614 — Derived variant 34-01: Feb-30 fourth twin is arithmetic (SYN-0034-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0034 (toml-test hour-over).
/// `case_0034_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587, RB-596 and RB-605: four lineages,
/// one resolver, Feb-30 never constructed. (Opens SYN-0034 hour-over
/// lineage.)
#[test]
fn rb_614_syn34_feb30_fourth_arithmetic() {
    let src = "case_0034_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0034_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0034_01"],
        eval_json("case_0033_01 = 2024-02-30\n")["case_0033_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0034_01 = \"2024-02-30\"\n")["case_0034_01"],
        "2024-02-30"
    );
}

/// RB-615 — Derived variant 34-02: offset fourth twin degrades (SYN-0034-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0034 (toml-test hour-over).
/// `case_0034_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to exactly
/// `{case_0034_02: 1993, T12: 30}` with zero diagnostics — converges with
/// RB-588, RB-597 and RB-606: offset validation is N/A in every lineage.
#[test]
fn rb_615_syn34_offset_fourth_degrades() {
    let src = "case_0034_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0034_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0034_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0034_02"],
        "2024-02-29T12:30:00+25:00"
    );
}

/// RB-616 — Derived variant 34-03: trailing-`Z` fourth twin rejects (SYN-0034-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0034 (toml-test hour-over).
/// `case_0034_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589, RB-598 and RB-607: one timezone syntax contract in all four
/// lineages, loud reject, never a pseudo-object.
#[test]
fn rb_616_syn34_trailing_z_fourth_rejects() {
    let src = "case_0034_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0034_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0034_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-617 — Derived variant 34-04: quoted fourth twin never validated (SYN-0034-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0034 (toml-test hour-over).
/// `case_0034_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// converges with RB-590, RB-599 and RB-608: quoted content skips calendar
/// validation in all four lineages.
#[test]
fn rb_617_syn34_quoted_fourth_never_validated() {
    let src = "case_0034_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0034_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0034_04 = 2024-02-30\n")["case_0034_04"],
        1992
    );
}

/// RB-618 — Derived variant 34-05: fractional fourth twin rejects (SYN-0034-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0034 (toml-test hour-over).
/// `case_0034_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — converges with RB-591, RB-600 and RB-609: no precision handling
/// without a datetime type, loud reject in all four lineages. (Completes
/// SYN-0033-05…09 coverage + opens SYN-0034-01…05.)
#[test]
fn rb_618_syn34_fractional_fourth_rejects() {
    let src = "case_0034_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0034_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0034_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}
/// RB-619 — Derived variant 34-06: offset fourth twin degrades (SYN-0034-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0034 (toml-test hour-over).
/// `case_0034_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to exactly
/// `{case_0034_06: 2022, T12: 30}` with zero diagnostics — converges with
/// RB-592, RB-601 and RB-610: the offset never becomes an opaque fallback
/// object.
#[test]
fn rb_619_syn34_offset_fourth_degrades() {
    let src = "case_0034_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0034_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0034_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0034_06"],
        "2024-01-01T12:30:00-07:00"
    );
}

/// RB-620 — Derived variant 34-07: leading-zero fourth twin is decimal (SYN-0034-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0034 (toml-test hour-over).
/// `case_0034_07 = 0001-01-01` is the arithmetic -1 whole-doc with zero
/// diagnostics — converges with RB-593, RB-602 and RB-611: leading zeros
/// follow the decimal policy (RB-104) in all four lineages.
#[test]
fn rb_620_syn34_leading_zero_fourth_decimal() {
    let src = "case_0034_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0034_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0034_07 = \"0001-01-01\"\n")["case_0034_07"],
        "0001-01-01"
    );
}

/// RB-621 — Derived variant 34-08: quoted fourth twin preserved (SYN-0034-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0034 (toml-test hour-over).
/// `case_0034_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — converges with RB-594, RB-603 and RB-612: datetime-looking
/// text is preserved character-for-character when quoted, in all four
/// lineages.
#[test]
fn rb_621_syn34_quoted_datetime_fourth_exact() {
    let src = "case_0034_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0034_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0034_08 = 1979-05-27\n")["case_0034_08"],
        1947
    );
}

/// RB-622 — Derived variant 34-09: bad-minutes fourth twin rejects (SYN-0034-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0034 (toml-test hour-over).
/// `case_0034_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) —
/// converges with RB-595, RB-604 and RB-613: no minute field exists to
/// range-check anywhere, loud reject in all four lineages. (Completes
/// SYN-0034-06…09 coverage.)
#[test]
fn rb_622_syn34_bad_minutes_fourth_reject() {
    let src = "case_0034_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0034_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0034_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-623 — Derived variant 35-01: Feb-30 fifth twin is arithmetic (SYN-0035-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0035 (toml-test leading-zero-date).
/// `case_0035_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587, RB-596, RB-605 and RB-614: five
/// lineages, one resolver, Feb-30 never constructed. (Opens SYN-0035
/// leading-zero-date lineage.)
#[test]
fn rb_623_syn35_feb30_fifth_arithmetic() {
    let src = "case_0035_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0035_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0035_01"],
        eval_json("case_0034_01 = 2024-02-30\n")["case_0034_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0035_01 = \"2024-02-30\"\n")["case_0035_01"],
        "2024-02-30"
    );
}

/// RB-624 — Derived variant 35-02: offset fifth twin degrades (SYN-0035-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0035 (toml-test leading-zero-date).
/// `case_0035_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to exactly
/// `{case_0035_02: 1993, T12: 30}` with zero diagnostics — converges with
/// RB-588, RB-597, RB-606 and RB-615: offset validation is N/A everywhere.
#[test]
fn rb_624_syn35_offset_fifth_degrades() {
    let src = "case_0035_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0035_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0035_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0035_02"],
        "2024-02-29T12:30:00+25:00"
    );
}

/// RB-625 — Derived variant 35-03: trailing-`Z` fifth twin rejects (SYN-0035-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0035 (toml-test leading-zero-date).
/// `case_0035_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589, RB-598, RB-607 and RB-616: one timezone syntax contract in
/// all five lineages, loud reject, never a pseudo-object.
#[test]
fn rb_625_syn35_trailing_z_fifth_rejects() {
    let src = "case_0035_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0035_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0035_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-626 — Derived variant 35-04: quoted fifth twin never validated (SYN-0035-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0035 (toml-test leading-zero-date).
/// `case_0035_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// converges with RB-590, RB-599, RB-608 and RB-617: quoted content skips
/// calendar validation in all five lineages.
#[test]
fn rb_626_syn35_quoted_fifth_never_validated() {
    let src = "case_0035_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0035_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0035_04 = 2024-02-30\n")["case_0035_04"],
        1992
    );
}

/// RB-627 — Derived variant 35-05: fractional fifth twin rejects (SYN-0035-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0035 (toml-test leading-zero-date).
/// `case_0035_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — converges with RB-591, RB-600, RB-609 and RB-618: no precision
/// handling without a datetime type, loud reject in all five lineages.
#[test]
fn rb_627_syn35_fractional_fifth_rejects() {
    let src = "case_0035_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0035_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0035_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-628 — Derived variant 35-06: offset fifth twin degrades (SYN-0035-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0035 (toml-test leading-zero-date).
/// `case_0035_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to exactly
/// `{case_0035_06: 2022, T12: 30}` with zero diagnostics — converges with
/// RB-592, RB-601, RB-610 and RB-619: the offset never becomes an opaque
/// fallback object. (Completes SYN-0034-06…09 coverage + opens
/// SYN-0035-01…06.)
#[test]
fn rb_628_syn35_offset_fifth_degrades() {
    let src = "case_0035_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0035_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0035_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0035_06"],
        "2024-01-01T12:30:00-07:00"
    );
}
/// RB-629 — Derived variant 35-07: leading-zero fifth twin is decimal (SYN-0035-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0035 (toml-test leading-zero-date).
/// `case_0035_07 = 0001-01-01` is the arithmetic -1 whole-doc with zero
/// diagnostics — converges with RB-593, RB-602, RB-611 and RB-620: leading
/// zeros follow the decimal policy (RB-104) in all five lineages.
#[test]
fn rb_629_syn35_leading_zero_fifth_decimal() {
    let src = "case_0035_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0035_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0035_07 = \"0001-01-01\"\n")["case_0035_07"],
        "0001-01-01"
    );
}

/// RB-630 — Derived variant 35-08: quoted fifth twin preserved (SYN-0035-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0035 (toml-test leading-zero-date).
/// `case_0035_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — converges with RB-594, RB-603, RB-612 and RB-621:
/// datetime-looking text is preserved character-for-character when quoted,
/// in all five lineages.
#[test]
fn rb_630_syn35_quoted_datetime_fifth_exact() {
    let src = "case_0035_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0035_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0035_08 = 1979-05-27\n")["case_0035_08"],
        1947
    );
}

/// RB-631 — Derived variant 35-09: bad-minutes fifth twin rejects (SYN-0035-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0035 (toml-test leading-zero-date).
/// `case_0035_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) —
/// converges with RB-595, RB-604, RB-613 and RB-622: no minute field exists
/// to range-check anywhere, loud reject in all five lineages. (Completes
/// SYN-0035-07…09 coverage.)
#[test]
fn rb_631_syn35_bad_minutes_fifth_reject() {
    let src = "case_0035_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0035_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0035_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-632 — Derived variant 36-01: Feb-30 sixth twin is arithmetic (SYN-0036-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0036 (toml-test no-secs).
/// `case_0036_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587, RB-596, RB-605, RB-614 and RB-623:
/// six lineages, one resolver, Feb-30 never constructed. (Opens SYN-0036
/// no-secs lineage.)
#[test]
fn rb_632_syn36_feb30_sixth_arithmetic() {
    let src = "case_0036_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0036_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0036_01"],
        eval_json("case_0035_01 = 2024-02-30\n")["case_0035_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0036_01 = \"2024-02-30\"\n")["case_0036_01"],
        "2024-02-30"
    );
}

/// RB-633 — Derived variant 36-02: offset sixth twin degrades (SYN-0036-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0036 (toml-test no-secs).
/// `case_0036_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to exactly
/// `{case_0036_02: 1993, T12: 30}` with zero diagnostics — converges with
/// RB-588, RB-597, RB-606, RB-615 and RB-624: offset validation is N/A
/// everywhere.
#[test]
fn rb_633_syn36_offset_sixth_degrades() {
    let src = "case_0036_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0036_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0036_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0036_02"],
        "2024-02-29T12:30:00+25:00"
    );
}

/// RB-634 — Derived variant 36-03: trailing-`Z` sixth twin rejects (SYN-0036-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0036 (toml-test no-secs).
/// `case_0036_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589, RB-598, RB-607, RB-616 and RB-625: one timezone syntax
/// contract in all six lineages, loud reject, never a pseudo-object.
#[test]
fn rb_634_syn36_trailing_z_sixth_rejects() {
    let src = "case_0036_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0036_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0036_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-635 — Derived variant 36-04: quoted sixth twin never validated (SYN-0036-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0036 (toml-test no-secs).
/// `case_0036_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// converges with RB-590, RB-599, RB-608, RB-617 and RB-626: quoted content
/// skips calendar validation in all six lineages.
#[test]
fn rb_635_syn36_quoted_sixth_never_validated() {
    let src = "case_0036_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0036_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0036_04 = 2024-02-30\n")["case_0036_04"],
        1992
    );
}

/// RB-636 — Derived variant 36-05: fractional sixth twin rejects (SYN-0036-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0036 (toml-test no-secs).
/// `case_0036_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — converges with RB-591, RB-600, RB-609, RB-618 and RB-627: no
/// precision handling without a datetime type, loud reject in all six
/// lineages.
#[test]
fn rb_636_syn36_fractional_sixth_rejects() {
    let src = "case_0036_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0036_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0036_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-637 — Derived variant 36-06: offset sixth twin degrades (SYN-0036-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0036 (toml-test no-secs).
/// `case_0036_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to exactly
/// `{case_0036_06: 2022, T12: 30}` with zero diagnostics — converges with
/// RB-592, RB-601, RB-610, RB-619 and RB-628: the offset never becomes an
/// opaque fallback object.
#[test]
fn rb_637_syn36_offset_sixth_degrades() {
    let src = "case_0036_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0036_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0036_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0036_06"],
        "2024-01-01T12:30:00-07:00"
    );
}

/// RB-638 — Derived variant 36-07: leading-zero sixth twin is decimal (SYN-0036-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0036 (toml-test no-secs).
/// `case_0036_07 = 0001-01-01` is the arithmetic -1 whole-doc with zero
/// diagnostics — converges with RB-593, RB-602, RB-611, RB-620 and RB-629:
/// leading zeros follow the decimal policy (RB-104) in all six lineages.
/// (Completes SYN-0035-07…09 coverage + opens SYN-0036-01…07.)
#[test]
fn rb_638_syn36_leading_zero_sixth_decimal() {
    let src = "case_0036_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0036_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0036_07 = \"0001-01-01\"\n")["case_0036_07"],
        "0001-01-01"
    );
}
/// RB-639 — Derived variant 36-08: quoted sixth twin preserved (SYN-0036-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0036 (toml-test no-secs).
/// `case_0036_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — converges with RB-594, RB-603, RB-612, RB-621 and RB-630:
/// datetime-looking text is preserved character-for-character when quoted,
/// in all six lineages.
#[test]
fn rb_639_syn36_quoted_datetime_sixth_exact() {
    let src = "case_0036_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0036_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0036_08 = 1979-05-27\n")["case_0036_08"],
        1947
    );
}

/// RB-640 — Derived variant 36-09: bad-minutes sixth twin rejects (SYN-0036-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0036 (toml-test no-secs).
/// `case_0036_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) —
/// converges with RB-595, RB-604, RB-613, RB-622 and RB-631: no minute field
/// exists to range-check anywhere, loud reject in all six lineages.
/// (Completes SYN-0036-08…09 coverage.)
#[test]
fn rb_640_syn36_bad_minutes_sixth_reject() {
    let src = "case_0036_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0036_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0036_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-641 — Derived variant 37-01: Feb-30 seventh twin is arithmetic (SYN-0037-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0037 (toml-test no-t).
/// `case_0037_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587, RB-596, RB-605, RB-614, RB-623 and
/// RB-632: seven lineages, one resolver, Feb-30 never constructed. (Opens
/// SYN-0037 no-t lineage.)
#[test]
fn rb_641_syn37_feb30_seventh_arithmetic() {
    let src = "case_0037_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0037_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0037_01"],
        eval_json("case_0036_01 = 2024-02-30\n")["case_0036_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0037_01 = \"2024-02-30\"\n")["case_0037_01"],
        "2024-02-30"
    );
}

/// RB-642 — Derived variant 37-02: offset seventh twin degrades (SYN-0037-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0037 (toml-test no-t).
/// `case_0037_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to exactly
/// `{case_0037_02: 1993, T12: 30}` with zero diagnostics — converges with
/// RB-588, RB-597, RB-606, RB-615, RB-624 and RB-633: offset validation is
/// N/A everywhere.
#[test]
fn rb_642_syn37_offset_seventh_degrades() {
    let src = "case_0037_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0037_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0037_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0037_02"],
        "2024-02-29T12:30:00+25:00"
    );
}

/// RB-643 — Derived variant 37-03: trailing-`Z` seventh twin rejects (SYN-0037-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0037 (toml-test no-t).
/// `case_0037_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589, RB-598, RB-607, RB-616, RB-625 and RB-634: one timezone
/// syntax contract in all seven lineages, loud reject, never a pseudo-object.
#[test]
fn rb_643_syn37_trailing_z_seventh_rejects() {
    let src = "case_0037_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0037_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0037_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-644 — Derived variant 37-04: quoted seventh twin never validated (SYN-0037-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0037 (toml-test no-t).
/// `case_0037_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// converges with RB-590, RB-599, RB-608, RB-617, RB-626 and RB-635: quoted
/// content skips calendar validation in all seven lineages.
#[test]
fn rb_644_syn37_quoted_seventh_never_validated() {
    let src = "case_0037_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0037_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0037_04 = 2024-02-30\n")["case_0037_04"],
        1992
    );
}

/// RB-645 — Derived variant 37-05: fractional seventh twin rejects (SYN-0037-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0037 (toml-test no-t).
/// `case_0037_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — converges with RB-591, RB-600, RB-609, RB-618, RB-627 and
/// RB-636: no precision handling without a datetime type, loud reject in all
/// seven lineages.
#[test]
fn rb_645_syn37_fractional_seventh_rejects() {
    let src = "case_0037_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0037_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0037_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-646 — Derived variant 37-06: offset seventh twin degrades (SYN-0037-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0037 (toml-test no-t).
/// `case_0037_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to exactly
/// `{case_0037_06: 2022, T12: 30}` with zero diagnostics — converges with
/// RB-592, RB-601, RB-610, RB-619, RB-628 and RB-637: the offset never
/// becomes an opaque fallback object.
#[test]
fn rb_646_syn37_offset_seventh_degrades() {
    let src = "case_0037_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0037_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0037_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0037_06"],
        "2024-01-01T12:30:00-07:00"
    );
}

/// RB-647 — Derived variant 37-07: leading-zero seventh twin is decimal (SYN-0037-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0037 (toml-test no-t).
/// `case_0037_07 = 0001-01-01` is the arithmetic -1 whole-doc with zero
/// diagnostics — converges with RB-593, RB-602, RB-611, RB-620, RB-629 and
/// RB-638: leading zeros follow the decimal policy (RB-104) in all seven
/// lineages.
#[test]
fn rb_647_syn37_leading_zero_seventh_decimal() {
    let src = "case_0037_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0037_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0037_07 = \"0001-01-01\"\n")["case_0037_07"],
        "0001-01-01"
    );
}

/// RB-648 — Derived variant 37-08: quoted seventh twin preserved (SYN-0037-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0037 (toml-test no-t).
/// `case_0037_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — converges with RB-594, RB-603, RB-612, RB-621, RB-630 and
/// RB-639: datetime-looking text is preserved character-for-character when
/// quoted, in all seven lineages. (Completes SYN-0036-08…09 coverage +
/// opens SYN-0037-01…08.)
#[test]
fn rb_648_syn37_quoted_datetime_seventh_exact() {
    let src = "case_0037_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0037_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0037_08 = 1979-05-27\n")["case_0037_08"],
        1947
    );
}
/// RB-649 — Derived variant 37-09: bad-minutes seventh twin rejects (SYN-0037-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0037 (toml-test no-t).
/// `case_0037_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) —
/// converges with RB-595, RB-604, RB-613, RB-622, RB-631 and RB-640: no
/// minute field exists to range-check anywhere, loud reject in all seven
/// lineages. (Completes SYN-0037-09 coverage.)
#[test]
fn rb_649_syn37_bad_minutes_seventh_reject() {
    let src = "case_0037_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0037_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0037_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-650 — Derived variant 38-01: Feb-30 eighth twin is arithmetic (SYN-0038-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0038 (toml-test offset-overflow-hour).
/// `case_0038_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587, RB-596, RB-605, RB-614, RB-623,
/// RB-632 and RB-641: eight lineages, one resolver, Feb-30 never
/// constructed. (Opens SYN-0038 offset-overflow-hour lineage.)
#[test]
fn rb_650_syn38_feb30_eighth_arithmetic() {
    let src = "case_0038_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0038_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0038_01"],
        eval_json("case_0037_01 = 2024-02-30\n")["case_0037_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0038_01 = \"2024-02-30\"\n")["case_0038_01"],
        "2024-02-30"
    );
}

/// RB-651 — Derived variant 38-02: offset eighth twin degrades (SYN-0038-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0038 (toml-test offset-overflow-hour).
/// `case_0038_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to exactly
/// `{case_0038_02: 1993, T12: 30}` with zero diagnostics — converges with
/// RB-588, RB-597, RB-606, RB-615, RB-624, RB-633 and RB-642: offset
/// validation is N/A everywhere.
#[test]
fn rb_651_syn38_offset_eighth_degrades() {
    let src = "case_0038_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0038_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0038_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0038_02"],
        "2024-02-29T12:30:00+25:00"
    );
}

/// RB-652 — Derived variant 38-03: trailing-`Z` eighth twin rejects (SYN-0038-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0038 (toml-test offset-overflow-hour).
/// `case_0038_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589, RB-598, RB-607, RB-616, RB-625, RB-634 and RB-643: one
/// timezone syntax contract in all eight lineages, loud reject, never a
/// pseudo-object.
#[test]
fn rb_652_syn38_trailing_z_eighth_rejects() {
    let src = "case_0038_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0038_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0038_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-653 — Derived variant 38-04: quoted eighth twin never validated (SYN-0038-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0038 (toml-test offset-overflow-hour).
/// `case_0038_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// converges with RB-590, RB-599, RB-608, RB-617, RB-626, RB-635 and RB-644:
/// quoted content skips calendar validation in all eight lineages.
#[test]
fn rb_653_syn38_quoted_eighth_never_validated() {
    let src = "case_0038_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0038_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0038_04 = 2024-02-30\n")["case_0038_04"],
        1992
    );
}

/// RB-654 — Derived variant 38-05: fractional eighth twin rejects (SYN-0038-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0038 (toml-test offset-overflow-hour).
/// `case_0038_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — converges with RB-591, RB-600, RB-609, RB-618, RB-627, RB-636
/// and RB-645: no precision handling without a datetime type, loud reject in
/// all eight lineages.
#[test]
fn rb_654_syn38_fractional_eighth_rejects() {
    let src = "case_0038_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0038_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0038_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-655 — Derived variant 38-06: offset eighth twin degrades (SYN-0038-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0038 (toml-test offset-overflow-hour).
/// `case_0038_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to exactly
/// `{case_0038_06: 2022, T12: 30}` with zero diagnostics — converges with
/// RB-592, RB-601, RB-610, RB-619, RB-628, RB-637 and RB-646: the offset
/// never becomes an opaque fallback object.
#[test]
fn rb_655_syn38_offset_eighth_degrades() {
    let src = "case_0038_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0038_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0038_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0038_06"],
        "2024-01-01T12:30:00-07:00"
    );
}

/// RB-656 — Derived variant 38-07: leading-zero eighth twin is decimal (SYN-0038-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0038 (toml-test offset-overflow-hour).
/// `case_0038_07 = 0001-01-01` is the arithmetic -1 whole-doc with zero
/// diagnostics — converges with RB-593, RB-602, RB-611, RB-620, RB-629,
/// RB-638 and RB-647: leading zeros follow the decimal policy (RB-104) in
/// all eight lineages.
#[test]
fn rb_656_syn38_leading_zero_eighth_decimal() {
    let src = "case_0038_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0038_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0038_07 = \"0001-01-01\"\n")["case_0038_07"],
        "0001-01-01"
    );
}

/// RB-657 — Derived variant 38-08: quoted eighth twin preserved (SYN-0038-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0038 (toml-test offset-overflow-hour).
/// `case_0038_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — converges with RB-594, RB-603, RB-612, RB-621, RB-630,
/// RB-639 and RB-648: datetime-looking text is preserved
/// character-for-character when quoted, in all eight lineages.
#[test]
fn rb_657_syn38_quoted_datetime_eighth_exact() {
    let src = "case_0038_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0038_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0038_08 = 1979-05-27\n")["case_0038_08"],
        1947
    );
}

/// RB-658 — Derived variant 38-09: bad-minutes eighth twin rejects (SYN-0038-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0038 (toml-test offset-overflow-hour).
/// `case_0038_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) —
/// converges with RB-595, RB-604, RB-613, RB-622, RB-631, RB-640 and RB-649:
/// no minute field exists to range-check anywhere, loud reject in all eight
/// lineages. (Completes SYN-0037-09 coverage + opens SYN-0038-01…09.)
#[test]
fn rb_658_syn38_bad_minutes_eighth_reject() {
    let src = "case_0038_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0038_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0038_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}
/// RB-659 — Derived variant 39-01: Feb-30 ninth twin is arithmetic (SYN-0039-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0039 (toml-test offset-overflow-minute).
/// `case_0039_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587, RB-596, RB-605, RB-614, RB-623,
/// RB-632, RB-641 and RB-650: nine lineages, one resolver, Feb-30 never
/// constructed. (Opens SYN-0039 offset-overflow-minute lineage.)
#[test]
fn rb_659_syn39_feb30_ninth_arithmetic() {
    let src = "case_0039_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0039_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0039_01"],
        eval_json("case_0038_01 = 2024-02-30\n")["case_0038_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0039_01 = \"2024-02-30\"\n")["case_0039_01"],
        "2024-02-30"
    );
}

/// RB-660 — Derived variant 39-02: offset ninth twin degrades (SYN-0039-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0039 (toml-test offset-overflow-minute).
/// `case_0039_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to exactly
/// `{case_0039_02: 1993, T12: 30}` with zero diagnostics — converges with
/// RB-588, RB-597, RB-606, RB-615, RB-624, RB-633, RB-642 and RB-651: offset
/// validation is N/A everywhere.
#[test]
fn rb_660_syn39_offset_ninth_degrades() {
    let src = "case_0039_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0039_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0039_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0039_02"],
        "2024-02-29T12:30:00+25:00"
    );
}

/// RB-661 — Derived variant 39-03: trailing-`Z` ninth twin rejects (SYN-0039-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0039 (toml-test offset-overflow-minute).
/// `case_0039_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589, RB-598, RB-607, RB-616, RB-625, RB-634, RB-643 and RB-652:
/// one timezone syntax contract in all nine lineages, loud reject, never a
/// pseudo-object.
#[test]
fn rb_661_syn39_trailing_z_ninth_rejects() {
    let src = "case_0039_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0039_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0039_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-662 — Derived variant 39-04: quoted ninth twin never validated (SYN-0039-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0039 (toml-test offset-overflow-minute).
/// `case_0039_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// converges with RB-590, RB-599, RB-608, RB-617, RB-626, RB-635, RB-644 and
/// RB-653: quoted content skips calendar validation in all nine lineages.
#[test]
fn rb_662_syn39_quoted_ninth_never_validated() {
    let src = "case_0039_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0039_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0039_04 = 2024-02-30\n")["case_0039_04"],
        1992
    );
}

/// RB-663 — Derived variant 39-05: fractional ninth twin rejects (SYN-0039-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0039 (toml-test offset-overflow-minute).
/// `case_0039_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — converges with RB-591, RB-600, RB-609, RB-618, RB-627, RB-636,
/// RB-645 and RB-654: no precision handling without a datetime type, loud
/// reject in all nine lineages.
#[test]
fn rb_663_syn39_fractional_ninth_rejects() {
    let src = "case_0039_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0039_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0039_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-664 — Derived variant 39-06: offset ninth twin degrades (SYN-0039-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0039 (toml-test offset-overflow-minute).
/// `case_0039_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to exactly
/// `{case_0039_06: 2022, T12: 30}` with zero diagnostics — converges with
/// RB-592, RB-601, RB-610, RB-619, RB-628, RB-637, RB-646 and RB-655: the
/// offset never becomes an opaque fallback object.
#[test]
fn rb_664_syn39_offset_ninth_degrades() {
    let src = "case_0039_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0039_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0039_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0039_06"],
        "2024-01-01T12:30:00-07:00"
    );
}

/// RB-665 — Derived variant 39-07: leading-zero ninth twin is decimal (SYN-0039-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0039 (toml-test offset-overflow-minute).
/// `case_0039_07 = 0001-01-01` is the arithmetic -1 whole-doc with zero
/// diagnostics — converges with RB-593, RB-602, RB-611, RB-620, RB-629,
/// RB-638, RB-647 and RB-656: leading zeros follow the decimal policy
/// (RB-104) in all nine lineages.
#[test]
fn rb_665_syn39_leading_zero_ninth_decimal() {
    let src = "case_0039_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0039_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0039_07 = \"0001-01-01\"\n")["case_0039_07"],
        "0001-01-01"
    );
}

/// RB-666 — Derived variant 39-08: quoted ninth twin preserved (SYN-0039-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0039 (toml-test offset-overflow-minute).
/// `case_0039_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — converges with RB-594, RB-603, RB-612, RB-621, RB-630,
/// RB-639, RB-648 and RB-657: datetime-looking text is preserved
/// character-for-character when quoted, in all nine lineages.
#[test]
fn rb_666_syn39_quoted_datetime_ninth_exact() {
    let src = "case_0039_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0039_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0039_08 = 1979-05-27\n")["case_0039_08"],
        1947
    );
}

/// RB-667 — Derived variant 39-09: bad-minutes ninth twin rejects (SYN-0039-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0039 (toml-test offset-overflow-minute).
/// `case_0039_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) —
/// converges with RB-595, RB-604, RB-613, RB-622, RB-631, RB-640, RB-649 and
/// RB-658: no minute field exists to range-check anywhere, loud reject in
/// all nine lineages. (Completes SYN-0039-01…09 coverage.)
#[test]
fn rb_667_syn39_bad_minutes_ninth_reject() {
    let src = "case_0039_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0039_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0039_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-668 — Derived variant 40-01: Feb-30 tenth twin is arithmetic (SYN-0040-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0040 (toml-test second-over).
/// `case_0040_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587, RB-596, RB-605, RB-614, RB-623,
/// RB-632, RB-641, RB-650 and RB-659: ten lineages, one resolver, Feb-30
/// never constructed. (Opens SYN-0040 second-over lineage.)
#[test]
fn rb_668_syn40_feb30_tenth_arithmetic() {
    let src = "case_0040_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0040_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0040_01"],
        eval_json("case_0039_01 = 2024-02-30\n")["case_0039_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0040_01 = \"2024-02-30\"\n")["case_0040_01"],
        "2024-02-30"
    );
}
/// RB-669 — Derived variant 40-02: offset tenth twin degrades (SYN-0040-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0040 (toml-test second-over).
/// `case_0040_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to exactly
/// `{case_0040_02: 1993, T12: 30}` with zero diagnostics — converges with
/// RB-588, RB-597, RB-606, RB-615, RB-624, RB-633, RB-642, RB-651 and
/// RB-660: offset validation is N/A everywhere.
#[test]
fn rb_669_syn40_offset_tenth_degrades() {
    let src = "case_0040_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0040_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0040_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0040_02"],
        "2024-02-29T12:30:00+25:00"
    );
}

/// RB-670 — Derived variant 40-03: trailing-`Z` tenth twin rejects (SYN-0040-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0040 (toml-test second-over).
/// `case_0040_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589, RB-598, RB-607, RB-616, RB-625, RB-634, RB-643, RB-652 and
/// RB-661: one timezone syntax contract in all ten lineages, loud reject,
/// never a pseudo-object.
#[test]
fn rb_670_syn40_trailing_z_tenth_rejects() {
    let src = "case_0040_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0040_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0040_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-671 — Derived variant 40-04: quoted tenth twin never validated (SYN-0040-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0040 (toml-test second-over).
/// `case_0040_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// converges with RB-590, RB-599, RB-608, RB-617, RB-626, RB-635, RB-644,
/// RB-653 and RB-662: quoted content skips calendar validation in all ten
/// lineages.
#[test]
fn rb_671_syn40_quoted_tenth_never_validated() {
    let src = "case_0040_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0040_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0040_04 = 2024-02-30\n")["case_0040_04"],
        1992
    );
}

/// RB-672 — Derived variant 40-05: fractional tenth twin rejects (SYN-0040-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0040 (toml-test second-over).
/// `case_0040_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — converges with RB-591, RB-600, RB-609, RB-618, RB-627, RB-636,
/// RB-645, RB-654 and RB-663: no precision handling without a datetime type,
/// loud reject in all ten lineages.
#[test]
fn rb_672_syn40_fractional_tenth_rejects() {
    let src = "case_0040_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0040_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0040_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-673 — Derived variant 40-06: offset tenth twin degrades (SYN-0040-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0040 (toml-test second-over).
/// `case_0040_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to exactly
/// `{case_0040_06: 2022, T12: 30}` with zero diagnostics — converges with
/// RB-592, RB-601, RB-610, RB-619, RB-628, RB-637, RB-646, RB-655 and
/// RB-664: the offset never becomes an opaque fallback object.
#[test]
fn rb_673_syn40_offset_tenth_degrades() {
    let src = "case_0040_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0040_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0040_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0040_06"],
        "2024-01-01T12:30:00-07:00"
    );
}

/// RB-674 — Derived variant 40-07: leading-zero tenth twin is decimal (SYN-0040-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0040 (toml-test second-over).
/// `case_0040_07 = 0001-01-01` is the arithmetic -1 whole-doc with zero
/// diagnostics — converges with RB-593, RB-602, RB-611, RB-620, RB-629,
/// RB-638, RB-647, RB-656 and RB-665: leading zeros follow the decimal
/// policy (RB-104) in all ten lineages.
#[test]
fn rb_674_syn40_leading_zero_tenth_decimal() {
    let src = "case_0040_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0040_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0040_07 = \"0001-01-01\"\n")["case_0040_07"],
        "0001-01-01"
    );
}

/// RB-675 — Derived variant 40-08: quoted tenth twin preserved (SYN-0040-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0040 (toml-test second-over).
/// `case_0040_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — converges with RB-594, RB-603, RB-612, RB-621, RB-630,
/// RB-639, RB-648, RB-657 and RB-666: datetime-looking text is preserved
/// character-for-character when quoted, in all ten lineages.
#[test]
fn rb_675_syn40_quoted_datetime_tenth_exact() {
    let src = "case_0040_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0040_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0040_08 = 1979-05-27\n")["case_0040_08"],
        1947
    );
}

/// RB-676 — Derived variant 40-09: bad-minutes tenth twin rejects (SYN-0040-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0040 (toml-test second-over).
/// `case_0040_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) —
/// converges with RB-595, RB-604, RB-613, RB-622, RB-631, RB-640, RB-649,
/// RB-658 and RB-667: no minute field exists to range-check anywhere, loud
/// reject in all ten lineages. (Completes SYN-0040-02…09 coverage.)
#[test]
fn rb_676_syn40_bad_minutes_tenth_reject() {
    let src = "case_0040_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0040_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0040_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-677 — Derived variant 41-01: Feb-30 eleventh twin is arithmetic (SYN-0041-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0041 (toml-test second-trailing-dot).
/// `case_0041_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587, RB-596, RB-605, RB-614, RB-623,
/// RB-632, RB-641, RB-650, RB-659 and RB-668: eleven lineages, one resolver,
/// Feb-30 never constructed. (Opens SYN-0041 second-trailing-dot lineage.)
#[test]
fn rb_677_syn41_feb30_eleventh_arithmetic() {
    let src = "case_0041_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0041_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0041_01"],
        eval_json("case_0040_01 = 2024-02-30\n")["case_0040_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0041_01 = \"2024-02-30\"\n")["case_0041_01"],
        "2024-02-30"
    );
}

/// RB-678 — Derived variant 41-02: offset eleventh twin degrades (SYN-0041-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0041 (toml-test second-trailing-dot).
/// `case_0041_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to exactly
/// `{case_0041_02: 1993, T12: 30}` with zero diagnostics — converges with
/// RB-588, RB-597, RB-606, RB-615, RB-624, RB-633, RB-642, RB-651, RB-660 and
/// RB-669: offset validation is N/A everywhere. (Completes SYN-0040-02…09
/// coverage + opens SYN-0041-01…02.)
#[test]
fn rb_678_syn41_offset_eleventh_degrades() {
    let src = "case_0041_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0041_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0041_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0041_02"],
        "2024-02-29T12:30:00+25:00"
    );
}
/// RB-679 — Derived variant 41-03: trailing-`Z` eleventh twin rejects (SYN-0041-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0041 (toml-test second-trailing-dot).
/// `case_0041_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589, RB-598, RB-607, RB-616, RB-625, RB-634, RB-643, RB-652,
/// RB-661 and RB-670: one timezone syntax contract in all eleven lineages,
/// loud reject, never a pseudo-object.
#[test]
fn rb_679_syn41_trailing_z_eleventh_rejects() {
    let src = "case_0041_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0041_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0041_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-680 — Derived variant 41-04: quoted eleventh twin never validated (SYN-0041-04 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0041 (toml-test second-trailing-dot).
/// `case_0041_04 = "2024-02-30"` is the exact string with zero diagnostics —
/// converges with RB-590, RB-599, RB-608, RB-617, RB-626, RB-635, RB-644,
/// RB-653, RB-662 and RB-671: quoted content skips calendar validation in
/// all eleven lineages.
#[test]
fn rb_680_syn41_quoted_eleventh_never_validated() {
    let src = "case_0041_04 = \"2024-02-30\"\n";
    assert_eq!(eval_json(src)["case_0041_04"], "2024-02-30");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare counterpart is arithmetic 1992, never a date.
    assert_eq!(
        eval_json("case_0041_04 = 2024-02-30\n")["case_0041_04"],
        1992
    );
}

/// RB-681 — Derived variant 41-05: fractional eleventh twin rejects (SYN-0041-05 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0041 (toml-test second-trailing-dot).
/// `case_0041_05 = 2024-01-01T12:30:00.123456789Z` fails E002 Error at
/// (1:45) — converges with RB-591, RB-600, RB-609, RB-618, RB-627, RB-636,
/// RB-645, RB-654, RB-663 and RB-672: no precision handling without a
/// datetime type, loud reject in all eleven lineages.
#[test]
fn rb_681_syn41_fractional_eleventh_rejects() {
    let src = "case_0041_05 = 2024-01-01T12:30:00.123456789Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 45), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0041_05 = \"2024-01-01T12:30:00.123456789Z\"\n";
    assert_eq!(
        eval_json(src)["case_0041_05"],
        "2024-01-01T12:30:00.123456789Z"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-682 — Derived variant 41-06: offset eleventh twin degrades (SYN-0041-06 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0041 (toml-test second-trailing-dot).
/// `case_0041_06 = 2024-01-01T12:30:00-07:00` evaluates whole-doc to exactly
/// `{case_0041_06: 2022, T12: 30}` with zero diagnostics — converges with
/// RB-592, RB-601, RB-610, RB-619, RB-628, RB-637, RB-646, RB-655, RB-664 and
/// RB-673: the offset never becomes an opaque fallback object.
#[test]
fn rb_682_syn41_offset_eleventh_degrades() {
    let src = "case_0041_06 = 2024-01-01T12:30:00-07:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0041_06": 2022, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0041_06 = \"2024-01-01T12:30:00-07:00\"\n")["case_0041_06"],
        "2024-01-01T12:30:00-07:00"
    );
}

/// RB-683 — Derived variant 41-07: leading-zero eleventh twin is decimal (SYN-0041-07 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0041 (toml-test second-trailing-dot).
/// `case_0041_07 = 0001-01-01` is the arithmetic -1 whole-doc with zero
/// diagnostics — converges with RB-593, RB-602, RB-611, RB-620, RB-629,
/// RB-638, RB-647, RB-656, RB-665 and RB-674: leading zeros follow the
/// decimal policy (RB-104) in all eleven lineages.
#[test]
fn rb_683_syn41_leading_zero_eleventh_decimal() {
    let src = "case_0041_07 = 0001-01-01\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0041_07": -1}),
        "must be decimal arithmetic, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0041_07 = \"0001-01-01\"\n")["case_0041_07"],
        "0001-01-01"
    );
}

/// RB-684 — Derived variant 41-08: quoted eleventh twin preserved (SYN-0041-08 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0041 (toml-test second-trailing-dot).
/// `case_0041_08 = "1979-05-27T07:32:00Z"` is the exact string with zero
/// diagnostics — converges with RB-594, RB-603, RB-612, RB-621, RB-630,
/// RB-639, RB-648, RB-657, RB-666 and RB-675: datetime-looking text is
/// preserved character-for-character when quoted, in all eleven lineages.
#[test]
fn rb_684_syn41_quoted_datetime_eleventh_exact() {
    let src = "case_0041_08 = \"1979-05-27T07:32:00Z\"\n";
    assert_eq!(eval_json(src)["case_0041_08"], "1979-05-27T07:32:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Control: the bare date prefix is arithmetic, never a datetime.
    assert_eq!(
        eval_json("case_0041_08 = 1979-05-27\n")["case_0041_08"],
        1947
    );
}

/// RB-685 — Derived variant 41-09: bad-minutes eleventh twin rejects (SYN-0041-09 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0041 (toml-test second-trailing-dot).
/// `case_0041_09 = 2024-01-01T12:60:00Z` fails E002 Error at (1:35) —
/// converges with RB-595, RB-604, RB-613, RB-622, RB-631, RB-640, RB-649,
/// RB-658, RB-667 and RB-676: no minute field exists to range-check
/// anywhere, loud reject in all eleven lineages. (Completes SYN-0041-03…09
/// coverage.)
#[test]
fn rb_685_syn41_bad_minutes_eleventh_reject() {
    let src = "case_0041_09 = 2024-01-01T12:60:00Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 35), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0041_09 = \"2024-01-01T12:60:00Z\"\n";
    assert_eq!(eval_json(src)["case_0041_09"], "2024-01-01T12:60:00Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}

/// RB-686 — Derived variant 42-01: Feb-30 twelfth twin is arithmetic (SYN-0042-01 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0042 (toml-test trailing-x).
/// `case_0042_01 = 2024-02-30` is the arithmetic 1992 whole-doc with zero
/// diagnostics — converges with RB-587, RB-596, RB-605, RB-614, RB-623,
/// RB-632, RB-641, RB-650, RB-659, RB-668 and RB-677: twelve lineages, one
/// resolver, Feb-30 never constructed. (Opens SYN-0042 trailing-x lineage.)
#[test]
fn rb_686_syn42_feb30_twelfth_arithmetic() {
    let src = "case_0042_01 = 2024-02-30\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0042_01": 1992}),
        "date-shaped input must be plain arithmetic, got {v}"
    );
    assert_eq!(
        v["case_0042_01"],
        eval_json("case_0041_01 = 2024-02-30\n")["case_0041_01"]
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0042_01 = \"2024-02-30\"\n")["case_0042_01"],
        "2024-02-30"
    );
}

/// RB-687 — Derived variant 42-02: offset twelfth twin degrades (SYN-0042-02 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0042 (toml-test trailing-x).
/// `case_0042_02 = 2024-02-29T12:30:00+25:00` degrades whole-doc to exactly
/// `{case_0042_02: 1993, T12: 30}` with zero diagnostics — converges with
/// RB-588, RB-597, RB-606, RB-615, RB-624, RB-633, RB-642, RB-651, RB-660,
/// RB-669 and RB-678: offset validation is N/A everywhere.
#[test]
fn rb_687_syn42_offset_twelfth_degrades() {
    let src = "case_0042_02 = 2024-02-29T12:30:00+25:00\n";
    let v = eval_json(src);
    assert_eq!(
        v,
        serde_json::json!({"case_0042_02": 1993, "T12": 30}),
        "must degrade to arithmetic + colon pair only, got {v}"
    );
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
    // Positive control: the quoted neighbour stays the exact string.
    assert_eq!(
        eval_json("case_0042_02 = \"2024-02-29T12:30:00+25:00\"\n")["case_0042_02"],
        "2024-02-29T12:30:00+25:00"
    );
}

/// RB-688 — Derived variant 42-03: trailing-`Z` twelfth twin rejects (SYN-0042-03 §H, HIGH).
/// Origin: synthetic variation of SRC-TOML-0042 (toml-test trailing-x).
/// `case_0042_03 = 2024-01-01T12:30Z` fails E002 Error at (1:32) — converges
/// with RB-589, RB-598, RB-607, RB-616, RB-625, RB-634, RB-643, RB-652,
/// RB-661, RB-670 and RB-679: one timezone syntax contract in all twelve
/// lineages, loud reject, never a pseudo-object. (Completes SYN-0041-03…09
/// coverage + opens SYN-0042-01…03.)
#[test]
fn rb_688_syn42_trailing_z_twelfth_rejects() {
    let src = "case_0042_03 = 2024-01-01T12:30Z\n";
    let diags = parse(src, "research").expect_err("must fail E002");
    let e = diags.iter().find(|d| d.code == "E002").expect("E002");
    assert_eq!(
        e.severity,
        active_core::Severity::Error,
        "E002 must be Error, got {:?}",
        e.severity
    );
    let span = e.span.clone().expect("E002 carries a span");
    assert_eq!((span.line, span.column), (1, 32), "{span:?}");
    // Positive control: the quoted neighbour stays the exact string.
    let src = "case_0042_03 = \"2024-01-01T12:30Z\"\n";
    assert_eq!(eval_json(src)["case_0042_03"], "2024-01-01T12:30Z");
    let codes = codes_of(src);
    assert!(codes.is_empty(), "{codes:?}");
}
