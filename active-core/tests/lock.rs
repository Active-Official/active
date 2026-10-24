//! Hermeticity, provenance, hashing, and lockfiles (C-050/C-051,
//! spec/reproducibility.md).
//!
//! Snapshot enforcement (ambient invisible, injected visible),
//! manifest shape (plain/secret/unset vars, includes, profile,
//! vanishing files), hash stability and redaction, lockfile round
//! trips, and golden output.

use active_core::provenance::{collect_provenance, content_hash, LockManifest};
use active_core::{parse, Engine};
use std::collections::HashMap;

fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn eval_with(src: &str, env: HashMap<String, String>) -> active_core::Value {
    let module = parse(src, "lock").expect("must parse");
    Engine::new()
        .with_env(env)
        .evaluate(&module)
        .expect("must evaluate")
        .0
}

// ------------------------------------------------------------------
// Hermeticity: the injected snapshot is the only environment.
// ------------------------------------------------------------------

#[test]
fn lockfile_records_blob_identities_and_redacts_tainted_media() {
    let src = "m = env.R14BLOB\nk = secret(m)\ntainted = blob(m, \"data\")\nplain = blob(\"text/plain\", \"hi\")\n";
    let module = parse(src, "lock-blobs").expect("must parse");
    let env = env_map(&[("R14BLOB", "ENVSECRET-media")]);
    let engine = Engine::hermetic().with_env(env.clone());
    let (value, _) = engine.evaluate(&module).expect("must evaluate");
    let provenance = collect_provenance(&module, &engine.env, None, None).expect("provenance");
    let manifest = LockManifest::new("active-test".to_string(), &value, provenance);
    let rendered = manifest.render();
    assert!(rendered.contains("[[blobs]]"), "{rendered}");
    assert!(rendered.contains("path = \"plain\""), "{rendered}");
    assert!(rendered.contains("path = \"tainted\""), "{rendered}");
    assert!(
        rendered.contains("media_type = \"***SECRET***\""),
        "tainted media not redacted: {rendered}"
    );
    assert!(
        !rendered.contains("ENVSECRET"),
        "lockfile leaked material: {rendered}"
    );
    // The lockfile must reparse and be byte-identical on a rerun.
    let parsed: toml::Value = toml::from_str(&rendered).expect("lockfile parses");
    let blobs = parsed["blobs"].as_array().expect("[[blobs]] array");
    assert_eq!(blobs.len(), 2, "{rendered}");
    assert!(blobs.iter().any(|b| b["path"].as_str() == Some("plain")));
    assert_eq!(manifest.render(), rendered, "render must be deterministic");
}

#[test]
fn ambient_environment_is_invisible_under_an_explicit_snapshot() {
    // Even with the variable set in the process ambient, an empty
    // snapshot evaluates the default: no implicit reads, ever.
    let name = "ACTIVE_LOCK_PROBE_AMBIENT";
    let prior = std::env::var(name).ok();
    std::env::set_var(name, "ambient-should-not-leak");
    let src = format!("greeting = env.{name} ?: \"d\"\n");
    let value = eval_with(&src, HashMap::new());
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("\"d\""), "{json}");
    assert!(!json.contains("ambient-should-not-leak"), "{json}");
    match prior {
        Some(value) => std::env::set_var(name, value),
        None => std::env::remove_var(name),
    }
}

#[test]
fn injected_environment_is_visible() {
    let value = eval_with(
        "greeting = env.ACTIVE_LOCK_PROBE_SET\n",
        env_map(&[("ACTIVE_LOCK_PROBE_SET", "hello")]),
    );
    let json = active_core::convert::value_to_pretty_json(&value);
    assert!(json.contains("\"hello\""), "{json}");
}

// ------------------------------------------------------------------
// Manifest collection.
// ------------------------------------------------------------------

#[test]
fn manifest_records_plain_secret_and_unset_vars() {
    let module = parse(
        "host = env.ACTIVE_LOCK_HOST ?: \"h\"\ntoken = secret.env(\"ACTIVE_LOCK_PW\")\nmissing = env.ACTIVE_LOCK_MISSING_XYZ\n",
        "lock",
    )
    .expect("must parse");
    let env = env_map(&[
        ("ACTIVE_LOCK_HOST", "db.internal"),
        ("ACTIVE_LOCK_PW", "s3cr3t-lock"),
    ]);
    let provenance = collect_provenance(&module, &env, None, None).expect("provenance");
    assert!(provenance.main.is_none());
    assert!(provenance.includes.is_empty());
    assert!(provenance.profile.is_none());
    let by_name = |name: &str| {
        provenance
            .env
            .iter()
            .find(|input| input.name == name)
            .unwrap_or_else(|| panic!("{name} recorded: {:?}", provenance.env))
            .clone()
    };
    // Plain reads record values verbatim.
    let host = by_name("ACTIVE_LOCK_HOST");
    assert!(host.set && !host.secret);
    assert_eq!(host.value.as_deref(), Some("db.internal"));
    // Secret-bound reads withhold the value but keep a drift hash.
    let pw = by_name("ACTIVE_LOCK_PW");
    assert!(pw.set && pw.secret);
    assert_eq!(pw.value, None);
    let hash = pw.value_hash.expect("drift hash");
    assert!(hash.starts_with("sha256:"), "{hash}");
    // Unset vars record absence.
    let missing = by_name("ACTIVE_LOCK_MISSING_XYZ");
    assert!(!missing.set);
    // The manifest never carries the live secret.
    let debug = format!("{provenance:?}");
    assert!(!debug.contains("s3cr3t-lock"), "{debug}");
}

