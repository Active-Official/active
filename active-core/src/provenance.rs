//! Hermetic provenance, content hashing, and lockfiles (C-050/C-051,
//! spec/reproducibility.md). Pure collection over the parsed module,
//! an environment snapshot, and the filesystem (hashes only):
//! evaluation itself performs no I/O.

use crate::ir::{Module, Node, NodeId};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// Locate a path relative to the root document's directory, using `/`
/// separators so a lockfile stays valid when the tree moves (L-P03).
/// Paths outside the root use `..` segments; only paths on an unrelated
/// root (a different Windows volume) stay absolute.
pub(crate) fn root_relative(path: &Path, root: Option<&Path>) -> String {
    if let Some(root) = root.filter(|root| !root.as_os_str().is_empty()) {
        if let Ok(rel) = path.strip_prefix(root) {
            // A stripped relative path never carries a root or prefix;
            // if it does, the two paths are not in the same frame.
            let mut clean = true;
            for component in rel.components() {
                if matches!(
                    component,
                    std::path::Component::RootDir | std::path::Component::Prefix(_)
                ) {
                    clean = false;
                    break;
                }
            }
            if clean && !rel.as_os_str().is_empty() {
                return slash_path(rel);
            }
        }
        // An include allowed outside the root still travels with the
        // tree: record it with `..` segments instead of an absolute
        // host path.
        if let Some(rel) = path_relative(root, path) {
            return slash_path(&rel);
        }
    }
    path.to_string_lossy().into_owned()
}

/// Render a path with `/` separators, preserving `..` segments.
fn slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            std::path::Component::ParentDir => Some("..".to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Lockfile format version. Version 2 adds `provenance.include_pins`
/// (L-P03); version 1 lockfiles still parse and resolve by content hash.
pub const LOCK_VERSION: u32 = 2;

/// How strictly an eval-time lockfile is applied (L-P03).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// A mismatched or missing pin is a hard error (E041).
    Enforce,
    /// A mismatch is an error; an include with no pin is a loud W017.
    Warn,
}

/// Include pins loaded from a lockfile, for eval-time resolution
/// (L-P03). `resolved` maps a root-relative identity recorded at lock
/// time to its pinned digests; `pins` maps a document spelling for pins
/// recorded without a resolved path. Matching prefers `resolved`, so one
/// spelling used from several directories stays unambiguous. `hashes`
/// admits a renamed include with identical content in warn mode.
#[derive(Debug, Clone)]
pub struct IncludeLock {
    pub resolved: BTreeMap<String, Vec<String>>,
    pub pins: BTreeMap<String, Vec<String>>,
    pub hashes: HashSet<String>,
    pub mode: LockMode,
    /// Whether the lockfile carried `provenance.include_pins` at all.
    /// Legacy v1 lockfiles do not; they resolve content hashes in warn
    /// mode because they cannot name the include graph.
    pub identity_pins: bool,
}

