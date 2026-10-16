//! Schema generator tests (C-047, spec/converters.md §7).
//!
//! All five targets from one schema exercising every type,
//! constraints, defaults, nesting, closed tables, secrets, and
//! deprecations — plus the no-schema error, golden snapshots for
//! TS/Go, and a rustfmt syntax gate for the Rust output.

use solid_core::convert::{
    schema_to_go, schema_to_json_schema, schema_to_openapi, schema_to_rust, schema_to_typescript,
};
use solid_core::parse;

const SCHEMA_DOC: &str = r#"schema = {
  closed = true,
  port = Int(min = 1, max = 65535),
  host = String(min_len = 1, default = "localhost"),
  debug = Bool,
  timeout = Duration,
  size = Bytes,
  mode = Enum("a", "b"),
  tags = Array<String>,
  server = Table { host = String },
  token = Secret,
  old = String(deprecated = "use new"),
}
port = 8080
"#;

fn module() -> solid_core::Module {
    parse(SCHEMA_DOC, "generate").expect("schema must parse")
}

#[test]
fn json_schema_covers_types_constraints_and_flags() {
    let schema = schema_to_json_schema(&module()).expect("generate");
    assert_eq!(schema["type"], "object");
    let props = schema["properties"].as_object().expect("properties");
    assert_eq!(props["port"]["type"], "integer");
    assert_eq!(props["port"]["minimum"], 1);
    assert_eq!(props["port"]["maximum"], 65535);
    assert_eq!(props["host"]["minLength"], 1);
    assert_eq!(props["host"]["default"], "localhost");
    assert_eq!(props["mode"]["enum"], serde_json::json!(["a", "b"]));
    assert_eq!(props["tags"]["type"], "array");
    assert_eq!(props["tags"]["items"]["type"], "string");
    assert_eq!(props["server"]["type"], "object");
    assert_eq!(props["server"]["properties"]["host"]["type"], "string");
    assert_eq!(props["token"]["type"], "string");
    assert_eq!(schema["additionalProperties"], false);
    assert!(props["server"].get("additionalProperties").is_none());
    assert_eq!(props["old"]["deprecated"], true);
    assert_eq!(props["timeout"]["properties"]["ms"]["type"], "integer");
    // Required: everything without a static default, minus Nulls.
    let required = schema["required"].as_array().expect("required");
    let names: Vec<&str> = required.iter().map(|v| v.as_str().expect("name")).collect();
    assert!(names.contains(&"port"), "{names:?}");
    assert!(!names.contains(&"host"), "{names:?}");
}

#[test]
fn openapi_wraps_the_schema_under_components() {
    let openapi = schema_to_openapi(&module(), "Svc").expect("generate");
    assert_eq!(openapi["openapi"], "3.1.0");
    assert_eq!(openapi["components"]["schemas"]["Svc"]["type"], "object");
}

#[test]
fn typescript_golden_snapshot() {
    let ts = schema_to_typescript(&module(), "Config").expect("generate");
    for probe in [
        "export interface Config {",
        "port: number;",
        "host?: string;",
        "mode: \"a\" | \"b\";",
        "tags: string[];",
        "server: ConfigServer;",
        "token: string;",
        "export interface ConfigServer {",
        "/** @deprecated use new */",
    ] {
        assert!(ts.contains(probe), "missing {probe}:\n{ts}");
    }
}

#[test]
fn go_golden_snapshot() {
    let go = schema_to_go(&module(), "Config").expect("generate");
    for probe in [
        "type Config struct {",
        "Port int64 `json:\"port\"`",
        "Host string `json:\"host\"`",
        "Mode string `json:\"mode\"`",
        "Tags []string `json:\"tags\"`",
        "Server ConfigServer `json:\"server\"`",
        "type ConfigServer struct {",
        "// Deprecated: use new",
    ] {
        assert!(go.contains(probe), "missing {probe}:\n{go}");
    }
}

#[test]
fn rust_output_parses_as_valid_syntax() {
    let rust = schema_to_rust(&module(), "Config").expect("generate");
    for probe in [
        "pub struct Config {",
        "pub port: i64,",
        "pub host: Option<String>,",
        "pub mode: ConfigMode,",
        "pub tags: Vec<String>,",
        "pub server: ConfigServer,",
        "pub struct ConfigServer {",
        "pub enum ConfigMode {",
        "#[deprecated(note = \"use new\")]",
    ] {
        assert!(rust.contains(probe), "missing {probe}:\n{rust}");
    }
    // Syntax gate: rustfmt parses what serde would compile (plain
    // format run — success means the syntax is valid; --check would
    // additionally demand canonical formatting, which is not the
    // generated style).
    let dir = std::env::temp_dir().join("solid-gen-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("generated.rs");
    std::fs::write(&path, &rust).expect("write");
    let out = std::process::Command::new("rustfmt")
        .args(["--edition", "2021"])
        .arg(&path)
        .output()
        .expect("run rustfmt");
    assert!(
        out.status.success(),
        "generated Rust must parse: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn generation_fails_without_a_schema() {
    let module = parse("port = 1\n", "generate").expect("must parse");
    for result in [
        schema_to_json_schema(&module).map(|_| ()),
        schema_to_openapi(&module, "X").map(|_| ()),
        schema_to_typescript(&module, "X").map(|_| ()),
        schema_to_go(&module, "X").map(|_| ()),
        schema_to_rust(&module, "X").map(|_| ()),
    ] {
        assert!(result.is_err(), "schema-less documents must fail");
    }
}

#[test]
fn dynamic_bounds_skip_without_diagnostics() {
    // A computed bound has no static form: skipped, still valid.
    let module = parse(
        "let cap = 10\nschema = { port = Int(max = cap) }\nport = 1\n",
        "generate",
    )
    .expect("must parse");
    let schema = schema_to_json_schema(&module).expect("generate");
    assert!(schema["properties"]["port"].get("maximum").is_none());
}

#[test]
fn colliding_type_names_suffix_deterministically() {
    // `port` and `Port` tables derive the same base: references and
    // declarations must agree on the suffixed split.
    let module = parse(
        "schema = { port = Table { x = Int }, Port = Table { y = Int } }\n",
        "generate",
    )
    .expect("must parse");
    for output in [
        schema_to_typescript(&module, "Config").expect("ts"),
        schema_to_go(&module, "Config").expect("go"),
        schema_to_rust(&module, "Config").expect("rust"),
    ] {
        assert!(output.contains("ConfigPort"), "{output}");
        assert!(output.contains("ConfigPort_"), "{output}");
    }
}
