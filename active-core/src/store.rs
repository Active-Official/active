//! Content-addressed include store (L-P04).
//!
//! The store is a cache, never an input to evaluation on its own: an
//! include is still read from its declared path and verified against its
//! pin. Storing the parsed bytes deduplicates repeated includes of the
//! same content (LANG-C-002 fan-out) at the storage layer, and gives a
//! lockfile's content hashes a concrete local address.
//!
//! Layout: `<root>/<sha256-hex>` holds one immutable file with those
//! bytes. Entries are written through a private temp file (0600 on Unix),
//! fsynced, made read-only (0400 on Unix), then renamed into place. The
//! root is created 0700 on Unix. Garbage collection is explicit and
//! rooted in lockfiles: every `sha256` the canonical lock parser accepts
//! keeps its entry.
//!
//! Opt-in side effect: `ParseOptions::include_store` makes the parser
//! publish bytes it has already read. It never changes the evaluated
//! tree, and a cache failure is informational (W018).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Environment override for the default store root. Only the `active
/// store` subcommands use it; `eval`/`lock` require an explicit
/// `--store` directory.
pub const ENV_STORE_DIR: &str = "ACTIVE_STORE_DIR";

/// Largest entry the store will accept. Includes are already bounded by
/// the parser's 16 MiB include budget.
pub const MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;

/// Largest lockfile accepted as a GC root source.
pub const MAX_ROOT_BYTES: u64 = 1 << 20;

/// A content-addressed include store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// A store rooted at `root` (created lazily on the first write).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$ACTIVE_STORE_DIR`, else `~/.cache/active/store`. Empty variables
    /// are ignored: an empty path would silently target the cwd.
    pub fn default_root() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os(ENV_STORE_DIR).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .filter(|v| !v.is_empty())
            .map(|home| {
                PathBuf::from(home)
                    .join(".cache")
                    .join("active")
                    .join("store")
            })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn check_root(&self) -> Result<(), String> {
        if self.root.as_os_str().is_empty() {
            return Err("store directory is empty".to_string());
        }
        Ok(())
    }

    /// The immutable entry path for a 64-hex digest. Entries are always
    /// lowercase: an upper/mixed-case name is not one of ours.
    pub fn path_for(&self, hash: &str) -> Result<PathBuf, String> {
        self.check_root()?;
        if !valid_hash(hash) {
            return Err(format!("not a SHA-256 hex digest: {hash:?}"));
        }
        Ok(self.root.join(hash.to_ascii_lowercase()))
    }

    /// Store `bytes` and return their digest.
    pub fn put(&self, bytes: &[u8]) -> Result<String, String> {
        let hash = crate::provenance::sha256_hex(bytes);
        self.put_hashed(&hash, bytes)?;
        Ok(hash)
    }

    /// Store `bytes` under a digest the caller already computed (the
    /// parser hashes every include for its pin). Existing entries are
    /// never rewritten unless their content no longer matches the name,
    /// in which case the damaged entry is repaired.
    pub(crate) fn put_hashed(&self, hash: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.path_for(hash)?;
        if bytes.len() as u64 > MAX_ENTRY_BYTES {
            return Err(format!(
                "entry is {} bytes; the store limit is {MAX_ENTRY_BYTES}",
                bytes.len()
            ));
        }
        ensure_root(&self.root)?;
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            // A hard link shares attributes with a file outside the
            // store; refuse to touch it.
            if is_hard_link(&metadata) {
                return Err(format!(
                    "store entry {} is a hard link; refusing to use it",
                    path.display()
                ));
            }
            if metadata.file_type().is_file() {
                if let Ok(Some(existing)) = read_regular_capped(&path) {
                    if existing == bytes {
                        return Ok(());
                    }
                }
            }
            // Corrupt, oversized, partial, or a symlink/special file:
            // unlink the name without following it, then republish.
            remove_entry(&path)?;
        }
        publish(&self.root, &path, bytes)
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.path_for(hash)
            .ok()
            .and_then(|p| std::fs::symlink_metadata(p).ok())
            .map(|m| m.file_type().is_file() && !is_hard_link(&m))
            .unwrap_or(false)
    }

    /// Read a stored entry, verifying it still hashes to its name.
    pub fn get(&self, hash: &str) -> Result<Option<Vec<u8>>, String> {
        let path = self.path_for(hash)?;
        let Some(bytes) = read_regular_capped(&path)? else {
            return Ok(None);
        };
        let got = crate::provenance::sha256_hex(&bytes);
        if !got.eq_ignore_ascii_case(hash) {
            return Err(format!(
                "store entry {} is corrupt (hashes to {got})",
                path.display()
            ));
        }
        Ok(Some(bytes))
    }

    /// Every complete (lowercase, regular, non-symlink) entry name,
    /// sorted. Contents are not re-hashed; `get` verifies on read.
    pub fn hashes(&self) -> Result<Vec<String>, String> {
        self.check_root()?;
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("cannot read store {}: {e}", self.root.display())),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("cannot read store entry: {e}"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            // A symlink or a hard link is not one of our immutable
            // entries: never list it (and never chmod a target).
            if !valid_hash(name) || name != name.to_ascii_lowercase() {
                continue;
            }
            let metadata = match std::fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.file_type().is_file() && !is_hard_link(&metadata) {
                out.push(name.to_string());
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Delete entries not in `roots`; returns the removed (or, with
    /// `dry_run`, removable) digests. `gc` is not safe concurrently with
    /// `put`: run it between evaluations.
    #[allow(clippy::permissions_set_readonly_false)]
    pub fn gc(&self, roots: &HashSet<String>, dry_run: bool) -> Result<Vec<String>, String> {
        self.check_root()?;
        let roots: HashSet<String> = roots.iter().map(|h| h.to_ascii_lowercase()).collect();
        let mut removed = Vec::new();
        for hash in self.hashes()? {
            if roots.contains(&hash) {
                continue;
            }
            if dry_run {
                removed.push(hash);
                continue;
            }
            let path = self.root.join(&hash);
            // On Unix removal only needs directory write permission, and
            // chmod could follow an attacker's link; never touch modes.
            // Windows needs the read-only attribute cleared on the entry
            // itself (symlink_metadata, so a link's target is untouched).
            #[cfg(windows)]
            if let Ok(metadata) = std::fs::symlink_metadata(&path) {
                let mut permissions = metadata.permissions();
                if permissions.readonly() {
                    permissions.set_readonly(false);
                    let _ = std::fs::set_permissions(&path, permissions);
                }
            }
            std::fs::remove_file(&path)
                .map_err(|e| format!("cannot remove store entry {}: {e}", path.display()))?;
            removed.push(hash);
        }
        Ok(removed)
    }
}

/// Collect every 64-hex `sha256` in a lockfile as a GC root, through the
/// canonical lock parser (version, pins, and shapes all validated). An
/// empty root set is refused: `gc` must never interpret a truncated or
/// empty file as "delete everything".
pub fn roots_from_lockfile(lock_text: &str) -> Result<HashSet<String>, String> {
    if lock_text.len() as u64 > MAX_ROOT_BYTES {
        return Err(format!(
            "lockfile exceeds the {MAX_ROOT_BYTES} byte root limit"
        ));
    }
    crate::provenance::lock_roots(lock_text)
}

/// Create or validate the store root. A root that is a symlink or a
/// non-directory is refused, and an existing root is re-restricted to
/// `0700` on Unix: a pre-existing shared root must not expose entries.
fn ensure_root(root: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "store root {} is not a directory (symlinks are refused)",
                    root.display()
                ));
            }
            restrict_root(root)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(root)
                .map_err(|e| format!("cannot create store {}: {e}", root.display()))?;
            restrict_root(root)
        }
        Err(e) => Err(format!("cannot inspect store root {}: {e}", root.display())),
    }
}