impl IncludeLock {
    /// Parse `provenance.include_pins` (L-P03) and fall back to
    /// `provenance.includes` so a v1 lockfile written before the pin
    /// section existed still admits content by hash. Legacy include
    /// paths are lockframe paths, not root-relative identities, so they
    /// contribute hashes only.
    pub fn parse(lock_text: &str) -> Result<Self, String> {
        let doc: toml::Value =
            toml::from_str(lock_text).map_err(|e| format!("unparseable lockfile: {e}"))?;
        let mut version = 1i64;
        if let Some(declared) = doc.get("version") {
            version = declared
                .as_integer()
                .ok_or_else(|| "lockfile version is not an integer".to_string())?;
            if version < 1 || version > i64::from(LOCK_VERSION) {
                return Err(format!(
                    "lockfile version {version} is not supported (this tool reads {LOCK_VERSION})"
                ));
            }
        }
        let mut resolved = BTreeMap::new();
        let mut pins = BTreeMap::new();
        let mut hashes = HashSet::new();
        let provenance = doc.get("provenance");
        if let Some(value) = provenance {
            if !value.is_table() {
                return Err("provenance is not a table".to_string());
            }
        }
        let include_pins = match provenance.and_then(|p| p.get("include_pins")) {
            Some(value) => Some(
                value
                    .as_array()
                    .ok_or_else(|| "provenance.include_pins is not an array".to_string())?,
            ),
            None => None,
        };
        let identity_pins = include_pins.is_some();
        if version >= 2 && include_pins.is_none() {
            return Err(
                "lockfile version 2 requires provenance.include_pins (re-run `active lock`)"
                    .to_string(),
            );
        }
        if version < 2 && include_pins.is_some() {
            return Err(
                "lockfile version 1 cannot carry provenance.include_pins; re-run `active lock`"
                    .to_string(),
            );
        }
        if let Some(entries) = include_pins {
            for entry in entries {
                let as_written = entry
                    .get("as")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "include pin has no string `as` spelling".to_string())?;
                let hash = valid_pin(entry)?;
                match entry.get("resolved") {
                    Some(value) => {
                        let path = value
                            .as_str()
                            .ok_or_else(|| "include pin `resolved` is not a string".to_string())?;
                        resolved
                            .entry(path.to_string())
                            .or_insert_with(Vec::new)
                            .push(hash.clone());
                    }
                    // A pin without a resolved path can only match by
                    // spelling; keeping it out of `resolved` is what
                    // makes an enforced lock refuse a new same-spelling
                    // edge.
                    None => pins
                        .entry(as_written.to_string())
                        .or_insert_with(Vec::new)
                        .push(hash.clone()),
                }
                hashes.insert(hash);
            }
        }
        let includes = match provenance.and_then(|p| p.get("includes")) {
            Some(value) => Some(
                value
                    .as_array()
                    .ok_or_else(|| "provenance.includes is not an array".to_string())?,
            ),
            None => None,
        };
        if let Some(entries) = includes {
            for entry in entries {
                entry
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "include entry has no string `path`".to_string())?;
                let hash = valid_pin(entry)?;
                // Legacy `includes` paths live in the lockfile frame and
                // never equal a document's root-relative identity, so
                // they contribute content hashes only.
                hashes.insert(hash);
            }
        }
        // A lockfile with identity pins can name the include graph, so
        // it enforces. A legacy lockfile cannot, and admits matching
        // content with a warning.
        let mode = if identity_pins {
            LockMode::Enforce
        } else {
            LockMode::Warn
        };
        Ok(Self {
            resolved,
            pins,
            hashes,
            mode,
            identity_pins,
        })
    }
}

/// Every content hash a lockfile names, through the canonical parser
/// (L-P04 GC roots): main, recorded includes, and identity pins. A lock
/// that names nothing - or that the evaluator would reject - is an
/// error, never an empty root set.
pub fn lock_roots(lock_text: &str) -> Result<HashSet<String>, String> {
    let lock = IncludeLock::parse(lock_text)?;
    let doc: toml::Value =
        toml::from_str(lock_text).map_err(|e| format!("unparseable lockfile: {e}"))?;
    let mut roots = lock.hashes;
    if let Some(main) = doc.get("main") {
        if !main.is_table() {
            return Err("lockfile `main` is not a table".to_string());
        }
        let hash = valid_pin(main).map_err(|e| format!("lockfile `main`: {e}"))?;
        roots.insert(hash);
    }
    if roots.is_empty() {
        return Err("lockfile names no sha256 values; refusing to use it as a GC root".to_string());
    }
    Ok(roots)
}

/// Read one 64-hex `sha256` value from a lockfile entry, refusing a
/// malformed pin instead of silently dropping it. Hex is stored
/// lowercase so every comparison is exact.
fn valid_pin(entry: &toml::Value) -> Result<String, String> {
    let hash = entry
        .get("sha256")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "lockfile pin has no string `sha256`".to_string())?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("lockfile pin is not 64 hex digits: {hash:?}"));
    }
    Ok(hash.to_ascii_lowercase())
}

