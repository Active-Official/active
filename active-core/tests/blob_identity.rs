//! Tainted-blob identity is salted (LANG-C-010, spec/blob.md §1).
//!
//! The v1 identity is a public function of the payload, so anyone holding a
//! tainted blob's identity could dictionary-check a low-entropy secret
//! offline. Tainted blobs now hash under the `active-blob/v2` domain with a
//! per-document salt; untainted identity keeps the pinned v1 vectors.

use active_core::blob::{self, DOMAIN, DOMAIN_SALTED};
use active_core::{parse, Engine};

fn identity(src: &str, env: &[(&str, &str)]) -> String {
    let module = parse(src, "blob-v2").expect("parse");
    let mut engine = Engine::hermetic();
    let mut map = std::collections::HashMap::new();
    for (k, v) in env {
        map.insert(k.to_string(), v.to_string());
    }
    engine.env = map;
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    fn find_blob(value: &active_core::Value) -> Option<String> {
        match value {
            active_core::Value::Blob { hash, .. } => Some(blob::hash_hex(hash)),
            active_core::Value::Array(items) => items.iter().find_map(find_blob),
            active_core::Value::Table(map) => map.values().find_map(find_blob),
            _ => None,
        }
    }
    find_blob(&value).expect("document carries a blob")
}

const TAINTED: &str = "payload = secret(\"hunter2-tainted\")\nid = blob(\"text/plain\", payload)\n";
const CLEAN: &str = "id = blob(\"text/plain\", \"hello\")\n";

#[test]
fn untainted_identity_keeps_the_pinned_v1_vector() {
    // spec/blob.md section 2: `hello` under text/plain is a pinned digest,
    // and LANG-C-010 must not move it.
    assert_eq!(identity(CLEAN, &[]), blob::CANONICAL_TEST_DIGEST);
    assert_eq!(
        blob::hash_hex(&blob::hash(
            blob::CANONICAL_TEST_MEDIA,
            blob::CANONICAL_TEST_PAYLOAD
        )),
        blob::CANONICAL_TEST_DIGEST
    );
}

#[test]
fn a_promoted_host_blob_cannot_publish_an_unsalted_identity() {
    // A host-built blob with a false taint bit and a v1 hash over a
    // secret media type is inconsistent. The export must redact the
    // media type AND refuse to vouch for the guessable hash.
    use active_core::secret::SecretProfile;
    let media = "x-private-type";
    let bytes = b"public";
    let unsalted = blob::hash(media, bytes);
    let host = active_core::Value::Blob {
        hash: unsalted,
        size: bytes.len() as u64,
        tainted: false,
        media_type: media.into(),
        payload: None,
    };
    let module = parse("m = secret(\"x-private-type\")\n", "host-blob").expect("parse");
    let profile = SecretProfile::analyze(&module);
    let json = active_core::convert::value_to_json_with(&profile, &host);
    let identity = &json["$active.blob"];
    assert_eq!(identity["tainted"], true, "{json}");
    assert_eq!(identity["media_type"], "***SECRET***", "{json}");
    assert_ne!(
        identity["hash"],
        format!("blake3:{}", blob::hash_hex(&unsalted)),
        "the export must not publish the unverified v1 identity: {json}"
    );
    assert_eq!(
        identity["hash"],
        format!("blake3:{}", blob::hash_hex(&blob::redacted_identity())),
        "{json}"
    );
}

#[test]
fn a_media_type_embedding_a_short_secret_is_material_by_design() {
    // spec/blob.md §3: identity matching is broader than text redaction.
    // The export replaces the whole media string and withholds the
    // payload, so a short embedded secret cannot be published and no
    // unrelated text is rewritten. The identity is therefore salted.
    let short = "k = secret(\"ABC\")\nid = blob(\"xxABCxx\", \"hello\")\n";
    let tainted = identity(short, &[]);
    let unsalted = blob::hash_hex(&blob::hash("xxABCxx", b"hello"));
    assert_ne!(
        tainted, unsalted,
        "a media type embedding material must not publish the v1 identity"
    );

    // A media type with no material text keeps the v1 identity.
    let clean = "k = secret(\"ABC\")\nid = blob(\"text/plain\", \"hello\")\n";
    assert_eq!(
        identity(clean, &[]),
        blob::hash_hex(&blob::hash("text/plain", b"hello")),
        "an unrelated media type stays clean"
    );
}

#[test]
fn tainted_identity_differs_from_the_unsalted_v1_digest() {
    let tainted = identity(TAINTED, &[]);
    let unsalted = blob::hash_hex(&blob::hash("text/plain", b"hunter2-tainted"));
    assert_ne!(
        tainted, unsalted,
        "a tainted blob must not publish the v1 (unsalted) identity"
    );
    // The two domains are distinct byte streams, by construction.
    assert_ne!(DOMAIN, DOMAIN_SALTED);
    assert_eq!(DOMAIN, b"active-blob/v1");
    assert_eq!(DOMAIN_SALTED, b"active-blob/v2");
}

