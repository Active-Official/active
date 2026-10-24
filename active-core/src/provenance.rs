//! Hermetic provenance, content hashing, and lockfiles (C-050/C-051,
//! spec/reproducibility.md). Pure collection over the parsed module,
//! an environment snapshot, and the filesystem (hashes only):
//! evaluation itself performs no I/O.

use crate::ir::{Module, Node, NodeId};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// Lockfile format version.
pub const LOCK_VERSION: u32 = 1;

/// One environment input declared by the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvInput {
    pub name: String,
    pub set: bool,
    /// Plain reads record the value verbatim; secret-bound reads
    /// record nothing here (the secret invariant wins).
    pub value: Option<String>,
    /// True when a `Secret` node directly locates this variable
    /// (`secret(env.X)` and `secret.env("X")` share that shape).
    pub secret: bool,
    /// Drift signal for secret-bound reads (preimage-resistant).
    pub value_hash: Option<String>,
}

/// One file input: stamped path plus content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInput {
    pub path: String,
    pub sha256: String,
}

/// Provenance for one evaluation: environment, includes, main file,
/// and the effective profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub env: Vec<EnvInput>,
    pub includes: Vec<FileInput>,
    pub main: Option<FileInput>,
    pub profile: Option<String>,
}

/// Names bound for secrecy: every environment variable reachable
/// backward through the dependency graph from any `Secret` node.
/// Direct locators (`secret(env.X)`, `secret.env("X")`) are the
/// common case; transitive flows (`let h = env.H` +
/// `secret(h)`, interpolations) count too — a variable that feeds
/// a secret anywhere is secret everywhere (the conservative
/// direction: plain uses of the same variable withhold the value
/// as well, documented in spec §2).
fn secret_bound_names(module: &Module) -> HashSet<String> {
    use petgraph::Direction;
    let mut names = HashSet::new();
    let mut visited = HashSet::new();
    let mut stack = Vec::new();
    for (index, node) in module.arena.iter().enumerate() {
        if matches!(node, Node::Secret { .. }) {
            let id = NodeId(index as u32);
            if visited.insert(id) {
                stack.push(id);
            }
        }
    }
    while let Some(id) = stack.pop() {
        if let Some(Node::Env { name, .. }) = module.get(id) {
            names.insert(name.clone());
        }
        let Some(&index) = module.node_to_graph.get(&id) else {
            continue;
        };
        for neighbor in module.graph.neighbors_directed(index, Direction::Incoming) {
            let dep = module.graph[neighbor];
            if visited.insert(dep) {
                stack.push(dep);
            }
        }
    }
    names
}