/// One environment input declared by the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvInput {
    pub name: String,
    pub set: bool,
    /// Plain reads record the value verbatim; secret-bound reads
    /// record nothing here (the secret invariant wins). A read is
    /// secret-bound when a `Secret` node locates it, a profile makes
    /// it material by path, or its value matches an evaluator secret
    /// seed (a copy of a `let`-bound or computed secret).
    pub value: Option<String>,
    /// True when the name is secret-bound or its value matches a
    /// secret seed (`secret(env.X)` and `secret.env("X")` share the
    /// locator shape; the value match covers plain copies).
    pub secret: bool,
    /// Drift signal for secret-bound reads. Salted per lockfile
    /// (LANG-I-001): `sha256:<salt-hex>:<digest-hex>`, where the digest is
    /// over the salt bytes followed by the value. The salt is stored beside
    /// it, so a re-lock reproduces the bytes; salting defeats precomputed
    /// tables, never a targeted guess at a low-entropy value.
    pub value_hash: Option<String>,
    /// The per-lockfile salt used by `value_hash` (hex). Present exactly
    /// when the hash is.
    pub value_salt: Option<String>,
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
    /// Include pins by document spelling (L-P03): what the eval-time
    /// lockfile matches and what `active lock` writes.
    pub include_pins: Vec<crate::ir::IncludePin>,
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
            names.insert(crate::eval::canonical_env_name(name));
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
/// filesystem-canonicalized (which resolves 8.3 short-names on Windows and
/// the on-disk casing on case-insensitive filesystems). No case-folding
/// shortcut: on a case-sensitive filesystem `Main.active` and
/// `main.active` are two different files (LANG-H-005). Unresolvable paths
/// fall back to string equality.
fn same_file(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
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
pub fn hash_file(path: &str) -> Result<String, String> {
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
    collect_provenance_with_seeds(module, env, main_file, profile, None)
}

/// Provenance that also withholds an environment value whose text
/// matches an evaluator secret seed, even when its name is not
/// structurally material. A copy of a `let`-bound secret can otherwise
/// reach the lockfile's `[[provenance.env]]` value.
pub fn collect_provenance_with_seeds(
    module: &Module,
    env: &std::collections::HashMap<String, String>,
    main_file: Option<&Path>,
    profile: Option<&str>,
    seeds: Option<&crate::secret::SecretRedactor>,
) -> Result<Provenance, String> {
    collect_provenance_with_salts(module, env, main_file, profile, seeds, None)
}

/// A fresh per-lockfile salt (16 bytes, hex). A lockfile's salt is public
/// by design (it sits beside the hash); it defeats precomputed tables and
/// cross-lockfile correlation, never a targeted guess at a low-entropy
/// value (LANG-I-001). Without host entropy the salt is a fixed constant:
/// locking stays available, with only precomputation resistance lost.
fn fresh_salt_hex() -> String {
    let mut buf = [0u8; 16];
    if getrandom::getrandom(&mut buf).is_err() {
        return "00000000000000000000000000000000".to_string();
    }
    buf.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Verify a lockfile's recorded file hashes against disk (LANG-I-002c).
/// Fails closed on a missing or changed file. Version 1 lockfiles are
/// checked in full (main plus `provenance.includes`). Version 2 identity
/// pins govern include content at parse time (E041 per edge), so this
/// checks the main document only; use it before evaluation, and let the
/// pins report the include graph.
pub fn verify_lockfile_files(lock_text: &str) -> Result<(), String> {
    verify_lockfile_files_at(lock_text, None, None)
}

/// Largest file a drift check will hash, and the most entries it will
/// check. A lockfile is small and its inputs are documents; an
/// attacker-supplied sibling must not turn verification into an
/// unbounded read or fan-out.
const MAX_VERIFY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_VERIFY_ENTRIES: usize = 1024;

/// [`verify_lockfile_files`] with a base directory for relative recorded
/// paths and an optional expected main document. `base` is the
/// lockfile's own directory, so a lock created in the document directory
/// verifies from anywhere; a recorded main that names another file is
/// refused rather than silently checked alongside it.
pub fn verify_lockfile_files_at(
    lock_text: &str,
    base: Option<&Path>,
    expected_main: Option<&Path>,
) -> Result<(), String> {
    verify_lockfile_files_scoped(lock_text, base, expected_main, None)
}

/// [`verify_lockfile_files_at`] with a containment root: every recorded
/// path must resolve inside it. Implicitly discovered sibling lockfiles
/// use this so a rogue `active-lock.toml` cannot turn verification into a
/// read (and digest oracle) of arbitrary host paths.
pub fn verify_lockfile_files_scoped(
    lock_text: &str,
    base: Option<&Path>,
    expected_main: Option<&Path>,
    restrict_root: Option<&Path>,
) -> Result<(), String> {
    let doc: toml::Value =
        toml::from_str(lock_text).map_err(|e| format!("unparseable lockfile: {e}"))?;
    let version = doc.get("version").and_then(|v| v.as_integer()).unwrap_or(1);
    // Version 1 recorded paths in the process working directory (the old
    // tool's frame); version 2 records them relative to the lockfile.
    // Prefer the lockfile frame when that file exists, so a v1 lock made
    // from the document directory still works after a directory move,
    // and fall back to the cwd for a v1 lock made from a parent.
    let resolve = |p: &str| -> std::path::PathBuf {
        let raw = Path::new(p);
        if raw.is_relative() {
            if let Some(base) = base {
                let joined = base.join(raw);
                if version >= 2 || joined.exists() {
                    return joined;
                }
            }
            raw.to_path_buf()
        } else {
            raw.to_path_buf()
        }
    };
    if let (Some(expected), Some(recorded)) = (
        expected_main,
        doc.get("main")
            .and_then(|m| m.get("path"))
            .and_then(|v| v.as_str()),
    ) {
        let recorded = resolve(recorded);
        let same = match (
            std::fs::canonicalize(&recorded),
            std::fs::canonicalize(expected),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => same_file(&recorded.to_string_lossy(), &expected.to_string_lossy()),
        };
        if !same {
            return Err(format!(
                "lockfile is for '{}', not '{}'",
                recorded.display(),
                expected.display()
            ));
        }
    }
    // Version 2 identity pins govern the include graph; checking the
    // legacy include hashes first would preempt the per-edge E041 with an
    // uncoded drift error. Main drift is still checked here.
    let pins_govern_includes = version >= 2
        && doc
            .get("provenance")
            .and_then(|p| p.get("include_pins"))
            .is_some();
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Some(main) = doc.get("main") {
        if !main.is_table() {
            return Err("lockfile `main` is not a table".to_string());
        }
        let path = main
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "lockfile `main.path` is not a string".to_string())?;
        let hash = valid_pin(main).map_err(|e| format!("lockfile `main`: {e}"))?;
        entries.push((path.to_string(), hash));
    }
    if let Some(provenance) = doc.get("provenance") {
        if !provenance.is_table() {
            return Err("provenance is not a table".to_string());
        }
        if let Some(includes) = provenance.get("includes") {
            let includes = includes
                .as_array()
                .ok_or_else(|| "provenance.includes is not an array".to_string())?;
            for entry in includes {
                let path = entry
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "include entry has no string `path`".to_string())?;
                let hash = valid_pin(entry).map_err(|e| format!("include `{path}`: {e}"))?;
                if !pins_govern_includes {
                    entries.push((path.to_string(), hash));
                }
            }
        }
        if let Some(pins) = provenance.get("include_pins") {
            let pins = pins
                .as_array()
                .ok_or_else(|| "provenance.include_pins is not an array".to_string())?;
            for entry in pins {
                valid_pin(entry).map_err(|e| format!("include pin: {e}"))?;
            }
        }
    }
    if entries.is_empty() {
        if pins_govern_includes {
            return Ok(());
        }
        return Err("lockfile records no file hashes to verify".to_string());
    }
    if entries.len() > MAX_VERIFY_ENTRIES {
        return Err(format!(
            "lockfile records {} file hashes; the limit is {MAX_VERIFY_ENTRIES}",
            entries.len()
        ));
    }
    let mut drift = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut total_bytes = 0u64;
    for (path, want) in entries {
        let full = resolve(&path);
        if let Some(root) = restrict_root {
            if !path_inside(root, &full) {
                drift.push(format!("{}: outside the document tree", full.display()));
                continue;
            }
        }
        // The same file recorded twice costs one read, and the aggregate
        // stays bounded: a 1 MiB lock naming one 16 MiB file 1024 times
        // must not multiply the work.
        if !seen.insert(full.to_string_lossy().into_owned()) {
            continue;
        }
        match read_capped(&full) {
            Ok(bytes) => {
                total_bytes = total_bytes.saturating_add(bytes.len() as u64);
                if total_bytes > MAX_VERIFY_BYTES {
                    return Err(format!(
                        "lockfile verification exceeds the {MAX_VERIFY_BYTES} byte aggregate limit"
                    ));
                }
                let got = sha256_hex(&bytes);
                if !got.eq_ignore_ascii_case(&want) {
                    // Do not echo the disk digest: a recording that
                    // names an arbitrary path must not become an oracle.
                    drift.push(format!("{}: content changed", full.display()));
                }
            }
            Err(e) => drift.push(format!("{}: cannot read ({e})", full.display())),
        }
    }
    if drift.is_empty() {
        Ok(())
    } else {
        Err(format!("lockfile drift: {}", drift.join("; ")))
    }
}

