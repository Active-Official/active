//! Packaging metadata tests (C-056, spec/packaging.md).
//!
//! Guards the crates.io surface on the real shipped path: parses the
//! four workspace manifests (plus the VS Code extension manifest and
//! TextMate grammar) and pins every field publication needs. Positive
//! only by design — malformed manifests are cargo's own rejection
//! domain (`cargo package` in CI proves the negative continuously).

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .to_path_buf()
}

fn read_toml(name: &str) -> toml::Value {
    let path = workspace_root().join(name).join("Cargo.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.parse::<toml::Value>()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn package_table<'v>(manifest: &'v toml::Value, name: &str) -> &'v toml::Table {
    manifest
        .get("package")
        .and_then(|p| p.as_table())
        .unwrap_or_else(|| panic!("{name}: no [package]"))
}

fn str_field<'v>(table: &'v toml::Table, manifest: &str, field: &str) -> &'v str {
    table
        .get(field)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("{manifest}: [package].{field} missing or not a string"))
}

#[test]
fn workspace_version_is_single_sourced() {
    let root: toml::Value = {
        let path = workspace_root().join("Cargo.toml");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("root manifest: {e}"))
            .parse()
            .expect("root manifest parses")
    };
    let version = root
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .expect("workspace version");
    // The inter-crate version spec duplicates [workspace.package]
    // (cargo forbids version.workspace inside [workspace.dependencies]):
    // pin the duplication so they cannot drift.
    let dep_version = root
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.get("active-core"))
        .and_then(|d| d.get("version"))
        .and_then(|v| v.as_str())
        .expect("active-core version spec");
    assert_eq!(dep_version, version);
    for name in [
        "active",
        "active-core",
        "active-cli",
        "active-fmt",
        "active-lsp",
    ] {
        let manifest = read_toml(name);
        let table = package_table(&manifest, name);
        // Unified release: every crate inherits the workspace version.
        assert_eq!(
            table
                .get("version")
                .and_then(|v| v.get("workspace"))
                .and_then(|v| v.as_bool()),
            Some(true),
            "{name}: version must be workspace-inherited"
        );
    }
}

#[test]
fn publish_metadata_is_complete() {
    // Package names follow G:\Active (plain `active-core` belongs to
    // an unrelated 2020 crate): pin the frozen mapping.
    let expectations = [
        ("active", "active"),
        ("active-core", "active-core"),
        ("active-cli", "active-cli"),
        ("active-fmt", "active-fmt"),
        ("active-lsp", "active-lsp"),
    ];
    for (dir, package) in expectations {
        let manifest = read_toml(dir);
        let table = package_table(&manifest, dir);
        assert_eq!(str_field(table, dir, "name"), package);
        for field in [
            "description",
            "license",
            "authors",
            "repository",
            "homepage",
            "documentation",
        ] {
            assert!(
                manifest.get("package").and_then(|p| p.get(field)).is_some(),
                "{dir}: [package].{field} missing"
            );
        }
        for field in ["keywords", "categories"] {
            let items = table
                .get(field)
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| panic!("{dir}: [package].{field} missing"));
            assert!(!items.is_empty(), "{dir}: [package].{field} empty");
        }
        // The readme path resolves to a real file (relative to the
        // crate directory, as cargo resolves it).
        let readme = str_field(table, dir, "readme");
        let readme_path = workspace_root().join(dir).join(readme);
        assert!(
            readme_path.is_file(),
            "{dir}: readme missing at {}",
            readme_path.display()
        );
    }
}

#[test]
fn vscode_extension_manifest_is_consistent() {
    let path = workspace_root()
        .join("editors")
        .join("vscode")
        .join("package.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let manifest: serde_json::Value = serde_json::from_str(&text).expect("package.json parses");
    let language = &manifest["contributes"]["languages"][0];
    assert_eq!(language["id"], "active");
    assert!(
        language["extensions"]
            .as_array()
            .is_some_and(|exts| exts.iter().any(|e| e == ".active")),
        "must claim .active: {language}"
    );
    let grammar = &manifest["contributes"]["grammars"][0];
    assert_eq!(grammar["language"], "active");
    assert_eq!(grammar["scopeName"], "source.active");
    // Extension version tracks the workspace version (release
    // checklist bumps both together).
    let root: toml::Value = {
        let path = workspace_root().join("Cargo.toml");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("root manifest: {e}"))
            .parse()
            .expect("root manifest parses")
    };
    let version = root
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .expect("workspace version");
    assert_eq!(manifest["version"], version);
    // The grammar file the manifest points at parses too.
    let grammar_path = workspace_root()
        .join("editors")
        .join("vscode")
        .join(grammar["path"].as_str().expect("grammar path"));
    let grammar_text = std::fs::read_to_string(&grammar_path)
        .unwrap_or_else(|e| panic!("{}: {e}", grammar_path.display()));
    let grammar_json: serde_json::Value =
        serde_json::from_str(&grammar_text).expect("grammar parses");
    assert_eq!(grammar_json["scopeName"], "source.active");
}
