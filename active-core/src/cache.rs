//! Content-keyed evaluation cache (L-P29).
//!
//! The key is a keyed BLAKE3 MAC over the document's source hash, every
//! include's content hash, the mode, the effective profile, the resolved
//! budget, the output variant, and the values of the environment
//! variables the document names. A change to any input misses; a hit is
//! the exact rendered result of the same inputs.
//!
//! The MAC key lives in a per-user file outside the cache directory, so
//! a directory listing does not let another user compute the name for
//! chosen inputs or confirm a guessed environment value. Entries carry
//! the v2 receipt as their provenance record.
//!
//! Local first: `EvalCache` is a directory of JSON entries. A shared
//! backend can implement the same `get`/`put` shape later; it needs the
//! same key on every machine.

use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::diag::Diagnostic;
use crate::eval::Budget;
use crate::ir::{Mode, Module, Node};
use serde::{Deserialize, Serialize};

/// Bump when the entry shape or key derivation changes; older entries
/// miss.
pub const CACHE_VERSION: u32 = 3;
/// Largest entry the cache will read or write.
pub const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;
/// Environment override for the per-user MAC key file.
pub const KEY_ENV: &str = "ACTIVE_CACHE_KEY_FILE";
/// Length of the MAC key.
pub const KEY_BYTES: usize = 32;

/// One rendered evaluation, keyed by its inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub version: u32,
    pub key: String,
    /// Pretty JSON of the redacted (and possibly coned) value.
    pub json: String,
    /// YAML/TOML renderings; absent when the format could not render,
    /// in which case the error is stored so a hit reproduces it exactly.
    pub yaml: Option<String>,
    #[serde(default)]
    pub yaml_error: Option<String>,
    pub toml: Option<String>,
    #[serde(default)]
    pub toml_error: Option<String>,
    /// Diagnostics the TOML renderer raised (the other renderers raise
    /// none), replayed so a hit reports the same output.
    #[serde(default)]
    pub export_diagnostics: Vec<Diagnostic>,
    /// Evaluation diagnostics, replayed so exit status and reporting
    /// match the run that filled the entry.
    pub diagnostics: Vec<Diagnostic>,
    /// The v2 receipt for this result (L-T06): the provenance record.
    pub receipt: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalCache {
    root: PathBuf,
    /// The uid the cache must belong to, taken from the per-user key
    /// file. `None` skips the ownership check (unit tests, non-Unix).
    owner: Option<u32>,
}

