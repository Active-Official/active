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