/// Canonical identity for path comparison: exact, then
/// case-insensitive, then filesystem-canonicalized (8.3
/// short-names on Windows). Unresolvable paths fall back to string
/// equality.
fn same_file(a: &str, b: &str) -> bool {
    if a == b || a.eq_ignore_ascii_case(b) {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// SHA-256 hex of a file's contents, or an error naming the file
/// (never its contents — paths are safe to quote, data is not).
fn hash_file(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("cannot read include file '{path}' for hashing: {e}"))?;
    Ok(sha256_hex(&bytes))
}

/// Collect provenance for an evaluated module: every `Env` node in
/// the arena (declared inputs — the audit-safe superset of
/// attempted reads), every stamped include file, the main file, and
/// the effective profile. `env` is the same snapshot evaluation
/// used; `main_file` is the parsed path when file-based.
pub fn collect_provenance(
    module: &Module,
    env: &std::collections::HashMap<String, String>,
    main_file: Option<&Path>,
    profile: Option<&str>,
) -> Result<Provenance, String> {
    let mut secret = secret_bound_names(module);
    // Profile selection is part of the secret analysis: a profile may
    // place `env.T` at a path the document marks material, and the
    // graph walk alone cannot see that. Every env read whose node is
    // material under the effective selector withholds its value.
    let profile_analysis = crate::secret::SecretProfile::analyze_with_profile(module, profile);
    for (index, node) in module.arena.iter().enumerate() {
        if let Node::Env { name, .. } = node {
            if profile_analysis.node_is_secret(NodeId(index as u32))
                || profile_analysis.env_is_material(name)
            {
                secret.insert(name.clone());
            }
        }
    }
    let mut names = BTreeMap::new();
    for node in &module.arena {
        if let Node::Env { name, .. } = node {
            names.insert(name.clone(), ());
        }
    }
    let mut env_inputs = Vec::new();
    for (name, ()) in names {
        let is_secret = secret.contains(&name);
        match env.get(&name) {
            Some(value) if !is_secret => env_inputs.push(EnvInput {
                name: name.clone(),
                set: true,
                value: Some(value.clone()),
                secret: false,
                value_hash: None,
            }),
            Some(value) => env_inputs.push(EnvInput {
                name: name.clone(),
                set: true,
                value: None,
                secret: true,
                value_hash: Some(format!("sha256:{}", sha256_hex(value.as_bytes()))),
            }),
            None => env_inputs.push(EnvInput {
                name: name.clone(),
                set: false,
                value: None,
                secret: is_secret,
                value_hash: None,
            }),
        }
    }
    // Distinct stamped files minus the main file (canonical
    // identity — stamps may differ textually from the CLI path).
    let main_canon = main_file.and_then(|p| std::fs::canonicalize(p).ok());
    let main_label = main_file.map(|p| p.to_string_lossy().into_owned());
    let mut seen: HashSet<String> = HashSet::new();
    let mut includes = Vec::new();
    let mut stamps: Vec<String> = module.node_files.values().cloned().collect();
    stamps.sort();
    for stamped in stamps {
        if !seen.insert(stamped.clone()) {
            continue;
        }
        let is_main = match (&main_canon, &main_label) {
            (Some(canon), _) => std::fs::canonicalize(&stamped)
                .map(|c| c == *canon)
                .unwrap_or_else(|_| same_file(&stamped, &canon.to_string_lossy())),
            (None, Some(label)) => same_file(&stamped, label),
            (None, None) => false,
        };
        if is_main {
            continue;
        }
        includes.push(FileInput {
            path: stamped.clone(),
            sha256: hash_file(&stamped)?,
        });
    }
    let main = match main_file {
        Some(path) => {
            let label = path.to_string_lossy().into_owned();
            Some(FileInput {
                path: label.clone(),
                sha256: std::fs::read(&label)
                    .map(|bytes| sha256_hex(&bytes))
                    .map_err(|e| format!("cannot read main file '{label}' for hashing: {e}"))?,
            })
        }
        None => None,
    };
    Ok(Provenance {
        env: env_inputs,
        includes,
        main,
        profile: profile.map(str::to_string),
    })
}

/// Content hash over the canonical redacted form: compact JSON with
/// sorted keys, secrets already replaced by `***SECRET***`
/// (`value_to_json`). Same tree → same `sha256:<hex>` on every
/// platform. Raw secret values never enter the hash.
pub fn content_hash(value: &crate::ir::Value) -> String {
    let canonical = serde_json::to_string(&crate::convert::value_to_json(value))
        .unwrap_or_else(|_| "null".to_string());
    format!("sha256:{}", sha256_hex(canonical.as_bytes()))
}

/// One content-addressed identity recorded in a lockfile
/// (spec/blob.md §7, spec/reproducibility.md §4): path, hash, size,
/// media type, taint. A tainted identity redacts its media type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRecord {
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub media_type: String,
    pub tainted: bool,
}

/// Collect every blob identity in an evaluated tree (pre-order;
/// tables iterate in source order and the caller sorts by path).
fn collect_blobs(value: &crate::ir::Value, path: &str, out: &mut Vec<BlobRecord>) {
    use crate::ir::Value;
    match value {
        Value::Blob {
            hash,
            size,
            media_type,
            tainted,
            ..
        } => out.push(BlobRecord {
            path: path.to_string(),
            hash: format!("blake3:{}", crate::blob::hash_hex(hash)),
            size: *size,
            media_type: crate::secret::exposed_media_type(*tainted, media_type).to_string(),
            tainted: *tainted,
        }),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_blobs(item, &format!("{path}[{index}]"), out);
            }
        }
        Value::Table(map) => {
            for (key, item) in map {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                collect_blobs(item, &child, out);
            }
        }
        _ => {}
    }
}