/// Whether `path` resolves inside `root` (canonical or absolute fallback
/// on both sides, so a verbatim Windows prefix does not decide the
/// answer).
fn path_inside(root: &Path, path: &Path) -> bool {
    let root = root
        .canonicalize()
        .or_else(|_| std::path::absolute(root))
        .unwrap_or_else(|_| root.to_path_buf());
    let path = path
        .canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    path.starts_with(root)
}

/// Read one file for hashing under the verification size cap.
fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".to_string());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_VERIFY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_VERIFY_BYTES {
        return Err(format!(
            "larger than the {MAX_VERIFY_BYTES} byte verification limit"
        ));
    }
    Ok(bytes)
}

/// Extract `name -> value_salt` from a previously rendered lockfile so a
/// re-lock reproduces the salted hashes byte-for-byte (LANG-I-001).
/// Unparseable or saltless files yield an empty map.
pub fn env_salts_from_lock(text: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Ok(doc) = text.parse::<toml::Value>() else {
        return out;
    };
    let Some(env) = doc
        .get("provenance")
        .and_then(|p| p.get("env"))
        .and_then(|e| e.as_array())
    else {
        return out;
    };
    for entry in env {
        if let (Some(name), Some(salt)) = (
            entry.get("name").and_then(|v| v.as_str()),
            entry.get("value_salt").and_then(|v| v.as_str()),
        ) {
            out.insert(name.to_string(), salt.to_string());
        }
    }
    out
}

