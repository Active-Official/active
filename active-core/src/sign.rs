//! Signed overlays (C-052, spec/threat-model.md §4).
//!
//! Detached Ed25519 signatures over `sha256(identity || 0x00 || bytes)`,
//! where the identity binds the sidecar's `path` plus optional `role` and
//! `version` (LANG-I-002): a signature verifies only for the identity it
//! was made for, so copying a signed overlay to another path fails.
//! Sidecars may carry `expires = "YYYY-MM-DD"`; the trusted-keys file may
//! carry `revoked = ["kid", ...]`. Both fail closed.
//!
//! Verification resolves `kid` against a trusted-keys set; enforcement
//! hooks into include loading via `ParseOptions` (E032/E037/E038/E039).
//! Key files are bearer credentials (0600, never committed, never logged).

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// A trusted key set: key id → public key, plus revoked ids. Cloned into
/// the parser when overlay enforcement is on (small maps; no lifetimes).
#[derive(Debug, Clone, Default)]
pub struct TrustedKeys {
    pub keys: HashMap<String, VerifyingKey>,
    /// Key ids this trust file has revoked: a sidecar naming one fails
    /// closed (E038), even if its public half is still listed.
    pub revoked: HashSet<String>,
}

impl TrustedKeys {
    pub fn key(&self, kid: &str) -> Option<&VerifyingKey> {
        self.keys.get(kid)
    }
}

/// A verification failure: a stable diagnostic code plus a message that
/// names the file and reason, never key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyFailure {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for VerifyFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Key-id charset: printable, no TOML metacharacters, no controls,
/// non-empty. Enforced when writing sidecars, when parsing them,
/// and when loading trusted sets — an id that cannot round-trip
/// byte-identical through TOML is rejected everywhere (injection
/// and log-forging resistance).
pub fn valid_kid(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| !c.is_control() && c != '"' && c != '\\' && c != '\'' && c != '#')
}

/// Parse a trusted-keys TOML document (`[[keys]] id/public`, optional
/// `revoked = [...]`) into the trust set. Malformed entries are hard
/// errors naming the entry, never key material.
pub fn load_trusted_keys(text: &str) -> Result<TrustedKeys, String> {
    let doc: toml::Value =
        toml::from_str(text).map_err(|e| format!("invalid trusted-keys file: {e}"))?;
    let keys = doc
        .get("keys")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "trusted-keys file needs a [[keys]] array".to_string())?;
    let mut out = TrustedKeys::default();
    for (i, entry) in keys.iter().enumerate() {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("trusted-keys entry {i} needs a string `id`"))?;
        if !valid_kid(id) {
            return Err(format!("trusted-keys entry {i} has an invalid id"));
        }
        let public = entry
            .get("public")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("trusted-keys entry '{id}' needs a hex `public` key"))?;
        let bytes = hex_bytes(public).ok_or_else(|| {
            format!("trusted-keys entry '{id}' has a malformed public key (want 64 hex chars)")
        })?;
        let key = VerifyingKey::from_bytes(&bytes)
            .map_err(|_| format!("trusted-keys entry '{id}' is not a valid Ed25519 public key"))?;
        if out.keys.insert(id.to_string(), key).is_some() {
            return Err(format!("duplicate trusted-keys id '{id}'"));
        }
    }
    if out.keys.is_empty() {
        return Err("trusted-keys file contains no [[keys]] entries".to_string());
    }
    if let Some(revoked) = doc.get("revoked") {
        let list = revoked
            .as_array()
            .ok_or_else(|| "`revoked` must be an array of key ids".to_string())?;
        for entry in list {
            let id = entry
                .as_str()
                .ok_or_else(|| "`revoked` entries must be strings".to_string())?;
            if !valid_kid(id) {
                return Err("`revoked` holds an invalid key id".to_string());
            }
            out.revoked.insert(id.to_string());
        }
    }
    Ok(out)
}

/// Parse a 64-hex-char Ed25519 secret seed (a `<name>.secret` file).
pub fn load_secret_key(text: &str) -> Result<SigningKey, String> {
    let bytes = hex_bytes(text.trim())
        .ok_or_else(|| "secret key file must hold 64 hex chars (32-byte seed)".to_string())?;
    Ok(SigningKey::from_bytes(&bytes))
}

/// Generate a fresh secret seed from OS randomness.
pub fn generate_secret_key() -> Result<SigningKey, String> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| format!("OS randomness unavailable: {e}"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Hex of a signing key's seed (for `<name>.secret` files).
pub fn secret_hex(key: &SigningKey) -> String {
    hex_encode(&key.to_bytes())
}

/// Hex of a verifying key (for the trusted-keys TOML).
pub fn public_hex(key: &VerifyingKey) -> String {
    hex_encode(key.as_bytes())
}

/// What a signature is bound to (LANG-I-002). `path` is the identity the
/// receiver will use: the include path as written in the document, or the
/// path passed to `active verify`. Normalized to `/` separators.
#[derive(Debug, Clone, Copy)]
pub struct SignBinding<'a> {
    pub path: &'a str,
    pub role: Option<&'a str>,
    pub version: Option<&'a str>,
    pub expires: Option<&'a str>,
}