/// A complete lockfile record: hash, toolchain, provenance, and blob
/// identities. Non-exhaustive: consumers construct it through
/// [`LockManifest::new`], and future fields stay additive.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockManifest {
    pub toolchain: String,
    pub content_hash: String,
    pub profile: Option<String>,
    pub main: Option<FileInput>,
    pub env: Vec<EnvInput>,
    pub includes: Vec<FileInput>,
    pub blobs: Vec<BlobRecord>,
}

impl LockManifest {
    pub fn new(toolchain: String, value: &crate::ir::Value, provenance: Provenance) -> Self {
        let profile = provenance.profile.clone();
        let mut blobs = Vec::new();
        collect_blobs(value, "", &mut blobs);
        blobs.sort_by(|a, b| a.path.cmp(&b.path));
        Self {
            toolchain,
            content_hash: content_hash(value),
            profile,
            main: provenance.main,
            env: provenance.env,
            includes: provenance.includes,
            blobs,
        }
    }

    /// Render the `active-lock.toml` document (spec §4).
    pub fn render(&self) -> String {
        let mut doc = toml::map::Map::new();
        doc.insert(
            "version".to_string(),
            toml::Value::Integer(LOCK_VERSION as i64),
        );
        doc.insert(
            "toolchain".to_string(),
            toml::Value::String(self.toolchain.clone()),
        );
        doc.insert(
            "content_hash".to_string(),
            toml::Value::String(self.content_hash.clone()),
        );
        if let Some(profile) = &self.profile {
            doc.insert("profile".to_string(), toml::Value::String(profile.clone()));
        }
        if let Some(main) = &self.main {
            let mut table = toml::map::Map::new();
            table.insert("path".to_string(), toml::Value::String(main.path.clone()));
            table.insert(
                "sha256".to_string(),
                toml::Value::String(main.sha256.clone()),
            );
            doc.insert("main".to_string(), toml::Value::Table(table));
        }
        if !self.blobs.is_empty() {
            let mut array = Vec::new();
            for blob in &self.blobs {
                let mut entry = toml::map::Map::new();
                entry.insert("path".to_string(), toml::Value::String(blob.path.clone()));
                entry.insert("hash".to_string(), toml::Value::String(blob.hash.clone()));
                entry.insert("size".to_string(), toml::Value::Integer(blob.size as i64));
                entry.insert(
                    "media_type".to_string(),
                    toml::Value::String(blob.media_type.clone()),
                );
                entry.insert("tainted".to_string(), toml::Value::Boolean(blob.tainted));
                array.push(toml::Value::Table(entry));
            }
            doc.insert("blobs".to_string(), toml::Value::Array(array));
        }
        let mut provenance = toml::map::Map::new();
        if !self.env.is_empty() {
            let mut array = Vec::new();
            for input in &self.env {
                let mut entry = toml::map::Map::new();
                entry.insert("name".to_string(), toml::Value::String(input.name.clone()));
                entry.insert("set".to_string(), toml::Value::Boolean(input.set));
                if let Some(value) = &input.value {
                    entry.insert("value".to_string(), toml::Value::String(value.clone()));
                }
                if input.secret {
                    entry.insert("secret".to_string(), toml::Value::Boolean(true));
                }
                if let Some(hash) = &input.value_hash {
                    entry.insert("value_hash".to_string(), toml::Value::String(hash.clone()));
                }
                array.push(toml::Value::Table(entry));
            }
            provenance.insert("env".to_string(), toml::Value::Array(array));
        }
        if !self.includes.is_empty() {
            let mut array = Vec::new();
            for include in &self.includes {
                let mut entry = toml::map::Map::new();
                entry.insert(
                    "path".to_string(),
                    toml::Value::String(include.path.clone()),
                );
                entry.insert(
                    "sha256".to_string(),
                    toml::Value::String(include.sha256.clone()),
                );
                array.push(toml::Value::Table(entry));
            }
            provenance.insert("includes".to_string(), toml::Value::Array(array));
        }
        if !provenance.is_empty() {
            doc.insert("provenance".to_string(), toml::Value::Table(provenance));
        }
        let header = "# Active lockfile v1 — generated by `active lock`. Do not edit.\n";
        format!(
            "{header}{}",
            toml::to_string(&toml::Value::Table(doc)).unwrap_or_default()
        )
    }
}