/// Read an entry through the size cap, refusing links and special files.
/// `Ok(None)` means the entry does not exist.
fn read_regular_capped(path: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "cannot inspect store entry {}: {e}",
                path.display()
            ))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!(
            "store entry {} is not a regular file",
            path.display()
        ));
    }
    if metadata.len() > MAX_ENTRY_BYTES {
        return Err(format!(
            "store entry {} exceeds the {MAX_ENTRY_BYTES} byte limit",
            path.display()
        ));
    }
    let file = std::fs::File::open(path)
        .map_err(|e| format!("cannot read store entry {}: {e}", path.display()))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read store entry {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err(format!(
            "store entry {} exceeds the {MAX_ENTRY_BYTES} byte limit",
            path.display()
        ));
    }
    Ok(Some(bytes))
}

/// Unlink an entry without following a symlink or clearing modes on Unix.
#[allow(clippy::permissions_set_readonly_false)]
fn remove_entry(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_file() {
            let mut permissions = metadata.permissions();
            if permissions.readonly() {
                permissions.set_readonly(false);
                let _ = std::fs::set_permissions(path, permissions);
            }
        }
    }
    std::fs::remove_file(path)
        .map_err(|e| format!("cannot remove store entry {}: {e}", path.display()))
}

/// A hard link shares attributes with a file outside the store. Unix
/// exposes the link count; Windows has no stable equivalent here, so its
/// entries rely on the root ACL plus the read-only attribute.
fn is_hard_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.nlink() > 1
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn publish(root: &Path, final_path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut last_error = None;
    for _ in 0..8 {
        let tmp = root.join(format!(
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
        let mut file = match options.open(&tmp) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = Some(e);
                continue;
            }
            Err(e) => {
                return Err(format!(
                    "cannot create store temp file {}: {e}",
                    tmp.display()
                ))
            }
        };
        let write = (|| -> std::io::Result<()> {
            file.write_all(bytes)?;
            file.sync_all()
        })();
        if let Err(e) = write {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("cannot write store entry {}: {e}", tmp.display()));
        }
        drop(file);
        if let Err(e) = make_read_only(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        match std::fs::rename(&tmp, final_path) {
            Ok(()) => return Ok(()),
            Err(_) if final_path.is_file() => {
                let _ = std::fs::remove_file(&tmp);
                // A racing writer published the same digest; accept its
                // bytes only if they are the ones we were asked to store.
                if read_regular_capped(final_path)
                    .ok()
                    .flatten()
                    .map(|b| b == bytes)
                    .unwrap_or(false)
                {
                    return Ok(());
                }
                return Err(format!(
                    "store entry {} already exists with different bytes",
                    final_path.display()
                ));
            }
            Err(e) => {
                last_error = Some(e);
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
    Err(format!(
        "cannot publish store entry {}: {}",
        final_path.display(),
        last_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "retry budget exhausted".to_string())
    ))
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Entries are 0400 on Unix; Windows uses the read-only attribute (ACLs
/// are inherited from the directory).
fn make_read_only(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400))
            .map_err(|e| format!("cannot make store entry read-only {}: {e}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(path)
            .map_err(|e| format!("cannot inspect store entry {}: {e}", path.display()))?
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions)
            .map_err(|e| format!("cannot make store entry read-only {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Clear a read-only bit before test corruption/cleanup. Production
/// repair never chmods (a link's target must stay untouched).
#[cfg(test)]
#[allow(clippy::permissions_set_readonly_false)]
fn make_writable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(path)
            .map_err(|e| format!("cannot inspect store entry {}: {e}", path.display()))?
            .permissions();
        if permissions.readonly() {
            permissions.set_readonly(false);
            std::fs::set_permissions(path, permissions)
                .map_err(|e| format!("cannot repair store entry {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// The root holds raw include bytes; keep it private to the owner.
fn restrict_root(root: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            format!(
                "cannot restrict store directory {} to its owner: {e}",
                root.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = root;
    }
    Ok(())
}

/// Distinguish temp names within one process without extra state.
fn monotonic_suffix() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    seq.wrapping_add(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> Store {
        let root = std::env::temp_dir().join(format!(
            "active-store-{name}-{}-{}",
            std::process::id(),
            monotonic_suffix()
        ));
        let _ = std::fs::remove_dir_all(&root);
        Store::new(root)
    }

    fn cleanup(store: &Store) {
        // Windows read-only entries make remove_dir_all fail; clear them.
        if let Ok(hashes) = store.hashes() {
            for hash in hashes {
                let path = store.root().join(hash);
                let _ = make_writable(&path);
            }
        }
        let _ = std::fs::remove_dir_all(store.root());
    }

    #[test]
    fn put_is_idempotent_and_content_addressed() {
        let store = temp_store("put");
        let first = store.put(b"v = 1\n").expect("put");
        let again = store.put(b"v = 1\n").expect("put again");
        assert_eq!(first, again);
        assert_eq!(store.hashes().expect("hashes"), vec![first.clone()]);
        assert_eq!(
            store.get(&first).expect("get").as_deref(),
            Some(b"v = 1\n".as_slice())
        );
        let path = store.path_for(&first).expect("path");
        assert!(
            std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .readonly(),
            "store entries are read-only"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o400, "entry mode: {mode:o}");
            let root_mode = std::fs::metadata(store.root())
                .expect("root")
                .permissions()
                .mode();
            assert_eq!(root_mode & 0o777, 0o700, "root mode: {root_mode:o}");
        }
        cleanup(&store);
    }

    #[test]
    fn a_corrupt_entry_is_repaired_by_put_and_refused_by_get() {
        let store = temp_store("repair");
        let hash = store.put(b"v = 1\n").expect("put");
        let path = store.path_for(&hash).expect("path");
        make_writable(&path).expect("writable");
        std::fs::write(&path, b"CORRUPTED").expect("corrupt");
        assert!(store.get(&hash).is_err(), "get must refuse corruption");
        store.put_hashed(&hash, b"v = 1\n").expect("repair");
        assert_eq!(
            store.get(&hash).expect("get").as_deref(),
            Some(b"v = 1\n".as_slice())
        );
        cleanup(&store);
    }

    #[test]
    fn gc_keeps_rooted_entries_and_removes_the_rest() {
        let store = temp_store("gc");
        let keep = store.put(b"v = 1\n").expect("keep");
        let drop = store.put(b"v = 2\n").expect("drop");
        let roots = HashSet::from([keep.clone()]);
        let dry = store.gc(&roots, true).expect("dry run");
        assert_eq!(dry, vec![drop.clone()]);
        assert!(store.contains(&drop), "dry run removes nothing");
        let removed = store.gc(&roots, false).expect("gc");
        assert_eq!(removed, vec![drop.clone()]);
        assert!(store.contains(&keep));
        assert!(!store.contains(&drop));
        cleanup(&store);
    }

    #[test]
    fn non_entries_are_never_listed_or_deleted() {
        let store = temp_store("nonentries");
        store.put(b"v = 1\n").expect("put");
        std::fs::create_dir_all(store.root()).expect("root");
        std::fs::write(store.root().join(".tmp-leftover"), b"partial").expect("temp");
        std::fs::write(store.root().join("A".repeat(64)), b"upper").expect("upper");
        let hashes = store.hashes().expect("hashes");
        assert_eq!(hashes.len(), 1);
        let removed = store.gc(&HashSet::new(), false).expect("gc");
        assert_eq!(removed.len(), 1);
        assert!(
            store.root().join(".tmp-leftover").exists(),
            "temp files are not ours to delete"
        );
        assert!(
            store.root().join("A".repeat(64)).exists(),
            "non-lowercase names are not entries"
        );
        cleanup(&store);
    }

    #[test]
    fn an_empty_root_is_refused() {
        let store = Store::new("");
        assert!(store.put(b"v = 1\n").is_err());
        assert!(store.hashes().is_err());
        assert!(store.gc(&HashSet::new(), true).is_err());
    }

    #[test]
    fn lockfile_roots_cover_every_sha256_section() {
        let text = "\
version = 2
[main]
path = \"/x/main.active\"
sha256 = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"
[[provenance.includes]]
path = \"/x/lib.active\"
sha256 = \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"
[[provenance.include_pins]]
as = \"lib.active\"
resolved = \"lib.active\"
sha256 = \"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\"
";
        let roots = roots_from_lockfile(text).expect("roots");
        assert_eq!(roots.len(), 3);
        assert!(roots.contains(&"a".repeat(64)));
        assert!(roots.contains(&"b".repeat(64)));
        assert!(roots.contains(&"c".repeat(64)));
        assert!(roots_from_lockfile("main = { sha256 = 5 }\n").is_err());
        assert!(roots_from_lockfile("main = { sha256 = \"abc\" }\n").is_err());
        // Truncated/empty locks name no roots and must not authorize a
        // wipe.
        assert!(roots_from_lockfile("").is_err());
        assert!(roots_from_lockfile("version = 2\n[main]\npath = \"/x\"\n").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_never_followed_or_chmodded() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let store = temp_store("links");
        let outside = std::env::temp_dir().join(format!(
            "active-store-outside-{}-{}",
            std::process::id(),
            monotonic_suffix()
        ));
        std::fs::write(&outside, b"external").expect("outside");
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).expect("mode");
        // A symlink named like an entry is not an entry: it is not listed,
        // and gc never touches it.
        std::fs::create_dir_all(store.root()).expect("root");
        let link_name = "a".repeat(64);
        symlink(&outside, store.root().join(&link_name)).expect("symlink");
        assert!(store.hashes().expect("hashes").is_empty());
        assert!(store.gc(&HashSet::new(), false).expect("gc").is_empty());
        assert!(
            store.root().join(&link_name).exists(),
            "gc leaves links alone"
        );
        // Publishing under that digest unlinks the link and writes a fresh
        // entry; the target is untouched (no chmod, no write).
        let bytes = b"v = 1\n";
        let hash = crate::provenance::sha256_hex(bytes);
        symlink(&outside, store.root().join(&hash)).expect("symlink entry");
        store.put_hashed(&hash, bytes).expect("repair link");
        assert_eq!(std::fs::read(&outside).expect("outside"), b"external");
        let mode = std::fs::metadata(&outside)
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644, "target mode untouched: {mode:o}");
        assert_eq!(
            store.get(&hash).expect("get").as_deref(),
            Some(bytes.as_slice())
        );
        // A hard link shares its inode with an outside file and is refused.
        let hard = "b".repeat(64);
        std::fs::hard_link(&outside, store.root().join(&hard)).expect("hard link");
        assert!(store.hashes().expect("hashes").iter().all(|h| h != &hard));
        assert!(store.put_hashed(&hard, b"other").is_err());
        std::fs::remove_file(&outside).ok();
        cleanup(&store);
    }
}