impl<'a> SignBinding<'a> {
    pub fn path_only(path: &'a str) -> Self {
        Self {
            path,
            role: None,
            version: None,
            expires: None,
        }
    }
}

/// Normalize an identity path: `/` separators, no `./` prefix, collapsed
/// duplicate separators. A caller that signs with one spelling and
/// verifies with another fails, which is the binding working.
pub fn normalize_identity(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for segment in unified.split('/') {
        match segment {
            "" | "." => {}
            other => parts.push(other),
        }
    }
    let mut out = parts.join("/");
    if unified.starts_with('/') {
        out.insert(0, '/');
    }
    out
}

fn identity_bytes(binding: &SignBinding<'_>) -> Vec<u8> {
    format!(
        "path={}\nrole={}\nversion={}\nexpires={}\n",
        normalize_identity(binding.path),
        binding.role.unwrap_or(""),
        binding.version.unwrap_or(""),
        binding.expires.unwrap_or("")
    )
    .into_bytes()
}

/// The 32-byte message that is signed: SHA-256 over the identity, a NUL
/// separator, and the file bytes.
fn signed_digest(binding: &SignBinding<'_>, bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(identity_bytes(binding));
    hasher.update([0u8]);
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Sign file bytes into a sidecar body. The key id is validated first —
/// raw interpolation into TOML would otherwise admit injection via
/// quotes, escapes, or newlines.
pub fn sign_bytes(
    key: &SigningKey,
    kid: &str,
    binding: &SignBinding<'_>,
    bytes: &[u8],
) -> Result<String, String> {
    if !valid_kid(kid) {
        return Err("refusing to sign under an invalid key id".to_string());
    }
    if binding.path.trim().is_empty() {
        return Err("refusing to sign without an identity path".to_string());
    }
    if let Some(expires) = binding.expires {
        if date_to_days(expires).is_none() {
            return Err("refusing to sign with a malformed expiry (want YYYY-MM-DD)".to_string());
        }
    }
    let signature = key.sign(&signed_digest(binding, bytes));
    let mut sidecar = format!("kid = \"{kid}\"\n");
    sidecar.push_str(&format!(
        "path = \"{}\"\n",
        normalize_identity(binding.path)
    ));
    if let Some(role) = binding.role {
        sidecar.push_str(&format!("role = \"{role}\"\n"));
    }
    if let Some(version) = binding.version {
        sidecar.push_str(&format!("version = \"{version}\"\n"));
    }
    if let Some(expires) = binding.expires {
        sidecar.push_str(&format!("expires = \"{expires}\"\n"));
    }
    sidecar.push_str(&format!(
        "sig = \"{}\"\n",
        hex_encode(&signature.to_bytes())
    ));
    Ok(sidecar)
}

/// Parsed sidecar: the claimed identity, key id, and raw signature.
pub struct Sidecar {
    pub kid: String,
    pub path: Option<String>,
    pub role: Option<String>,
    pub version: Option<String>,
    pub expires: Option<String>,
    pub signature: Signature,
}

fn string_field(doc: &toml::Value, key: &str) -> Result<Option<String>, String> {
    match doc.get(key) {
        None => Ok(None),
        Some(toml::Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("signature sidecar `{key}` must be a string")),
    }
}

/// Parse a sidecar document. Malformed sidecars are hard errors
/// (fail closed — never treated as unsigned). Key ids obey the
/// same charset as written sidecars, so echoed ids in errors are
/// log-safe by construction.
fn parse_sidecar(text: &str) -> Result<Sidecar, String> {
    let doc: toml::Value =
        toml::from_str(text).map_err(|_| "unparseable signature sidecar".to_string())?;
    let kid = doc
        .get("kid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "signature sidecar needs a string `kid`".to_string())?;
    if !valid_kid(kid) {
        return Err("signature sidecar holds an invalid key id".to_string());
    }
    let sig = doc
        .get("sig")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "signature sidecar needs a hex `sig`".to_string())?;
    let bytes = hex_bytes::<64>(sig.trim())
        .ok_or_else(|| "signature sidecar holds a malformed signature".to_string())?;
    let signature = Signature::from_slice(&bytes)
        .map_err(|_| "signature sidecar holds a malformed signature".to_string())?;
    Ok(Sidecar {
        kid: kid.to_string(),
        path: string_field(&doc, "path")?,
        role: string_field(&doc, "role")?,
        version: string_field(&doc, "version")?,
        expires: string_field(&doc, "expires")?,
        signature,
    })
}

/// Verify file bytes against sidecar text, the expected identity, and a
/// trusted-keys set. Returns the signing key id on success; every failure
/// carries a stable code (E032 malformed/unsigned/mismatch, E037 expired,
/// E038 revoked, E039 identity mismatch or unbound sidecar).
pub fn verify_bytes(
    file_label: &str,
    expected_path: &str,
    bytes: &[u8],
    sidecar_text: &str,
    trusted: &TrustedKeys,
) -> Result<String, VerifyFailure> {
    let failure = |code: &'static str, message: String| VerifyFailure { code, message };
    let sidecar =
        parse_sidecar(sidecar_text).map_err(|e| failure("E032", format!("{file_label}: {e}")))?;
    let bound_path = sidecar.path.as_deref().ok_or_else(|| {
        failure(
            "E039",
            format!(
                "{file_label}: sidecar is not bound to a path; re-sign it with `active sign` \
                 (LANG-I-002)"
            ),
        )
    })?;
    if normalize_identity(bound_path) != normalize_identity(expected_path) {
        return Err(failure(
            "E039",
            format!(
                "{file_label}: signature is bound to `{}`, not `{}`; a signed overlay cannot be \
                 replayed to another path",
                bound_path, expected_path
            ),
        ));
    }
    if trusted.revoked.contains(&sidecar.kid) {
        return Err(failure(
            "E038",
            format!(
                "{file_label}: signing key '{}' is revoked by the trust file",
                sidecar.kid
            ),
        ));
    }
    let key = trusted.key(&sidecar.kid).ok_or_else(|| {
        failure(
            "E032",
            format!("{file_label}: unknown signing key '{}'", sidecar.kid),
        )
    })?;
    if let Some(expires) = sidecar.expires.as_deref() {
        let expiry = date_to_days(expires).ok_or_else(|| {
            failure(
                "E032",
                format!("{file_label}: malformed `expires` (want YYYY-MM-DD)"),
            )
        })?;
        if today_days().is_some_and(|today| expiry < today) {
            return Err(failure(
                "E037",
                format!("{file_label}: signature expired on {expires}"),
            ));
        }
    }
    let binding = SignBinding {
        path: bound_path,
        role: sidecar.role.as_deref(),
        version: sidecar.version.as_deref(),
        expires: sidecar.expires.as_deref(),
    };
    // verify_strict rejects weak keys and non-canonical signatures
    // (LANG-I-008); plain verify would accept a torsion-component key.
    key.verify_strict(&signed_digest(&binding, bytes), &sidecar.signature)
        .map_err(|_| {
            failure(
                "E032",
                format!("{file_label}: signature mismatch (key '{}')", sidecar.kid),
            )
        })?;
    Ok(sidecar.kid)
}

/// Sidecar path for a file: `.sig` appended to the full filename
/// (`foo.active` ↔ `foo.active.sig`). Detached, so signatures
/// survive formatters, and distinct from other tools' habits.
pub fn sidecar_path_for(path: &std::path::Path) -> std::path::PathBuf {
    let mut named = path.as_os_str().to_owned();
    named.push(".sig");
    std::path::PathBuf::from(named)
}

/// Verify a file on disk plus its sibling sidecar: point-in-time
/// attestation for operators (TOCTOU between check and use is the
/// caller's concern). The identity is the path as given, normalized.
/// Enforcement paths that go on to PARSE the file must read once and
/// call `verify_bytes` on the buffer instead — see the include hook.
pub fn verify_file(path: &std::path::Path, trusted: &TrustedKeys) -> Result<String, VerifyFailure> {
    let label = path.to_string_lossy().into_owned();
    let bytes = std::fs::read(path).map_err(|e| VerifyFailure {
        code: "E032",
        message: format!("cannot read '{label}' for verification: {e}"),
    })?;
    let sidecar = read_sidecar(path, &label)?;
    verify_bytes(&label, &label, &bytes, &sidecar, trusted)
}

/// Read a sibling sidecar or report it missing (unsigned). Public
/// so enforcement paths share the exact unsigned wording.
pub fn read_sidecar(path: &std::path::Path, label: &str) -> Result<String, VerifyFailure> {
    let sidecar_path = sidecar_path_for(path);
    let sidecar_label = sidecar_path.to_string_lossy().into_owned();
    std::fs::read_to_string(&sidecar_path).map_err(|_| VerifyFailure {
        code: "E032",
        message: format!("{label}: unsigned (no signature sidecar at '{sidecar_label}')"),
    })
}

/// Days since 1970-01-01 for a `YYYY-MM-DD` date (proleptic Gregorian,
/// Howard Hinnant's algorithm). None for a malformed or impossible date.
fn date_to_days(text: &str) -> Option<i64> {
    let mut parts = text.trim().split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    if !(1..=max_day).contains(&day) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let month_pos = (month + 9) % 12;
    let day_of_year = (153 * month_pos + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn today_days() -> Option<i64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some((secs / 86_400) as i64)
}

/// Parse 2×N hex chars into N bytes (N=32 for keys, N=64 for
/// signatures). Anything else is malformed — never guessed.
fn hex_bytes<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.trim();
    if text.len() != 2 * N || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; N];
    for (i, chunk) in out.iter_mut().enumerate() {
        *chunk = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_dates_convert_to_days() {
        assert_eq!(date_to_days("1970-01-01"), Some(0));
        assert_eq!(date_to_days("2000-01-01"), Some(10_957));
        assert_eq!(date_to_days("2024-02-29"), Some(19_782));
        assert_eq!(date_to_days("2024-13-01"), None);
        assert_eq!(date_to_days("2024-02-31"), None);
        assert_eq!(date_to_days("not-a-date"), None);
    }
}