impl EvalCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            owner: None,
        }
    }

    /// Require every entry and the root to belong to `owner`.
    pub fn with_owner(mut self, owner: Option<u32>) -> Self {
        self.owner = owner;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn check_root(&self) -> Result<(), String> {
        check_root_dir(&self.root, self.owner)
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, String> {
        if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("not a cache key: {key:?}"));
        }
        Ok(self.root.join(format!("{}.json", key.to_ascii_lowercase())))
    }

    /// Read a validated entry. A version/key mismatch is a miss, so a
    /// renamed or stale entry cannot serve a result under another key.
    ///
    /// Only a regular file is read: a symlink, FIFO, or device is a
    /// miss, so a planted non-file cannot block or exhaust the reader.
    pub fn get(&self, key: &str) -> Result<Option<CacheEntry>, String> {
        self.check_root()?;
        let path = self.path_for(key)?;
        let link = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(format!(
                    "cannot inspect cache entry {}: {e}",
                    path.display()
                ))
            }
        };
        if !link.file_type().is_file() {
            return Err(format!(
                "cache entry {} is not a regular file",
                path.display()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if link.permissions().mode() & 0o077 != 0 {
                return Err(format!(
                    "cache entry {} is readable by other users",
                    path.display()
                ));
            }
        }
        let file = std::fs::File::open(&path)
            .map_err(|e| format!("cannot open cache entry {}: {e}", path.display()))?;
        // Re-check the OPENED handle, not only the path: a swapped
        // symlink or a different file must not be read.
        let opened = file
            .metadata()
            .map_err(|e| format!("cannot inspect cache entry {}: {e}", path.display()))?;
        if !opened.is_file() {
            return Err(format!(
                "cache entry {} is not a regular file",
                path.display()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if opened.permissions().mode() & 0o077 != 0 {
                return Err(format!(
                    "cache entry {} is readable by other users",
                    path.display()
                ));
            }
            let owner = match self.owner {
                Some(owner) => owner,
                None => std::fs::metadata(&self.root)
                    .map_err(|e| format!("cannot inspect cache root {}: {e}", self.root.display()))?
                    .uid(),
            };
            if opened.uid() != owner {
                return Err(format!(
                    "cache entry {} does not belong to the cache owner",
                    path.display()
                ));
            }
        }
        if opened.len() > MAX_ENTRY_BYTES {
            return Err(format!(
                "cache entry {} exceeds the {MAX_ENTRY_BYTES} byte limit",
                path.display()
            ));
        }
        let mut text = String::new();
        file.take(MAX_ENTRY_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|e| format!("cannot read cache entry {}: {e}", path.display()))?;
        if text.len() as u64 > MAX_ENTRY_BYTES {
            return Err(format!(
                "cache entry {} exceeds the {MAX_ENTRY_BYTES} byte limit",
                path.display()
            ));
        }
        let entry: CacheEntry = serde_json::from_str(&text)
            .map_err(|e| format!("cache entry {} is malformed: {e}", path.display()))?;
        if entry.version != CACHE_VERSION || !entry.key.eq_ignore_ascii_case(key) {
            return Ok(None);
        }
        // A complete entry always has a JSON rendering and one outcome
        // for each other format (a rendering or its error). Anything
        // else is damage, not a result to serve.
        let yaml_complete = entry.yaml.is_some() != entry.yaml_error.is_some();
        let toml_complete = entry.toml.is_some() != entry.toml_error.is_some();
        if entry.json.is_empty() || !yaml_complete || !toml_complete {
            return Err(format!("cache entry {} is incomplete", path.display()));
        }
        Ok(Some(entry))
    }

    /// Publish an entry atomically. The cache is best-effort: callers
    /// decide whether a write failure is fatal.
    pub fn put(&self, entry: &CacheEntry) -> Result<(), String> {
        if entry.version != CACHE_VERSION {
            return Err("cache entry version mismatch".to_string());
        }
        self.check_root()?;
        let path = self.path_for(&entry.key)?;
        // Create with the mode atomically, then validate the directory
        // that now exists. Never chmod a path after the fact: a swap to
        // a symlink would redirect the chmod.
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&self.root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("cannot create cache {}: {e}", self.root.display())),
        }
        self.check_root()?;
        let text =
            serde_json::to_string(entry).map_err(|e| format!("cannot render cache entry: {e}"))?;
        if text.len() as u64 > MAX_ENTRY_BYTES {
            return Err(format!(
                "cache entry exceeds the {MAX_ENTRY_BYTES} byte limit"
            ));
        }
        let tmp = self.root.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            monotonic_suffix()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| format!("cannot create cache entry {}: {e}", tmp.display()))?;
        file.write_all(text.as_bytes()).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot write cache entry {}: {e}", tmp.display())
        })?;
        drop(file);
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot publish cache entry {}: {e}", path.display())
        })
    }
}

/// Environment variables the document names through `env.NAME` or
/// `secret.env(NAME)`.
///
/// Both read the environment: `env.NAME` inserts the value, and
/// `secret.env(NAME)` seeds the redactor with it, so a copy of the value
/// elsewhere in the tree redacts in one environment and prints in
/// another. Every named variable is therefore an input to the output,
/// not just the plain reads. Names fold to uppercase exactly as
/// evaluation folds them.
pub fn referenced_env_names(module: &Module) -> BTreeSet<String> {
    module
        .arena
        .iter()
        .filter_map(|node| match node {
            Node::Env { name, .. } => Some(crate::eval::canonical_env_name(name)),
            _ => None,
        })
        .collect()
}

