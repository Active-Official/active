//! Content-addressed values + Blob (PROPOSAL §1.1, CHECKLIST Phase CA).
//!
//! Canonical byte representation (spec/blob.md §1): the identity of a
//! blob is BLAKE3-256 over `active-blob/v1 || 0x00 || le64(len(media))
//! || media || le64(len(payload)) || payload`. Hashes never appear in
//! ordinary Active source; only in evaluated JSON, lockfiles, and
//! provenance. The B00x diagnostic family is defined here.

/// Domain separator prefix (spec/blob.md §1).
pub const DOMAIN: &[u8] = b"active-blob/v1";

/// Salted domain for TAINTED blobs (spec/blob.md section 1, LANG-C-010). The
/// v1 identity is a public function of the payload, so anyone holding a
/// tainted blob's identity could dictionary-check a low-entropy secret
/// offline. Tainted blobs hash under this domain with a per-document salt;
/// untainted identity stays v1 and keeps its pinned vectors.
pub const DOMAIN_SALTED: &[u8] = b"active-blob/v2";

/// Pinned test digest (spec/blob.md §2): BLAKE3 over the canonical
/// bytes for payload `hello` under media type `text/plain`.
/// Hex: `ef8a8525ffd1d32141916f461ccb49c92ee84d57279145268364fb7c1c179eea`.
pub const CANONICAL_TEST_DIGEST: &str =
    "ef8a8525ffd1d32141916f461ccb49c92ee84d57279145268364fb7c1c179eea";
pub const CANONICAL_TEST_MEDIA: &str = "text/plain";
pub const CANONICAL_TEST_PAYLOAD: &[u8] = b"hello";