/// [`collect_provenance_with_seeds`] with per-name salts reused from a
/// previous lockfile (LANG-I-001): a re-lock of an unchanged document is
/// byte-identical, while a fresh lockfile gets fresh salts.
pub fn collect_provenance_with_salts(
    module: &Module,
    env: &std::collections::HashMap<String, String>,
    main_file: Option<&Path>,
    profile: Option<&str>,
    seeds: Option<&crate::secret::SecretRedactor>,
    salts: Option<&std::collections::HashMap<String, String>>,
) -> Result<Provenance, String> {
    // One salt per lockfile, generated lazily: a document with no
    // secret-bound read consumes no entropy and writes no salt.
    let mut file_salt: Option<String> = None;
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
                secret.insert(crate::eval::canonical_env_name(name));
            }
        }
    }
    let mut names = BTreeMap::new();
    for node in &module.arena {
        if let Node::Env { name, .. } = node {
            names.insert(crate::eval::canonical_env_name(name), ());
        }
    }
    let mut env_inputs = Vec::new();
    for (name, ()) in names {
        let is_secret = secret.contains(&name);
        match env.get(&name) {
            Some(value)
                if !is_secret && !seeds.is_some_and(|seeds| seeds.redact_text(value).is_some()) =>
            {
                env_inputs.push(EnvInput {
                    name: name.clone(),
                    set: true,
                    value: Some(value.clone()),
                    secret: false,
                    value_hash: None,
                    value_salt: None,
                })
            }
            Some(value) => {
                // Salted drift hash (LANG-I-001): reuse the lockfile's
                // salt when re-locking, otherwise one fresh salt per file.
                let salt = salts
                    .and_then(|known| known.get(&name).cloned())
                    .or_else(|| file_salt.clone())
                    .unwrap_or_else(|| {
                        let fresh = fresh_salt_hex();
                        file_salt = Some(fresh.clone());
                        fresh
                    });
                let mut payload = Vec::with_capacity(salt.len() + value.len());
                payload.extend_from_slice(salt.as_bytes());
                payload.extend_from_slice(value.as_bytes());
                env_inputs.push(EnvInput {
                    name: name.clone(),
                    set: true,
                    value: None,
                    secret: true,
                    value_hash: Some(format!("sha256:{salt}:{}", sha256_hex(&payload))),
                    value_salt: Some(salt),
                })
            }
            None => env_inputs.push(EnvInput {
                name: name.clone(),
                set: false,
                value: None,
                secret: is_secret,
                value_hash: None,
                value_salt: None,
            }),
        }
    }
    // Distinct stamped files minus the main file (canonical
    // identity — stamps may differ textually from the CLI path).
    let main_canon = main_file.and_then(|p| std::fs::canonicalize(p).ok());
    let main_label = main_file.map(|p| p.to_string_lossy().into_owned());
    // L-P03: reuse the digest of the bytes that were PARSED for each
    // include. Re-reading here would let `include_pins` (parsed bytes)
    // and `includes` (re-read bytes) disagree inside one lockfile when a
    // file changes between parse and provenance collection. The lock
    // then describes exactly what was evaluated; a later rewrite is
    // caught by the next eval's drift check. A file re-read as different
    // bytes during the same parse is a hard error.
    let mut stamp_hash: BTreeMap<String, String> = BTreeMap::new();
    for pin in &module.include_pins {
        match stamp_hash.get(&pin.stamp) {
            Some(existing) if existing != &pin.sha256 => {
                return Err(format!(
                    "include `{}` changed while parsing; re-run the lock",
                    pin.resolved
                ));
            }
            Some(_) => {}
            None => {
                stamp_hash.insert(pin.stamp.clone(), pin.sha256.clone());
            }
        }
    }
    // `include_pins` carry the exact parser stamp; node stamps use the
    // same string (canonical on disk, lexical-absolute under a loader),
    // so the parsed digest is reused without a re-read.
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
        let sha256 = match stamp_hash.get(&stamped) {
            Some(hash) => {
                // The digest is the parsed bytes (no check/use gap), but a
                // file that vanished after parsing still cannot be pinned.
                std::fs::metadata(&stamped).map_err(|e| {
                    format!("cannot read include file '{stamped}' for hashing: {e}")
                })?;
                hash.clone()
            }
            None => hash_file(&stamped)?,
        };
        includes.push(FileInput {
            path: stamped.clone(),
            sha256,
        });
    }
    let main = match main_file {
        Some(path) => {
            let label = path.to_string_lossy().into_owned();
            // Record the canonical path: a stdout lock is not rebased,
            // and a canonical identity verifies from any directory. A
            // file output rebases this to the lockframe.
            let recorded = main_canon
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| label.clone());
            Some(FileInput {
                path: recorded.clone(),
                sha256: std::fs::read(&recorded)
                    .map(|bytes| sha256_hex(&bytes))
                    .map_err(|e| format!("cannot read main file '{label}' for hashing: {e}"))?,
            })
        }
        None => None,
    };
    Ok(Provenance {
        env: env_inputs,
        includes,
        include_pins: module.include_pins.clone(),
        main,
        profile: profile.map(str::to_string),
    })
}