/// The cache key for an evaluation: a keyed BLAKE3 MAC over the inputs.
///
/// `variant` names anything else that changes the OUTPUT (input path,
/// cone path, materialisation, reader version, resilient repairs,
/// capability); the format is chosen from one entry, so it is not in the
/// key. `None` means the module carries no source bytes (a host-built
/// tree), so there is no stable identity to key on.
/// The key enumerates every input; a struct wrapper would not make the
/// call site clearer and churns the public signature.
#[allow(clippy::too_many_arguments)]
pub fn eval_key(
    module: &Module,
    profile: Option<&str>,
    budget: &Budget,
    mode: Mode,
    variant: &str,
    env: &HashMap<String, String>,
    key: &[u8; KEY_BYTES],
    pure: bool,
) -> Option<String> {
    if module.source_sha256.is_empty() {
        return None;
    }
    let budget = budget.resolve();
    let mut includes: Vec<(String, String)> = module
        .include_pins
        .iter()
        .map(|pin| (pin.resolved.clone(), pin.sha256.clone()))
        .collect();
    includes.sort();
    includes.dedup();
    let env_values: Vec<(String, Option<String>)> = referenced_env_names(module)
        .into_iter()
        .map(|name| {
            let value = crate::eval::canonical_env_name(&name);
            (name, env.get(&value).cloned())
        })
        .collect();
    let object = serde_json::json!({
        "active.cache": CACHE_VERSION,
        // The evaluator's own version, so an upgrade never serves a
        // result an older binary computed under different semantics.
        "evaluator": env!("CARGO_PKG_VERSION"),
        "source": module.source_sha256,
        "includes": includes,
        "mode": match mode { Mode::Config => "config", Mode::Data => "data" },
        "profile": profile,
        "budget": {
            "nodes": budget.max_nodes,
            "depth": budget.max_depth,
            "array_len": budget.max_array_len,
            "keys": budget.max_keys,
            "wall_ms": budget.max_wall_ms,
            "value_size": budget.max_value_size,
            "total_size": budget.max_total_value_size,
        },
        "env": env_values,
        "variant": variant,
        // L-P05: a pure evaluation refuses impure documents, so it must
        // never read (or overwrite) the entry a default evaluation
        // filled for the same inputs.
        "pure": pure,
    });
    let canonical = crate::envelope::canonical_json(&object);
    Some(
        blake3::keyed_hash(key, canonical.as_bytes())
            .to_hex()
            .to_string(),
    )
}

/// A domain-separated keyed MAC, `mac:<hex>`. Plans and receipts use
/// this so a self-applied file cannot mint its own approval.
pub fn keyed_mac(key: &[u8; KEY_BYTES], domain: &str, bytes: &[u8]) -> String {
    let mut preimage = Vec::with_capacity(domain.len() + 1 + bytes.len());
    preimage.extend_from_slice(domain.as_bytes());
    preimage.push(b'\n');
    preimage.extend_from_slice(bytes);
    format!("mac:{}", blake3::keyed_hash(key, &preimage).to_hex())
}

/// The per-user key file: `$ACTIVE_CACHE_KEY_FILE` when set, else the
/// platform config directory (`%APPDATA%\active` on Windows,
/// `$XDG_CONFIG_HOME/active` or `~/.config/active` on Unix). It lives
/// outside the cache directory so a directory listing cannot reproduce
/// a key.
pub fn default_key_path() -> Option<PathBuf> {
    default_key_path_from(|name| std::env::var_os(name))
}

fn default_key_path_from(lookup: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(path) = lookup(KEY_ENV).filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path));
    }
    #[cfg(windows)]
    {
        let base = lookup("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                lookup("USERPROFILE")
                    .filter(|value| !value.is_empty())
                    .map(|value| PathBuf::from(value).join("AppData").join("Roaming"))
            })?;
        Some(base.join("active").join("cache-key"))
    }
    #[cfg(not(windows))]
    {
        let base = lookup("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                lookup("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|value| PathBuf::from(value).join(".config"))
            })
            .or_else(|| {
                lookup("USERPROFILE")
                    .filter(|value| !value.is_empty())
                    .map(|value| PathBuf::from(value).join(".config"))
            })?;
        Some(base.join("active").join("cache-key"))
    }
}

/// A conservative pre-check before an entry is built: every rendered
/// part plus the serialized diagnostics, export diagnostics, errors, and
/// receipt must fit the cap. The writer re-checks the serialized size as
/// the backstop.
pub fn entry_fits(json: &str, yaml: Option<&str>, toml: Option<&str>, extra_bytes: u64) -> bool {
    fits_with_cap(json, yaml, toml, extra_bytes, MAX_ENTRY_BYTES)
}

fn fits_with_cap(
    json: &str,
    yaml: Option<&str>,
    toml: Option<&str>,
    extra_bytes: u64,
    cap: u64,
) -> bool {
    let mut total = json.len() as u64;
    total = total.saturating_add(yaml.map_or(0, |text| text.len() as u64));
    total = total.saturating_add(toml.map_or(0, |text| text.len() as u64));
    total.saturating_add(extra_bytes) <= cap
}

