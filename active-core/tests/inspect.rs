//! Schema introspection tests (CHECKLIST C.007, PROPOSAL §3.2).
//!
//! The versioned JSON surface agents query instead of parsing schema
//! source: fields, types, constraints, defaults, descriptions,
//! deprecations, removed_in, requires targets, enum members, table
//! closure, and schema evolution metadata (version, min_reader,
//! min_writer).

use active_core::convert::{schema_inspect, INSPECT_VERSION};
use active_core::parse;

const SCHEMA_DOC: &str = r#"schema = {
  closed = true,
  port = Int(min = 1, max = 65535),
  host = String(min_len = 1, default = "localhost", description = "Public DNS name."),
  contact = String(format = "email"),
  mode = String,
  replicas = Int(requires = "mode"),
  color = Enum("red", "green", descriptions = ["stop", "go"]),
  old = String(deprecated = "use fqdn"),
  gone = String(removed_in = "1.2.0"),
  token = Secret,
  server = Table { host = String, closed = true },
  tags = Array<String(min_len = 2)>,
}
"#;

fn fields() -> serde_json::Value {
    let module = parse(SCHEMA_DOC, "inspect").expect("schema must parse");
    schema_inspect(&module).expect("inspect")
}

fn by_path(doc: &serde_json::Value, path: &str) -> serde_json::Value {
    doc["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|f| f["path"] == path)
        .unwrap_or_else(|| panic!("missing field {path}"))
        .clone()
}

#[test]
fn inspect_reports_version_and_field_list() {
    let doc = fields();
    assert_eq!(doc["version"], INSPECT_VERSION);
    assert_eq!(doc["version"], 2);
    assert!(doc["fields"].as_array().expect("fields").len() >= 9);
}

#[test]
fn inspect_covers_types_constraints_defaults() {
    let doc = fields();
    let port = by_path(&doc, "port");
    assert_eq!(port["type"], "Int");
    assert_eq!(port["required"], true);
    assert_eq!(port["constraints"]["min"], 1);
    assert_eq!(port["constraints"]["max"], 65535);
    let host = by_path(&doc, "host");
    assert_eq!(host["required"], false);
    assert_eq!(host["default"], "localhost");
    assert_eq!(host["description"], "Public DNS name.");
    assert_eq!(host["constraints"]["min_len"], 1);
}

#[test]
fn inspect_covers_new_schema_metadata() {
    let doc = fields();
    let contact = by_path(&doc, "contact");
    assert_eq!(contact["constraints"]["format"], "email");
    let replicas = by_path(&doc, "replicas");
    assert_eq!(replicas["requires"], "mode");
    let color = by_path(&doc, "color");
    assert_eq!(color["type"], "Enum");
    assert_eq!(color["members"], serde_json::json!(["red", "green"]));
    assert_eq!(
        color["member_descriptions"],
        serde_json::json!(["stop", "go"])
    );
    let old = by_path(&doc, "old");
    assert_eq!(old["deprecated"], "use fqdn");
    let token = by_path(&doc, "token");
    assert_eq!(token["type"], "Secret");
}

#[test]
fn inspect_covers_nested_tables() {
    let doc = fields();
    let server = by_path(&doc, "server");
    assert_eq!(server["kind"], "table");
    assert_eq!(server["closed"], true);
    let host = by_path(&doc, "server.host");
    assert_eq!(host["type"], "String");
    assert_eq!(host["required"], true);
}

#[test]
fn inspect_covers_array_elements() {
    let doc = fields();
    let tags = by_path(&doc, "tags");
    assert_eq!(tags["kind"], "array");
    assert_eq!(tags["element"], "String");
    let element = by_path(&doc, "tags[]");
    assert_eq!(element["type"], "String");
    assert_eq!(element["constraints"]["min_len"], 2);
}

#[test]
fn inspect_entries_share_one_shape() {
    let doc = fields();
    let keys = [
        "path",
        "name",
        "kind",
        "type",
        "required",
        "default",
        "constraints",
        "description",
        "deprecated",
        "requires",
        "members",
        "member_descriptions",
        "closed",
        "element",
    ];
    for entry in doc["fields"].as_array().expect("fields") {
        let obj = entry.as_object().expect("entry");
        for key in keys {
            assert!(obj.contains_key(key), "entry {} lacks {key}", obj["path"]);
        }
    }
}

#[test]
fn inspect_fails_without_a_schema() {
    let module = parse("port = 1\n", "inspect").expect("must parse");
    assert!(schema_inspect(&module).is_err());
}

#[test]
fn inspect_covers_evolution_metadata() {
    // PROPOSAL §2.4: schema-level version/min_reader/min_writer plus
    // per-field removed_in travel through the inspector for agents.
    let module = parse(
        "schema = {\n  version = \"1.2.0\",\n  min_reader = \"1.0.0\",\n  gone = String(removed_in = \"1.3.0\"),\n}\n",
        "inspect",
    )
    .expect("schema must parse");
    let doc = schema_inspect(&module).expect("inspect");
    assert_eq!(doc["schema_version"], "1.2.0");
    assert_eq!(doc["min_reader"], "1.0.0");
    assert_eq!(doc["min_writer"], serde_json::Value::Null);
    let gone = by_path(&doc, "gone");
    assert_eq!(gone["removed_in"], "1.3.0");
    assert_eq!(gone["type"], "String");
}

// ------------------------------------------------------------------
// Path inspection (TOOLING INSPECT.001-005)
// ------------------------------------------------------------------

fn inspect_fixture() -> (active_core::Module, active_core::Value) {
    let src = "schema = { port = Int(min = 1, max = 65535, description = \"listen port\"), token = String(description = \"auth\") }\nport = 8080\ntoken = secret(\"hunter2-inspect\")\nid = blob(\"text/plain\", \"hi\")\n";
    let module = active_core::parse(src, "inspect-value").expect("parse");
    let (value, _) = active_core::Engine::new()
        .evaluate(&module)
        .expect("evaluate");
    (module, value)
}

#[test]
fn inspect_path_reports_schema_and_value() {
    let (module, value) = inspect_fixture();
    let report = active_core::convert::inspect_path(&module, &value, "port").expect("inspect");
    assert_eq!(report["path"], "port");
    assert_eq!(report["kind"], "Int");
    assert_eq!(report["value"], 8080);
    assert_eq!(report["schema"]["type"], "Int");
    assert_eq!(report["schema"]["description"], "listen port");
    let constraints = report["schema"]["constraints"]
        .as_array()
        .expect("constraints");
    let texts: Vec<&str> = constraints.iter().filter_map(|v| v.as_str()).collect();
    assert!(texts.contains(&"min 1"), "{texts:?}");
    assert!(texts.contains(&"max 65535"), "{texts:?}");
}

#[test]
fn inspect_path_redacts_secret_material() {
    let (module, value) = inspect_fixture();
    let report = active_core::convert::inspect_path(&module, &value, "token").expect("inspect");
    assert_eq!(report["kind"], "Secret");
    assert_eq!(report["tainted"], true);
    assert_eq!(report["value"], "***SECRET***");
    let text = active_core::convert::render_inspection_text(&report);
    assert!(text.contains("***SECRET***"), "{text}");
    assert!(
        !text.contains("hunter2"),
        "raw secret in text output: {text}"
    );
    assert!(!report.to_string().contains("hunter2"));
}

#[test]
fn inspect_path_reports_content_addressed_identity() {
    let (module, value) = inspect_fixture();
    let report = active_core::convert::inspect_path(&module, &value, "id").expect("inspect");
    assert_eq!(report["kind"], "Blob");
    let blob = &report["content_addressed"];
    assert!(blob["hash"].as_str().expect("hash").starts_with("blake3:"));
    assert_eq!(blob["size"], 2);
    assert_eq!(blob["media_type"], "text/plain");
    assert_eq!(blob["tainted"], false);
}

#[test]
fn inspect_path_unknown_path_fails_closed() {
    let (module, value) = inspect_fixture();
    let err =
        active_core::convert::inspect_path(&module, &value, "missing.key").expect_err("must fail");
    assert!(err.contains("missing.key"), "{err}");
}

#[test]
fn inspect_path_redacts_secret_sources_and_interpolations() {
    use std::collections::HashMap;
    let src = "x = \"hunter2-source\"\np = env.INSPECT_PASS\nk = secret(x)\nj = secret(p)\nmsg = \"prefix ${p}\"\nnode_id = 1\nport = node_id\n";
    let module = active_core::parse(src, "inspect-source").expect("parse");
    let mut env = HashMap::new();
    env.insert("INSPECT_PASS".to_string(), "ENVSECRET-inspect".to_string());
    let (value, _) = active_core::Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    for path in ["x", "p", "msg"] {
        let report = active_core::convert::inspect_path(&module, &value, path).expect("inspect");
        assert_eq!(report["tainted"], true, "path {path}: {report}");
        assert_eq!(report["value"], "***SECRET***", "path {path}: {report}");
        let rendered = report.to_string();
        assert!(
            !rendered.contains("hunter2"),
            "path {path} leaked: {rendered}"
        );
        assert!(
            !rendered.contains("ENVSECRET"),
            "path {path} leaked: {rendered}"
        );
    }
    // A real key whose name starts with `node` is provenance, not an
    // internal arena label.
    let report = active_core::convert::inspect_path(&module, &value, "port").expect("inspect");
    assert_eq!(report["value"], 1);
    assert_eq!(
        report["dependencies"],
        serde_json::json!(["node_id"]),
        "dependency provenance: {report}"
    );
}

#[test]
fn inspect_path_keeps_benign_siblings_of_a_secret() {
    let src = "x = \"hunter2-sib\"\nk = secret(x)\nname = \"svc\"\n";
    let module = active_core::parse(src, "inspect-siblings").expect("parse");
    let (value, _) = active_core::Engine::new()
        .evaluate(&module)
        .expect("evaluate");
    let report = active_core::convert::inspect_path(&module, &value, "name").expect("inspect");
    assert_eq!(report["value"], "svc");
    assert_eq!(report["tainted"], false);
}

#[test]
fn inspect_taint_model_covers_shadowing_and_transforms() {
    // Fields win ties against `let` names; patch/profile/schema-default
    // paths carry the transform expression into the evaluated tree.
    fn check(src: &str, var: &str, value: &str, path: &str) {
        let module = active_core::parse(src, "inspect-transform").expect("parse");
        let mut env = std::collections::HashMap::new();
        env.insert(var.to_string(), value.to_string());
        let (evaluated, _) = active_core::Engine::hermetic()
            .with_env(env)
            .evaluate(&module)
            .expect("evaluate");
        let report =
            active_core::convert::inspect_path(&module, &evaluated, path).expect("inspect");
        assert_eq!(report["tainted"], true, "{src}: {report}");
        assert_eq!(report["value"], "***SECRET***", "{src}: {report}");
        assert!(!report.to_string().contains(value), "{src}: {report}");
    }
    check(
        "let x = \"safe\"\nx = env.R12SHADOW\nk = secret(x)\n",
        "R12SHADOW",
        "ENVSECRET-shadow",
        "x",
    );
    check(
        "p = env.R12PASS\nk = secret(p)\n@patch(\"out\", p)\nout = \"original\"\n",
        "R12PASS",
        "ENVSECRET-patch",
        "out",
    );
    check(
        "p = env.R12PROF\nk = secret(p)\nactive_profile = \"prod\"\nprofile \"prod\" { extra = p }\n",
        "R12PROF",
        "ENVSECRET-prof",
        "extra",
    );
    check(
        "p = env.R12FILL\nk = secret(p)\nschema = { extra = String(default = p) }\n",
        "R12FILL",
        "ENVSECRET-fill",
        "extra",
    );
}

#[test]
fn inspect_provenance_keeps_real_node_prefixed_keys() {
    let src = "node1 = 1\nnode_id = 2\nport = node1\nid = node_id\n";
    let module = active_core::parse(src, "inspect-node-keys").expect("parse");
    let (value, _) = active_core::Engine::new()
        .evaluate(&module)
        .expect("evaluate");
    let port = active_core::convert::inspect_path(&module, &value, "port").expect("inspect");
    assert_eq!(port["dependencies"], serde_json::json!(["node1"]), "{port}");
    let id = active_core::convert::inspect_path(&module, &value, "id").expect("inspect");
    assert_eq!(id["dependencies"], serde_json::json!(["node_id"]), "{id}");
}

#[test]
fn inspect_taint_model_covers_nested_transforms_and_precedence() {
    fn check(src: &str, var: &str, value: &str, path: &str) {
        let module = active_core::parse(src, "inspect-nested").expect("parse");
        let mut env = std::collections::HashMap::new();
        env.insert(var.to_string(), value.to_string());
        let (evaluated, _) = active_core::Engine::hermetic()
            .with_env(env)
            .evaluate(&module)
            .expect("evaluate");
        let report =
            active_core::convert::inspect_path(&module, &evaluated, path).expect("inspect");
        assert_eq!(report["tainted"], true, "{src} path {path}: {report}");
        assert!(!report.to_string().contains(value), "{src}: {report}");
    }
    // A patch value that is a table: the subtree is addressable.
    check(
        "let p = env.R13P\nk = secret(p)\n@patch(\"out\", { a = p })\nout = { a = \"orig\" }\n",
        "R13P",
        "ENVSECRET-patch-table",
        "out.a",
    );
    check(
        "let p = env.R13N\nk = secret(p)\n@patch(\"newblock\", { a = p })\n",
        "R13N",
        "ENVSECRET-newblock",
        "newblock.a",
    );
    // A profile nested table.
    check(
        "let p = env.R13PR\nk = secret(p)\nactive_profile = \"prod\"\nprofile \"prod\" { server { extra = p } }\nserver = { extra = \"orig\" }\n",
        "R13PR",
        "ENVSECRET-profile-nested",
        "server.extra",
    );
    // Patches apply after profiles, so the patch value wins.
    check(
        "let p = env.R13PX\nk = secret(p)\nactive_profile = \"prod\"\nprofile \"prod\" { x = \"benign\" }\n@patch(\"x\", p)\nx = \"orig\"\n",
        "R13PX",
        "ENVSECRET-patch-wins",
        "x",
    );
}

#[test]
fn inspect_does_not_taint_from_an_inactive_profile() {
    let src = "let p = env.R13DEV\nk = secret(p)\nactive_profile = \"dev\"\nprofile \"dev\" { y = \"ok\" }\nprofile \"prod\" { x = p }\nx = \"benign2\"\n";
    let module = active_core::parse(src, "inspect-profile").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("R13DEV".to_string(), "ENVSECRET-dev".to_string());
    let (evaluated, _) = active_core::Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let report = active_core::convert::inspect_path(&module, &evaluated, "x").expect("inspect");
    assert_eq!(report["tainted"], false, "{report}");
    assert_eq!(report["value"], "benign2", "{report}");
}

#[test]
fn inspect_container_flag_reflects_marked_children() {
    let src =
        "let p = env.R13FLAG\nk = secret(p)\n@patch(\"out.a\", p)\nout = { a = \"orig\", b = 2 }\n";
    let module = active_core::parse(src, "inspect-flag").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("R13FLAG".to_string(), "ENVSECRET-flag".to_string());
    let (evaluated, _) = active_core::Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let report = active_core::convert::inspect_path(&module, &evaluated, "out").expect("inspect");
    assert_eq!(report["tainted"], true, "{report}");
    assert_eq!(report["value"]["a"], "***SECRET***", "{report}");
    assert_eq!(report["value"]["b"], 2, "{report}");
}

#[test]
fn inspect_redacts_the_media_of_a_data_tainted_blob() {
    let src = "p = env.R13BLOB\nk = secret(p)\nf = blob(\"text/plain\", p)\n";
    let module = active_core::parse(src, "inspect-blob-data").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("R13BLOB".to_string(), "ENVSECRET-blob".to_string());
    let (evaluated, _) = active_core::Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let report = active_core::convert::inspect_path(&module, &evaluated, "f").expect("inspect");
    assert_eq!(report["tainted"], true, "{report}");
    assert_eq!(
        report["content_addressed"]["media_type"], "***SECRET***",
        "{report}"
    );
}

#[test]
fn analysis_terminates_on_a_cyclic_binding() {
    // A self-referential `let` container must not grow paths forever;
    // the LSP analyzes every keystroke.
    use std::sync::mpsc;
    use std::time::Duration;
    let (module, _) = active_core::parse_recovering(
        "let t = { a = t }\n",
        "cycle",
        active_core::ParseOptions::default(),
    );
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = active_core::secret::SecretProfile::analyze(&module);
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("analysis must terminate on a cyclic binding");
}

#[test]
fn inspect_fails_closed_on_a_computed_profile_selector() {
    // `active_profile` from an env read cannot be resolved statically,
    // so the profile's paths join a taint-only fallback: a base value
    // that the selected profile replaces must not certify as benign.
    for (src, selected) in [
        (
            "let p = env.R14COMP\nk = secret(p)\nactive_profile = env.R14PROF\nprofile \"prod\" { x = p }\nx = \"benign\"\n",
            "prod",
        ),
        (
            "let p = env.R14COMP\nlet suffix = \"od\"\nk = secret(p)\nactive_profile = \"pr${suffix}\"\nprofile \"prod\" { x = p }\nx = \"benign\"\n",
            "unused",
        ),
        // Two profiles, benign LAST: the union must not let the last
        // write hide the earlier material path.
        (
            "let p = env.R14COMP\nk = secret(p)\nactive_profile = env.R14PROF\nprofile \"b\" { x = p }\nprofile \"a\" { x = \"benign\" }\nx = \"base\"\n",
            "b",
        ),
    ] {
        let module = active_core::parse(src, "inspect-computed-profile").expect("parse");
        let mut env = std::collections::HashMap::new();
        env.insert("R14COMP".to_string(), "ENVSECRET-computed".to_string());
        env.insert("R14PROF".to_string(), selected.to_string());
        let (evaluated, _) = active_core::Engine::hermetic()
            .with_env(env)
            .evaluate(&module)
            .expect("evaluate");
        let report =
            active_core::convert::inspect_path(&module, &evaluated, "x").expect("inspect");
        assert_eq!(report["tainted"], true, "{src}: {report}");
        assert!(
            !report.to_string().contains("ENVSECRET"),
            "{src}: {report}"
        );
    }
}

#[test]
fn inspect_follows_rename_targets() {
    let src = "let p = env.R14REN\nk = secret(p)\na = p\n@rename(\"a\", \"b\")\n";
    let module = active_core::parse(src, "inspect-rename").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("R14REN".to_string(), "ENVSECRET-rename".to_string());
    let (evaluated, _) = active_core::Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let report = active_core::convert::inspect_path(&module, &evaluated, "b").expect("inspect");
    assert_eq!(report["tainted"], true, "{report}");
    assert_eq!(report["value"], "***SECRET***", "{report}");
    // The old path no longer resolves in the evaluated tree.
    assert!(
        active_core::convert::inspect_path(&module, &evaluated, "a").is_err(),
        "renamed source path must be gone"
    );
}

#[test]
fn inspect_follows_nested_renames_and_array_removals() {
    // The evaluator resolves `@rename` destinations in the source
    // parent and shifts array indices on `@remove`; the taint map must
    // mirror both.
    for (src, path, env_name) in [
        (
            "let p = env.R15REN\nk = secret(p)\nouter = { a = p, z = 1 }\n@rename(\"outer.a\", \"b\")\n",
            "outer.b",
            "R15REN",
        ),
        (
            "let p = env.R15MISS\nk = secret(p)\na = { x = p }\n@rename(\"nope\", \"a\")\n",
            "a.x",
            "R15MISS",
        ),
        (
            "let p = env.R15ARR\nk = secret(p)\na = [\"benign\", p]\n@remove(\"a[0]\")\n",
            "a[0]",
            "R15ARR",
        ),
        (
            "let p = env.R15NEST\nk = secret(p)\na = { b = [\"benign\", p] }\n@remove(\"a.b[0]\")\n",
            "a.b[0]",
            "R15NEST",
        ),
    ] {
        let module = active_core::parse(src, "inspect-transform").expect("parse");
        let mut env = std::collections::HashMap::new();
        env.insert(env_name.to_string(), "ENVSECRET-transform".to_string());
        let (evaluated, _) = active_core::Engine::hermetic()
            .with_env(env)
            .evaluate(&module)
            .expect("evaluate");
        let report =
            active_core::convert::inspect_path(&module, &evaluated, path).expect("inspect");
        assert_eq!(report["tainted"], true, "{src}: {report}");
        assert_eq!(report["value"], "***SECRET***", "{src}: {report}");
    }
}

#[test]
fn computed_profile_selector_survives_patches() {
    // A computed selector's material union must follow Set and Rename.
    for (src, path) in [
        (
            "let p = env.R15PATCH\nk = secret(p)\nactive_profile = env.R15PROF\nprofile \"b\" { x = p }\nx = \"base\"\n@patch(\"x\", p)\n",
            "x",
        ),
        (
            "let p = env.R15PATCH\nk = secret(p)\nactive_profile = env.R15PROF\nprofile \"b\" { x = p }\nx = \"base\"\n@rename(\"x\", \"z\")\n",
            "z",
        ),
    ] {
        let module = active_core::parse(src, "inspect-patch-union").expect("parse");
        let mut env = std::collections::HashMap::new();
        env.insert("R15PATCH".to_string(), "ENVSECRET-patch-union".to_string());
        env.insert("R15PROF".to_string(), "b".to_string());
        let (evaluated, _) = active_core::Engine::hermetic()
            .with_env(env)
            .evaluate(&module)
            .expect("evaluate");
        let report =
            active_core::convert::inspect_path(&module, &evaluated, path).expect("inspect");
        assert_eq!(report["tainted"], true, "{src}: {report}");
        assert!(!report.to_string().contains("ENVSECRET"), "{src}: {report}");
    }
}

#[test]
fn inspect_reports_a_tainted_blob_as_tainted() {
    let src = "f = blob(\"text/plain\", secret(\"hunter2-blob\"))\n";
    let module = active_core::parse(src, "inspect-blob").expect("parse");
    let (value, _) = active_core::Engine::new()
        .evaluate(&module)
        .expect("evaluate");
    let report = active_core::convert::inspect_path(&module, &value, "f").expect("inspect");
    assert_eq!(report["tainted"], true);
    assert_eq!(report["content_addressed"]["tainted"], true);
    assert!(!report.to_string().contains("hunter2"));
}