#[test]
fn transitive_secret_flows_are_secret_bound() {
    // `H` reaches the secret through a binding, not a direct
    // locator: still secret (conservative direction), even though
    // nothing else reads it plainly here.
    let module =
        parse("let h = env.ACTIVE_LOCK_H\ntoken = secret(h)\n", "lock").expect("must parse");
    let env = env_map(&[("ACTIVE_LOCK_H", "s3cr3t-transitive")]);
    let provenance = collect_provenance(&module, &env, None, None).expect("provenance");
    let entry = provenance
        .env
        .iter()
        .find(|input| input.name == "ACTIVE_LOCK_H")
        .expect("recorded");
    assert!(entry.set && entry.secret, "{entry:?}");
    assert_eq!(entry.value, None);
    let debug = format!("{provenance:?}");
    assert!(!debug.contains("s3cr3t-transitive"), "{debug}");
}

#[test]
fn manifest_collects_includes_and_main_file() {
    let dir = std::env::temp_dir().join("active-lock-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let lib = dir.join("locklib.active");
    let main = dir.join("lockmain.active");
    std::fs::write(&lib, "host = \"db\"\n").expect("write lib");
    std::fs::write(&main, "include \"locklib.active\"\nport = 1\n").expect("write main");
    let module = active_core::parse_file(&main, Default::default()).expect("must parse");
    let provenance =
        collect_provenance(&module, &HashMap::new(), Some(&main), None).expect("provenance");
    assert_eq!(provenance.includes.len(), 1, "{provenance:?}");
    assert!(
        provenance.includes[0].path.ends_with("locklib.active"),
        "{provenance:?}"
    );
    assert_eq!(provenance.includes[0].sha256.len(), 64);
    let main_input = provenance.main.expect("main recorded");
    assert!(main_input.path.ends_with("lockmain.active"));
    assert_eq!(main_input.sha256.len(), 64);
}

#[test]
fn manifest_records_the_effective_profile() {
    let module = parse(
        "port = 1\nprofile \"prod\" {\n  port = 2\n}\nactive_profile = \"prod\"\n",
        "lock",
    )
    .expect("must parse");
    let provenance =
        collect_provenance(&module, &HashMap::new(), None, Some("prod")).expect("provenance");
    assert_eq!(provenance.profile.as_deref(), Some("prod"));
}

#[test]
fn vanishing_includes_are_hard_errors() {
    let dir = std::env::temp_dir().join("active-lock-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let lib = dir.join("lockgone.active");
    let main = dir.join("lockgonemain.active");
    std::fs::write(&lib, "host = \"db\"\n").expect("write lib");
    std::fs::write(&main, "include \"lockgone.active\"\nport = 1\n").expect("write main");
    let module = active_core::parse_file(&main, Default::default()).expect("must parse");
    std::fs::remove_file(&lib).expect("delete lib");
    let err =
        collect_provenance(&module, &HashMap::new(), Some(&main), None).expect_err("must fail");
    assert!(err.contains("lockgone.active"), "{err}");
}

// ------------------------------------------------------------------
// Hashing.
// ------------------------------------------------------------------

#[test]
fn content_hash_is_stable_redacted_and_hex() {
    let a = eval_with("port = 1\ntoken = secret(\"s3cr3t-a\")\n", HashMap::new());
    let b = eval_with("port = 1\ntoken = secret(\"s3cr3t-b\")\n", HashMap::new());
    let c = eval_with("port = 2\n", HashMap::new());
    let (ha, hb, hc) = (content_hash(&a), content_hash(&b), content_hash(&c));
    // Deterministic…
    assert_eq!(ha, content_hash(&a));
    // …over the redacted tree (differing secrets, same hash)…
    assert_eq!(ha, hb);
    // …sensitive to real changes…
    assert_ne!(ha, hc);
    // …and well-formed.
    for hash in [&ha, &hb, &hc] {
        assert!(hash.starts_with("sha256:"), "{hash}");
        assert_eq!(hash.len(), 7 + 64, "{hash}");
        assert!(hash[7..].chars().all(|c| c.is_ascii_hexdigit()), "{hash}");
    }
}

// ------------------------------------------------------------------
// Lockfile rendering.
// ------------------------------------------------------------------

#[test]
fn lockfile_round_trips_through_toml() {
    let module =
        parse("host = env.ACTIVE_LOCK_HOST ?: \"h\"\nport = 1\n", "lock").expect("must parse");
    let (value, _) = Engine::new()
        .with_env(env_map(&[("ACTIVE_LOCK_HOST", "db")]))
        .evaluate(&module)
        .expect("must evaluate");
    let provenance =
        collect_provenance(&module, &env_map(&[("ACTIVE_LOCK_HOST", "db")]), None, None)
            .expect("provenance");
    let manifest = LockManifest::new(
        "active-cli TEST / active-core TEST".to_string(),
        &value,
        provenance,
    );
    let rendered = manifest.render();
    assert!(rendered.contains("version = 1"), "{rendered}");
    assert!(rendered.contains("content_hash = \"sha256:"), "{rendered}");
    assert!(rendered.contains("value = \"db\""), "{rendered}");
    let reparsed: toml::Value = toml::from_str(&rendered).expect("valid lockfile TOML");
    assert_eq!(reparsed["version"], toml::Value::Integer(1));
    assert!(reparsed["content_hash"]
        .as_str()
        .expect("hash")
        .starts_with("sha256:"));
}
