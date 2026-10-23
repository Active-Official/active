//! Content-addressed values + Blob (PROPOSAL §1.1, CHECKLIST Phase CA).
//!
//! Canonical byte representation (spec/blob.md §1): the identity of a
//! blob is BLAKE3-256 over `active-blob/v1 || 0x00 || le64(len(media))
//! || media || le64(len(payload)) || payload`. Hashes never appear in
//! ordinary Active source; only in evaluated JSON, lockfiles, and
//! provenance. The B00x diagnostic family is defined here.

/// Domain separator prefix (spec/blob.md §1).
pub const DOMAIN: &[u8] = b"active-blob/v1";

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

/// Lowercase hex rendering of an identity digest.
pub fn hash_hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
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