/// Content hash over the canonical redacted form: compact JSON with
/// sorted keys, secrets already replaced by `***SECRET***`
/// (`value_to_json`). Same tree → same `sha256:<hex>` on every
/// platform.
///
/// Caller obligation: pass a tree whose plain strings are already
/// redacted against the analyzed material profile (the CLI `lock` path
/// and the `debug --minimize` digest both do). `value_to_json` redacts
/// `Value::Secret` nodes; it cannot know that a plain `String` came from
/// an environment read, so an unredacted tree would hash the raw bytes.
/// Retained blob payloads are stripped first, so `--materialise-blobs`
/// cannot change a document's identity.
pub fn content_hash(value: &crate::ir::Value) -> String {
    let canonical = serde_json::to_string(&crate::convert::value_to_json(
        &crate::convert::strip_blob_payloads(value),
    ))
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
            payload,
        } => out.push(BlobRecord {
            path: path.to_string(),
            hash: format!(
                "blake3:{}",
                crate::blob::hash_hex(&crate::blob::verified_identity(
                    hash,
                    *tainted,
                    media_type,
                    payload.as_ref()
                ))
            ),
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

/// One file's path rewritten relative to a lockfile's directory, or an
/// absolute path when the two trees share no root component.
fn lockframe_path(base: &Path, recorded: &str) -> String {
    let target = Path::new(recorded);
    let absolute = std::path::absolute(target).ok();
    let Some(absolute) = absolute else {
        return recorded.to_string();
    };
    let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    match path_relative(base, &absolute) {
        Some(rel) => rel
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                std::path::Component::ParentDir => Some("..".to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
        None => absolute.to_string_lossy().into_owned(),
    }
}

/// `target` expressed relative to `base`, component-wise. `None` when the
/// paths share no leading component (different roots).
fn path_relative(base: &Path, target: &Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let base: Vec<Component> = base.components().collect();
    let target: Vec<Component> = target.components().collect();
    let mut shared = 0;
    while shared < base.len() && shared < target.len() && base[shared] == target[shared] {
        shared += 1;
    }
    if shared == 0 {
        return None;
    }
    let mut out = std::path::PathBuf::new();
    for _ in shared..base.len() {
        out.push("..");
    }
    for component in &target[shared..] {
        out.push(component.as_os_str());
    }
    Some(out)
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
    /// Include pins by document spelling (L-P03).
    pub include_pins: Vec<crate::ir::IncludePin>,
    pub blobs: Vec<BlobRecord>,
}

impl LockManifest {
    /// Rewrite main/include paths relative to `base`, with `/`
    /// separators, so a lockfile written into another directory
    /// verifies from that directory (L-P03). `include_pins[].resolved`
    /// stays root-relative: it is a document identity, not a lockframe
    /// path.
    pub fn rebase_paths(&mut self, base: &Path) {
        let base = std::path::absolute(base)
            .ok()
            .and_then(|p| std::fs::canonicalize(&p).ok().or(Some(p)));
        let Some(base) = base else {
            return;
        };
        if let Some(main) = &mut self.main {
            main.path = lockframe_path(&base, &main.path);
        }
        for include in &mut self.includes {
            include.path = lockframe_path(&base, &include.path);
        }
    }

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
            include_pins: provenance.include_pins,
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
                entry.insert(
                    "size".to_string(),
                    toml::Value::Integer(i64::try_from(blob.size).unwrap_or(i64::MAX)),
                );
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
                if let Some(salt) = &input.value_salt {
                    entry.insert("value_salt".to_string(), toml::Value::String(salt.clone()));
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
        {
            // Stable bytes: sort by (spelling, resolved, digest) and
            // collapse duplicate resolutions. Version 2 always emits the
            // section, even empty, so a lockfile that lost it is
            // distinguishable from a v1 legacy file.
            let mut pins: Vec<&crate::ir::IncludePin> = self.include_pins.iter().collect();
            pins.sort_by(|a, b| {
                (&a.as_written, &a.resolved, &a.sha256).cmp(&(
                    &b.as_written,
                    &b.resolved,
                    &b.sha256,
                ))
            });
            pins.dedup();
            let mut array = Vec::new();
            for pin in pins {
                let mut entry = toml::map::Map::new();
                entry.insert(
                    "as".to_string(),
                    toml::Value::String(pin.as_written.clone()),
                );
                entry.insert(
                    "resolved".to_string(),
                    toml::Value::String(pin.resolved.clone()),
                );
                entry.insert(
                    "sha256".to_string(),
                    toml::Value::String(pin.sha256.clone()),
                );
                array.push(toml::Value::Table(entry));
            }
            provenance.insert("include_pins".to_string(), toml::Value::Array(array));
        }
        if !provenance.is_empty() {
            doc.insert("provenance".to_string(), toml::Value::Table(provenance));
        }
        let header = format!(
            "# Active lockfile v{LOCK_VERSION} — generated by `active lock`. Do not edit.\n"
        );
        format!(
            "{header}{}",
            toml::to_string(&toml::Value::Table(doc)).unwrap_or_default()
        )
    }
}

#[cfg(all(test, target_os = "linux"))]
mod path_identity_tests {
    use super::same_file;

    #[test]
    fn case_differing_paths_are_distinct_files() {
        // LANG-H-005: on a case-sensitive filesystem these are two files.
        // The old eq_ignore_ascii_case shortcut conflated them.
        let dir = std::env::temp_dir().join(format!("active-same-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let upper = dir.join("Main.active");
        let lower = dir.join("main.active");
        std::fs::write(&upper, "a = 1\n").expect("write upper");
        std::fs::write(&lower, "b = 2\n").expect("write lower");
        let (a, b) = (upper.to_str().expect("utf8"), lower.to_str().expect("utf8"));
        assert!(!same_file(a, b), "distinct files must not compare equal");
        assert!(same_file(a, a));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
