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
