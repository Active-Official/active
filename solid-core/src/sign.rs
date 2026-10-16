//! Signed overlays (C-052, spec/threat-model.md §4).
//!
//! Detached Ed25519 signatures over raw file bytes: `<file>.sig`
//! sidecars naming the signing key (`kid`) plus the hex signature.
//! Verification resolves `kid` against a trusted-keys set; enforcement
//! hooks into include loading via `ParseOptions` (E032). Key files
//! are bearer credentials (0600, never committed, never logged).

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::collections::HashMap;

/// A trusted key set: key id → public key. Cloned into the parser
/// when overlay enforcement is on (small maps; no lifetimes).
pub type TrustedKeys = HashMap<String, VerifyingKey>;

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

/// Parse a trusted-keys TOML document (`[[keys]] id/public`) into an
/// id-keyed map. Malformed entries are hard errors naming the entry,
/// never key material.
pub fn load_trusted_keys(text: &str) -> Result<HashMap<String, VerifyingKey>, String> {
    let doc: toml::Value =
        toml::from_str(text).map_err(|e| format!("invalid trusted-keys file: {e}"))?;
    let keys = doc
        .get("keys")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "trusted-keys file needs a [[keys]] array".to_string())?;
    let mut out = HashMap::new();
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
        if out.insert(id.to_string(), key).is_some() {
            return Err(format!("duplicate trusted-keys id '{id}'"));
        }
    }
    if out.is_empty() {
        return Err("trusted-keys file contains no [[keys]] entries".to_string());
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

/// Sign raw bytes: the sidecar body for `<file>.sig`. The key id is
/// validated first — raw interpolation into TOML would otherwise
/// admit injection via quotes, escapes, or newlines.
pub fn sign_bytes(key: &SigningKey, kid: &str, bytes: &[u8]) -> Result<String, String> {
    if !valid_kid(kid) {
        return Err("refusing to sign under an invalid key id".to_string());
    }
    let signature = key.sign(bytes);
    Ok(format!(
        "kid = \"{kid}\"\nsig = \"{}\"\n",
        hex_encode(&signature.to_bytes())
    ))
}

/// Parsed sidecar: the claiming key id plus the raw signature.
pub struct Sidecar {
    pub kid: String,
    pub signature: Signature,
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
        signature,
    })
}

/// Verify file bytes against sidecar text and a trusted-keys map.
/// Returns the signing key id on success; every failure names the
/// file and the reason, never key material.
pub fn verify_bytes(
    file_label: &str,
    bytes: &[u8],
    sidecar_text: &str,
    trusted: &HashMap<String, VerifyingKey>,
) -> Result<String, String> {
    let sidecar = parse_sidecar(sidecar_text)?;
    let key = trusted
        .get(&sidecar.kid)
        .ok_or_else(|| format!("{file_label}: unknown signing key '{}'", sidecar.kid))?;
    key.verify(bytes, &sidecar.signature)
        .map_err(|_| format!("{file_label}: signature mismatch (key '{}')", sidecar.kid))?;
    Ok(sidecar.kid)
}

/// Sidecar path for a file: `.sig` appended to the full filename
/// (`foo.solid` ↔ `foo.solid.sig`). Detached, so signatures
/// survive formatters, and distinct from other tools' habits.
pub fn sidecar_path_for(path: &std::path::Path) -> std::path::PathBuf {
    let mut named = path.as_os_str().to_owned();
    named.push(".sig");
    std::path::PathBuf::from(named)
}

/// Verify a file on disk plus its sibling sidecar: point-in-time
/// attestation for operators (TOCTOU between check and use is the
/// caller's concern). Enforcement paths that go on to PARSE the
/// file must read once and call `verify_bytes` on the buffer
/// instead — see the include hook.
pub fn verify_file(
    path: &std::path::Path,
    trusted: &HashMap<String, VerifyingKey>,
) -> Result<String, String> {
    let label = path.to_string_lossy().into_owned();
    let bytes =
        std::fs::read(path).map_err(|e| format!("cannot read '{label}' for verification: {e}"))?;
    let sidecar = read_sidecar(path, &label)?;
    verify_bytes(&label, &bytes, &sidecar, trusted)
}

/// Read a sibling sidecar or report it missing (unsigned). Public
/// so enforcement paths share the exact unsigned wording.
pub fn read_sidecar(path: &std::path::Path, label: &str) -> Result<String, String> {
    let sidecar_path = sidecar_path_for(path);
    let sidecar_label = sidecar_path.to_string_lossy().into_owned();
    std::fs::read_to_string(&sidecar_path)
        .map_err(|_| format!("{label}: unsigned (no signature sidecar at '{sidecar_label}')"))
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