/// Canonical bytes for hashing (spec/blob.md §1). Fixed order, no
/// normalisation: the same payload under a different media type is a
/// different blob by construction.
pub fn canonical_bytes(media_type: &str, payload: &[u8]) -> Vec<u8> {
    let media = media_type.as_bytes();
    let mut out = Vec::with_capacity(DOMAIN.len() + 1 + 8 + media.len() + 8 + payload.len());
    out.extend_from_slice(DOMAIN);
    out.push(0x00);
    out.extend_from_slice(&(media.len() as u64).to_le_bytes());
    out.extend_from_slice(media);
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// BLAKE3-256 identity digest of a blob (spec/blob.md §2). Hard
/// dependency: no silent fallback to another algorithm — if BLAKE3
/// is unavailable the build fails instead of downgrading.
pub fn hash(media_type: &str, payload: &[u8]) -> [u8; 32] {
    *blake3::hash(&canonical_bytes(media_type, payload)).as_bytes()
}

/// Canonical bytes under the salted (`active-blob/v2`) domain: the v1
/// layout with the salt between the domain separator and the media type.
pub fn canonical_bytes_salted(salt: &[u8; 32], media_type: &str, payload: &[u8]) -> Vec<u8> {
    let media = media_type.as_bytes();
    let mut out = Vec::with_capacity(
        DOMAIN_SALTED.len() + 1 + 8 + salt.len() + 8 + media.len() + 8 + payload.len(),
    );
    out.extend_from_slice(DOMAIN_SALTED);
    out.push(0x00);
    out.extend_from_slice(&(salt.len() as u64).to_le_bytes());
    out.extend_from_slice(salt);
    out.extend_from_slice(&(media.len() as u64).to_le_bytes());
    out.extend_from_slice(media);
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Identity digest of a TAINTED blob under the salted domain.
pub fn hash_salted(salt: &[u8; 32], media_type: &str, payload: &[u8]) -> [u8; 32] {
    *blake3::hash(&canonical_bytes_salted(salt, media_type, payload)).as_bytes()
}

/// Per-document salt for tainted blob identities (LANG-C-010).
///
/// Derived from the document's SHAPE: node kinds, table keys, array
/// lengths, identifiers, and scalar literals. Secret payloads and blob
/// payloads contribute their presence only, so the salt never becomes an
/// oracle for the bytes it protects. Deterministic for a given document
/// (the identity contract and the cross-embedding gates need that); a host
/// that wants a per-process random salt sets `Engine::blob_salt`.
pub fn document_salt(module: &crate::ir::Module) -> [u8; 32] {
    use crate::ir::{Node, Value};
    fn scalar(hasher: &mut blake3::Hasher, value: &Value) {
        match value {
            Value::Null => {
                hasher.update(b"n");
            }
            Value::Bool(b) => {
                hasher.update(if *b { b"t" } else { b"f" });
            }
            Value::Int(i) => {
                hasher.update(&i.to_le_bytes());
            }
            Value::Float(f) => {
                hasher.update(&f.to_bits().to_le_bytes());
            }
            // Strings may be secret-derived: length only, never bytes.
            Value::String(s) => {
                hasher.update(b"s");
                hasher.update(&(s.len() as u64).to_le_bytes());
            }
            // Secrets hash by length only, never bytes (same rule).
            Value::Secret(s) => {
                hasher.update(b"s");
                hasher.update(&(s.len() as u64).to_le_bytes());
            }
            Value::DurationMs(ms) => {
                hasher.update(b"d");
                hasher.update(&ms.to_le_bytes());
            }
            Value::Quantity(_, unit) => {
                hasher.update(b"q");
                hasher.update(unit.suffix().as_bytes());
            }
            Value::Blob {
                media_type,
                tainted,
                ..
            } => {
                hasher.update(b"b");
                hasher.update(media_type.as_bytes());
                hasher.update(if *tainted { b"t" } else { b"u" });
            }
            Value::Table(map) => {
                hasher.update(b"T");
                for key in map.keys() {
                    hasher.update(key.as_bytes());
                    hasher.update(b"\0");
                }
            }
            Value::Array(items) => {
                hasher.update(b"A");
                hasher.update(&(items.len() as u64).to_le_bytes());
            }
        }
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"active-doc-salt/v1");
    for (index, node) in module.arena.iter().enumerate() {
        hasher.update(&(index as u64).to_le_bytes());
        match node {
            Node::Literal(value) => {
                hasher.update(b"L");
                scalar(&mut hasher, value);
            }
            Node::Table { fields } => {
                hasher.update(b"T");
                for key in fields.keys() {
                    hasher.update(key.as_bytes());
                    hasher.update(b"\0");
                }
            }
            Node::Array { elements } => {
                hasher.update(b"A");
                hasher.update(&(elements.len() as u64).to_le_bytes());
            }
            // Presence only: these carry the bytes the salt protects.
            Node::Secret { .. } => {
                hasher.update(b"S");
            }
            Node::ContentAddressed { .. } => {
                hasher.update(b"B");
            }
            Node::Ident(name) => {
                hasher.update(b"I");
                hasher.update(name.as_bytes());
            }
            Node::Ref(_) => {
                hasher.update(b"R");
            }
            Node::Env { name, .. } => {
                hasher.update(b"E");
                hasher.update(name.as_bytes());
            }
            Node::Interpolate { parts } => {
                hasher.update(b"P");
                hasher.update(&(parts.len() as u64).to_le_bytes());
            }
            Node::Binary { .. } => {
                hasher.update(b"O");
            }
            Node::Unary { .. } => {
                hasher.update(b"U");
            }
            Node::Quantity { unit, .. } => {
                hasher.update(b"Q");
                hasher.update(unit.suffix().as_bytes());
            }
            Node::SchemaTable { fields, .. } => {
                hasher.update(b"ST");
                for key in fields.keys() {
                    hasher.update(key.as_bytes());
                    hasher.update(b"\0");
                }
            }
            _ => {
                hasher.update(b"?");
            }
        }
    }
    *hasher.finalize().as_bytes()
}

/// Lowercase hex rendering of an identity digest.
pub fn hash_hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Synthetic identity for a blob the export must treat as tainted but
/// whose evaluated v2 identity is unknown: a host-built `Value::Blob`
/// with `tainted: false` whose content or serving path is material. The
/// export refuses to publish the unverified hash - a v1 hash over a
/// secret media type is an offline guess oracle - and this marker digest
/// carries no information about the withheld bytes. Evaluated blobs
/// already carry a real salted identity and never use it.
pub fn redacted_identity() -> [u8; 32] {
    let mut salt = [0u8; 32];
    salt[..6].copy_from_slice(b"redact");
    hash_salted(&salt, "active-blob/redacted-identity", &[])
}

/// Stable diagnostic codes for the blob family (spec/blob.md §8).
/// Messages live with their raiser; codes are the contract.
pub mod code {
    /// Malformed `blob(...)` call (parse).
    pub const BAD_FORM: &str = "B001";
    /// Blob payload exceeds schema `max_size` (validation).
    pub const TOO_LARGE: &str = "B002";
    /// Materialisation refused: blob is tainted (eval).
    pub const TAINTED_MATERIALISE: &str = "B003";
    /// Blob media type mismatches schema `media_type` (validation).
    pub const MEDIA_MISMATCH: &str = "B004";
    /// Materialisation impossible: payload not retained (eval).
    pub const NO_PAYLOAD: &str = "B005";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_digest_is_pinned() {
        // spec/blob.md §2: any digest change fails the gate.
        assert_eq!(
            hash_hex(&hash(CANONICAL_TEST_MEDIA, CANONICAL_TEST_PAYLOAD)),
            CANONICAL_TEST_DIGEST
        );
    }

    #[test]
    fn media_type_changes_identity() {
        assert_ne!(
            hash("text/plain", b"hello"),
            hash("Text/Plain", b"hello"),
            "no case folding: exact match only"
        );
    }

    #[test]
    fn canonical_layout_matches_spec_hex() {
        let bytes = canonical_bytes("text/plain", b"hello");
        assert_eq!(
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "6163746976652d626c6f622f7631000a00000000000000746578742f706c61696e050000000000000068656c6c6f"
        );
    }
}