/// Refuse a cache root that a second user could modify on Unix: a
/// symlink, a non-directory, or a group/other-writable directory. A
/// missing root is fine (the first write creates it 0700). Windows ACLs
/// are not checked.
fn check_root_dir(root: &Path, owner: Option<u32>) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        match std::fs::symlink_metadata(root) {
            Ok(metadata) => {
                if !metadata.file_type().is_dir() {
                    return Err(format!(
                        "cache root {} is not a plain directory",
                        root.display()
                    ));
                }
                if metadata.permissions().mode() & 0o022 != 0 {
                    return Err(format!(
                        "cache root {} is writable by group or others; restrict it to its owner",
                        root.display()
                    ));
                }
                if owner.is_some_and(|owner| metadata.uid() != owner) {
                    return Err(format!(
                        "cache root {} is not owned by the key file's user",
                        root.display()
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot inspect cache root {}: {e}", root.display())),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (root, owner);
    }
    Ok(())
}

/// Read the per-user key, creating it (32 random bytes, hex, owner-only
/// on Unix) when absent. A malformed file is an error: silently
/// replacing it would make every existing entry unreachable without
/// saying why.
pub fn load_or_create_key(path: &Path) -> Result<[u8; KEY_BYTES], String> {
    match read_key(path) {
        Ok(key) => Ok(key),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => create_key(path),
        Err(e) => Err(format!("cannot read cache key {}: {e}", path.display())),
    }
}

fn read_key(path: &Path) -> std::io::Result<[u8; KEY_BYTES]> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "cache key is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "cache key is readable by other users; restrict it to 0600",
            ));
        }
    }
    // The key is 64 hex characters; anything larger is a planted or
    // corrupt file, not a key worth reading into memory.
    let limit = (KEY_BYTES * 2 + 64) as u64;
    if metadata.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cache key is larger than {limit} bytes"),
        ));
    }
    let file = std::fs::File::open(path)?;
    // Re-check the OPENED handle: a swapped symlink or a file owned by
    // another user must not become the MAC key.
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "cache key is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if opened.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "cache key is readable by other users; restrict it to 0600",
            ));
        }
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cache key is larger than {limit} bytes"),
        ));
    }
    parse_key(&bytes, path).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn parse_key(bytes: &[u8], path: &Path) -> Result<[u8; KEY_BYTES], String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| format!("cache key {} is not UTF-8", path.display()))?
        .trim();
    if text.len() != KEY_BYTES * 2 {
        return Err(format!(
            "cache key {} must be {} hex characters",
            path.display(),
            KEY_BYTES * 2
        ));
    }
    let mut key = [0u8; KEY_BYTES];
    let bytes = text.as_bytes();
    for index in 0..KEY_BYTES {
        let high = hex_digit(bytes[index * 2], path)?;
        let low = hex_digit(bytes[index * 2 + 1], path)?;
        key[index] = (high << 4) | low;
    }
    Ok(key)
}

fn hex_digit(byte: u8, path: &Path) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!(
            "cache key {} contains a non-hex character",
            path.display()
        )),
    }
}

fn create_key(path: &Path) -> Result<[u8; KEY_BYTES], String> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            // Atomic mode: the created directory is private without a
            // follow-up chmod that a symlink swap could redirect.
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(parent) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(format!(
                        "cannot create cache key directory {}: {e}",
                        parent.display()
                    ))
                }
            }
        }
    }
    let mut key = [0u8; KEY_BYTES];
    getrandom::getrandom(&mut key).map_err(|e| format!("cannot generate a cache key: {e}"))?;
    let text: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(text.as_bytes())
                .map_err(|e| format!("cannot write cache key {}: {e}", path.display()))?;
            Ok(key)
        }
        // Another process won the race; use its key so both share a
        // cache identity.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            read_key(path).map_err(|e| format!("cannot read cache key {}: {e}", path.display()))
        }
        Err(e) => Err(format!("cannot create cache key {}: {e}", path.display())),
    }
}