#[test]
fn tainted_identity_differs_per_document_salt() {
    // Same payload and media type, different documents: the derived salts
    // differ, so the identities differ (the v2 contract).
    let a = identity(TAINTED, &[]);
    let b = identity(&format!("extra = 1\n{TAINTED}"), &[]);
    assert_ne!(a, b, "the per-document salt must vary the identity");

    // An explicit host salt changes the identity too, deterministically.
    let module = parse(TAINTED, "salt").expect("parse");
    let mut one = Engine::hermetic();
    one.blob_salt = Some([1u8; 32]);
    let mut two = Engine::hermetic();
    two.blob_salt = Some([2u8; 32]);
    let mut same = Engine::hermetic();
    same.blob_salt = Some([1u8; 32]);
    let hash_of = |engine: &Engine| {
        let (value, _) = engine.evaluate(&module).expect("evaluate");
        match value {
            active_core::Value::Table(map) => match map.get("id") {
                Some(active_core::Value::Blob { hash, .. }) => blob::hash_hex(hash),
                other => panic!("id must be a blob: {other:?}"),
            },
            other => panic!("root must be a table: {other:?}"),
        }
    };
    let h1 = hash_of(&one);
    assert_ne!(h1, hash_of(&two), "different salts, different identities");
    assert_eq!(h1, hash_of(&same), "same salt, same identity");
}

#[test]
fn the_document_salt_is_reproducible_and_payload_blind() {
    let a = parse(TAINTED, "a").expect("parse");
    let b = parse(TAINTED, "b").expect("parse");
    assert_eq!(
        blob::document_salt(&a),
        blob::document_salt(&b),
        "the derived salt is a function of the document, not the run"
    );

    // The salt must not depend on the secret bytes themselves: two
    // documents with different secret literals but the same shape share a
    // salt (otherwise the salt leaks a hash of the secret).
    let s1 = parse(
        "payload = secret(\"alpha-secret\")\nid = blob(\"text/plain\", payload)\n",
        "s1",
    )
    .expect("parse");
    let s2 = parse(
        "payload = secret(\"bravo-secret\")\nid = blob(\"text/plain\", payload)\n",
        "s2",
    )
    .expect("parse");
    assert_eq!(
        blob::document_salt(&s1),
        blob::document_salt(&s2),
        "the salt hashes secret material by length only"
    );
}

#[test]
fn b002_does_not_echo_a_tainted_payload_size() {
    // A tainted blob over max_size: the breach is reported, the size is
    // not (it narrows the secret's entropy).
    let src = "schema = { closed = false, id = Blob(max_size = 4) }\n\
               payload = secret(\"hunter2-tainted\")\nid = blob(\"text/plain\", payload)\n";
    let module = parse(src, "b002").expect("parse");
    let engine = Engine::hermetic();
    let diags = match engine.evaluate(&module) {
        Ok((_, diags)) => diags,
        Err(active_core::EvalError::ValidationFailed { diagnostics }) => diagnostics,
        Err(other) => panic!("unexpected error: {other:?}"),
    };
    let b002: Vec<_> = diags.iter().filter(|d| d.code == "B002").collect();
    assert_eq!(b002.len(), 1, "{diags:?}");
    let message = &b002[0].message;
    assert!(
        !message.contains("blob size") && !message.contains("17"),
        "a tainted breach must not name the payload size: {message}"
    );

    // Control: an UNTAINTED oversized blob still reports its size.
    let src = "schema = { closed = false, id = Blob(max_size = 2) }\n\
               id = blob(\"text/plain\", \"hello\")\n";
    let module = parse(src, "b002-clean").expect("parse");
    let diags = match Engine::hermetic().evaluate(&module) {
        Ok((_, diags)) => diags,
        Err(active_core::EvalError::ValidationFailed { diagnostics }) => diagnostics,
        Err(other) => panic!("unexpected error: {other:?}"),
    };
    let b002: Vec<_> = diags.iter().filter(|d| d.code == "B002").collect();
    assert_eq!(b002.len(), 1, "{diags:?}");
    assert!(
        b002[0].message.contains("blob size 5"),
        "an untainted breach still names the size: {}",
        b002[0].message
    );
}

#[test]
fn dual_hash_transition_vectors_cover_both_domains() {
    // spec/blob.md section 1: the migration carries a vector per domain.
    // v1 is pinned by the constants; v2 is pinned here under a fixed salt
    // so a change to the salted layout fails this gate.
    assert_eq!(
        blob::hash_hex(&blob::hash(
            blob::CANONICAL_TEST_MEDIA,
            blob::CANONICAL_TEST_PAYLOAD
        )),
        blob::CANONICAL_TEST_DIGEST
    );
    let salt = [7u8; 32];
    let v2 = blob::hash_hex(&blob::hash_salted(
        &salt,
        blob::CANONICAL_TEST_MEDIA,
        blob::CANONICAL_TEST_PAYLOAD,
    ));
    assert_eq!(v2, V2_TEST_DIGEST, "salted vector must not drift");
    assert_ne!(v2, blob::CANONICAL_TEST_DIGEST, "domains are distinct");
}

/// Pinned `active-blob/v2` digest for `hello` under `text/plain` with a
/// salt of 32 bytes, each `0x07` (the transition vector above).
const V2_TEST_DIGEST: &str = "862fea5bb61f8f618a43769b4535d2600bf240c1882fdd7d6e9d033724b547dd";
