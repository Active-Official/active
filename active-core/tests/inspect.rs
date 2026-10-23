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