/// The owner of the key file, so the cache can require the same owner.
/// `None` on non-Unix platforms (ACLs are not checked).
pub fn file_owner(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|metadata| metadata.uid())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; KEY_BYTES] = [7u8; KEY_BYTES];

    fn key(
        module: &Module,
        profile: Option<&str>,
        budget: &Budget,
        mode: Mode,
        variant: &str,
    ) -> String {
        eval_key(
            module,
            profile,
            budget,
            mode,
            variant,
            &HashMap::new(),
            &KEY,
            false,
        )
        .expect("source-backed module")
    }

    #[test]
    fn keys_cover_source_includes_profile_budget_mode_variant_and_mac_key() {
        let module = crate::parser::parse("port = 8080\n", "doc").expect("parse");
        let budget = Budget::default();
        let base = key(&module, None, &budget, Mode::Config, "");
        assert_eq!(base.len(), 64);
        assert_eq!(base, key(&module, None, &budget, Mode::Config, ""));
        assert_ne!(base, key(&module, Some("prod"), &budget, Mode::Config, ""));
        assert_ne!(base, key(&module, None, &budget, Mode::Data, ""));
        assert_ne!(base, key(&module, None, &budget, Mode::Config, "cone"));
        let mut other = budget;
        other.max_nodes += 1;
        assert_ne!(base, key(&module, None, &other, Mode::Config, ""));
        let module2 = crate::parser::parse("port = 8081\n", "doc").expect("parse");
        assert_ne!(base, key(&module2, None, &budget, Mode::Config, ""));
        let other_key = eval_key(
            &module,
            None,
            &budget,
            Mode::Config,
            "",
            &HashMap::new(),
            &[9u8; KEY_BYTES],
            false,
        )
        .expect("key");
        assert_ne!(base, other_key, "the MAC key is part of the identity");
    }

    #[test]
    fn a_module_without_source_bytes_has_no_cache_key() {
        let module = Module::new("synthetic");
        assert!(eval_key(
            &module,
            None,
            &Budget::default(),
            Mode::Config,
            "",
            &HashMap::new(),
            &KEY,
            false,
        )
        .is_none());
    }

    #[test]
    fn env_values_are_part_of_the_key_for_reads_and_secret_locators() {
        let module = crate::parser::parse(
            "host = env.ACTIVE_CACHE_HOST\nkey = secret.env(\"ACTIVE_CACHE_SECRET\")\n",
            "doc",
        )
        .expect("parse");
        assert_eq!(
            referenced_env_names(&module)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![
                "ACTIVE_CACHE_HOST".to_string(),
                "ACTIVE_CACHE_SECRET".to_string()
            ]
        );
        let budget = Budget::default();
        let empty = key(&module, None, &budget, Mode::Config, "");
        let mut env = HashMap::new();
        env.insert("ACTIVE_CACHE_HOST".to_string(), "a".to_string());
        let a = eval_key(&module, None, &budget, Mode::Config, "", &env, &KEY, false).expect("key");
        env.insert("ACTIVE_CACHE_HOST".to_string(), "b".to_string());
        let b = eval_key(&module, None, &budget, Mode::Config, "", &env, &KEY, false).expect("key");
        assert_ne!(empty, a);
        assert_ne!(a, b);
        // A secret locator's value seeds the redactor, so it changes the
        // redacted output and belongs in the key.
        env.insert("ACTIVE_CACHE_SECRET".to_string(), "x".to_string());
        assert_ne!(
            b,
            eval_key(&module, None, &budget, Mode::Config, "", &env, &KEY, false).expect("key")
        );
    }

    #[test]
    fn entries_round_trip_and_reject_stale_versions() {
        let root = std::env::temp_dir().join(format!("active-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cache = EvalCache::new(&root);
        let key = "a".repeat(64);
        let entry = CacheEntry {
            version: CACHE_VERSION,
            key: key.clone(),
            json: "{\"port\":8080}".into(),
            yaml: Some("port: 8080\n".into()),
            yaml_error: None,
            toml: Some("port = 8080\n".into()),
            toml_error: None,
            export_diagnostics: Vec::new(),
            diagnostics: Vec::new(),
            receipt: serde_json::json!({"outcome":"clean"}),
        };
        cache.put(&entry).expect("put");
        let got = cache.get(&key).expect("get").expect("entry");
        assert_eq!(got.json, entry.json);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join(format!("{key}.json")))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "entry mode: {mode:o}");
            let root_mode = std::fs::metadata(&root)
                .expect("root metadata")
                .permissions()
                .mode();
            assert_eq!(root_mode & 0o777, 0o700, "root mode: {root_mode:o}");
        }
        let mut stale = entry.clone();
        stale.version = CACHE_VERSION + 1;
        // `put` refuses a mismatched version, so write the stale file
        // directly: the reader must still treat it as a miss.
        std::fs::write(
            root.join(format!("{key}.json")),
            serde_json::to_string(&stale).expect("json"),
        )
        .expect("write stale");
        assert!(
            cache.get(&key).expect("get").is_none(),
            "stale version misses"
        );
        // A hand-edited file that still parses but names another key is a
        // miss, never a result under the wrong identity.
        let path = root.join(format!("{key}.json"));
        let mut text = std::fs::read_to_string(&path).expect("read");
        text = text.replace(
            &format!("\"key\":\"{key}\""),
            &format!("\"key\":\"{}\"", "b".repeat(64)),
        );
        std::fs::write(&path, text).expect("write");
        assert!(
            cache.get(&key).expect("get").is_none(),
            "key mismatch misses"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_non_regular_entry_is_a_miss_not_a_read() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("active-cache-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let outside = root.join("outside.txt");
        std::fs::write(&outside, "{}").expect("outside");
        let key = "c".repeat(64);
        symlink(&outside, root.join(format!("{key}.json"))).expect("symlink");
        let cache = EvalCache::new(&root);
        assert!(cache.get(&key).is_err(), "a symlink is refused");
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_shared_cache_root_and_a_shared_entry_are_refused() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let root = std::env::temp_dir().join(format!("active-cache-shared-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).expect("mode");
        let cache = EvalCache::new(&root);
        let key = "d".repeat(64);
        assert!(
            cache.get(&key).is_err(),
            "a group/other-writable root is refused"
        );
        let entry = CacheEntry {
            version: CACHE_VERSION,
            key: key.clone(),
            json: "{}".into(),
            yaml: None,
            yaml_error: None,
            toml: None,
            toml_error: None,
            export_diagnostics: Vec::new(),
            diagnostics: Vec::new(),
            receipt: serde_json::json!({"outcome":"clean"}),
        };
        assert!(cache.put(&entry).is_err(), "no write into a shared root");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).expect("mode");
        // The owner must match the key file's user.
        let owner = std::fs::metadata(&root).expect("root").uid();
        let other = EvalCache::new(&root).with_owner(Some(owner.wrapping_add(1)));
        assert!(
            other.get(&key).is_err(),
            "a root owned by another user is refused"
        );
        cache.put(&entry).expect("private root accepts");
        std::fs::set_permissions(
            root.join(format!("{key}.json")),
            std::fs::Permissions::from_mode(0o644),
        )
        .expect("mode");
        assert!(
            cache.get(&key).is_err(),
            "a world-readable entry is not served"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_entry_pre_check_counts_every_part() {
        assert!(fits_with_cap("aaaa", Some("bb"), None, 0, 100));
        assert!(!fits_with_cap("aaaa", Some("bb"), None, 0, 5));
        assert!(
            !fits_with_cap("a", None, None, 200, 100),
            "diagnostics and metadata count against the cap"
        );
        assert!(fits_with_cap("", None, None, 0, 0));
    }

    #[cfg(unix)]
    #[test]
    fn the_default_key_path_sits_under_the_config_directory() {
        let path = default_key_path_from(|name| match name {
            "HOME" => Some("/home/u".into()),
            _ => None,
        })
        .expect("path");
        assert_eq!(path, PathBuf::from("/home/u/.config/active/cache-key"));
        let path = default_key_path_from(|name| match name {
            "XDG_CONFIG_HOME" => Some("/cfg".into()),
            "HOME" => Some("/home/u".into()),
            _ => None,
        })
        .expect("path");
        assert_eq!(path, PathBuf::from("/cfg/active/cache-key"));
    }

    #[cfg(windows)]
    #[test]
    fn the_default_key_path_sits_under_appdata() {
        let path = default_key_path_from(|name| match name {
            "APPDATA" => Some("C:\\Users\\u\\AppData\\Roaming".into()),
            _ => None,
        })
        .expect("path");
        assert_eq!(
            path,
            PathBuf::from("C:\\Users\\u\\AppData\\Roaming")
                .join("active")
                .join("cache-key")
        );
    }

    #[test]
    fn an_oversized_key_file_is_refused_without_reading_it_all() {
        let root = std::env::temp_dir().join(format!("active-cache-bigkey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let path = root.join("cache-key");
        std::fs::write(&path, "a".repeat(KEY_BYTES * 2 + 65)).expect("write");
        assert!(load_or_create_key(&path).is_err(), "oversized key errors");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_key_file_round_trips_and_rejects_malformed_content() {
        let root = std::env::temp_dir().join(format!("active-cache-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let path = root.join("cache-key");
        let created = load_or_create_key(&path).expect("create");
        assert_eq!(load_or_create_key(&path).expect("reload"), created);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "key mode: {mode:o}");
            // A key another user can read defeats the MAC; refuse it.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("mode");
            assert!(
                load_or_create_key(&path).is_err(),
                "a world-readable key is refused"
            );
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("mode");
        }
        std::fs::write(&path, "not a key").expect("write");
        assert!(load_or_create_key(&path).is_err(), "malformed key errors");
        std::fs::remove_dir_all(&root).ok();
    }
}
