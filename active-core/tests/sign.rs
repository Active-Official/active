//! Signed overlays (C-052, spec/threat-model.md §4).
//!
//! Key loading, sign/verify round trips, tamper/unknown-key
//! failures, secret-free errors, include enforcement (E032) with legacy
//! preservation, and the LANG-I-002 bindings: path identity (E039),
//! expiry (E037), and revocation (E038).

use active_core::sign::TrustedKeys;
use active_core::sign::{
    generate_secret_key, load_secret_key, load_trusted_keys, public_hex, secret_hex, sign_bytes,
    verify_bytes, SignBinding,
};

fn keypair() -> (ed25519_dalek::SigningKey, String, String) {
    let secret = generate_secret_key().expect("keygen");
    let public = secret.verifying_key();
    let kid = "test-key".to_string();
    let secret_hex = secret_hex(&secret);
    let public_hex = public_hex(&public);
    let keys = format!("[[keys]]\nid = \"{kid}\"\npublic = \"{public_hex}\"\n");
    let trusted = load_trusted_keys(&keys).expect("trusted");
    assert!(trusted.keys.contains_key(&kid));
    (secret, secret_hex, kid)
}

fn trusted_of(kid: &str, secret: &ed25519_dalek::SigningKey) -> TrustedKeys {
    let public = secret.verifying_key();
    let keys = format!(
        "[[keys]]\nid = \"{kid}\"\npublic = \"{}\"\n",
        public_hex(&public)
    );
    load_trusted_keys(&keys).expect("trusted")
}

#[test]
fn sign_verify_round_trip() {
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    let bytes = b"host = \"db\"\n";
    let sidecar =
        sign_bytes(&secret, &kid, &SignBinding::path_only("lib.active"), bytes).expect("sign");
    assert_eq!(
        verify_bytes("lib.active", "lib.active", bytes, &sidecar, &trusted).expect("verify"),
        kid
    );
}

#[test]
fn tampered_bytes_unknown_kids_and_wrong_keys_fail() {
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    let bytes = b"port = 1\n";
    let sidecar =
        sign_bytes(&secret, &kid, &SignBinding::path_only("lib.active"), bytes).expect("sign");
    // Tampered content fails closed.
    let err = verify_bytes(
        "lib.active",
        "lib.active",
        b"port = 2\n",
        &sidecar,
        &trusted,
    )
    .expect_err("tamper");
    assert!(err.to_string().contains("signature mismatch"), "{err}");
    assert!(err.to_string().contains("lib.active"), "{err}");
    // Unknown key id fails closed.
    let foreign = generate_secret_key().expect("keygen");
    let foreign_sidecar = sign_bytes(
        &foreign,
        "stranger",
        &SignBinding::path_only("lib.active"),
        bytes,
    )
    .expect("sign");
    let err = verify_bytes(
        "lib.active",
        "lib.active",
        bytes,
        &foreign_sidecar,
        &trusted,
    )
    .expect_err("kid");
    assert!(err.to_string().contains("unknown signing key"), "{err}");
    // Malformed sidecars fail closed (never treated as unsigned).
    for bad in ["", "kid = \"x\"\n", "not toml [[["] {
        assert!(
            verify_bytes("lib.active", "lib.active", bytes, bad, &trusted).is_err(),
            "{bad:?}"
        );
    }
    // Invalid key ids are refused at every layer (spaces are
    // ugly but harmless: TOML-safe and single-line).
    for bad_kid in ["", "quote\"x", "back\\slash", "new\nline", "tab\there"] {
        assert!(!active_core::sign::valid_kid(bad_kid), "{bad_kid:?}");
        assert!(sign_bytes(
            &secret,
            bad_kid,
            &SignBinding::path_only("lib.active"),
            bytes
        )
        .is_err());
    }
    assert!(active_core::sign::valid_kid("has space"));

    assert!(active_core::sign::valid_kid(&kid));
}

#[test]
fn same_kid_different_key_fails() {
    // The critical confusion case: the kid matches a trusted id,
    // but the signature came from another key entirely.
    let (secret, _, kid) = keypair();
    let other = generate_secret_key().expect("keygen");
    let trusted = trusted_of(&kid, &secret);
    let bytes = b"port = 1\n";
    let sidecar =
        sign_bytes(&other, &kid, &SignBinding::path_only("lib.active"), bytes).expect("sign");
    let err =
        verify_bytes("lib.active", "lib.active", bytes, &sidecar, &trusted).expect_err("must fail");
    assert!(err.to_string().contains("signature mismatch"), "{err}");
}

#[test]
fn a_signed_overlay_cannot_be_replayed_to_another_path() {
    // LANG-I-002: the signature binds the identity path, so a signed
    // library copied to another path fails closed.
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    let bytes = b"host = \"db\"\n";
    let sidecar = sign_bytes(
        &secret,
        &kid,
        &SignBinding::path_only("inc/lib.active"),
        bytes,
    )
    .expect("sign");
    assert!(verify_bytes(
        "inc/lib.active",
        "inc/lib.active",
        bytes,
        &sidecar,
        &trusted
    )
    .is_ok());
    let err = verify_bytes(
        "inc/other.active",
        "inc/other.active",
        bytes,
        &sidecar,
        &trusted,
    )
    .expect_err("replay must fail");
    assert_eq!(err.code, "E039");
    assert!(err.message.contains("bound to"), "{err}");
    // Backslash spellings normalize to the same identity.
    assert!(verify_bytes(
        "inc\\lib.active",
        "inc/lib.active",
        bytes,
        &sidecar,
        &trusted
    )
    .is_ok());
}

#[test]
fn an_expired_sidecar_fails_closed() {
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    let bytes = b"host = \"db\"\n";
    let expired = SignBinding {
        path: "lib.active",
        role: None,
        version: None,
        expires: Some("2000-01-01"),
    };
    let sidecar = sign_bytes(&secret, &kid, &expired, bytes).expect("sign");
    let err = verify_bytes("lib.active", "lib.active", bytes, &sidecar, &trusted)
        .expect_err("expired must fail");
    assert_eq!(err.code, "E037");
    assert!(err.message.contains("expired"), "{err}");
    // A far-future expiry verifies; the same bytes rebound to it differ.
    let live = SignBinding {
        expires: Some("2999-12-31"),
        ..SignBinding::path_only("lib.active")
    };
    let sidecar = sign_bytes(&secret, &kid, &live, bytes).expect("sign");
    assert!(verify_bytes("lib.active", "lib.active", bytes, &sidecar, &trusted).is_ok());
}

#[test]
fn a_revoked_key_fails_closed() {
    let (secret, _, kid) = keypair();
    let public = public_hex(&secret.verifying_key());
    let keys = format!("revoked = [\"{kid}\"]\n[[keys]]\nid = \"{kid}\"\npublic = \"{public}\"\n");
    let trusted = load_trusted_keys(&keys).expect("trusted");
    let sidecar = sign_bytes(
        &secret,
        &kid,
        &SignBinding::path_only("lib.active"),
        b"host = \"db\"\n",
    )
    .expect("sign");
    let err = verify_bytes(
        "lib.active",
        "lib.active",
        b"host = \"db\"\n",
        &sidecar,
        &trusted,
    )
    .expect_err("revoked must fail");
    assert_eq!(err.code, "E038");
    assert!(err.message.contains("revoked"), "{err}");
}

/// Sign a file in place: write its sibling `.sig` sidecar bound to its
/// file name (the fixture documents include it by bare name).
fn sign_file(secret: &ed25519_dalek::SigningKey, kid: &str, path: &std::path::Path) {
    let bytes = std::fs::read(path).expect("read for signing");
    let identity = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .expect("file name");
    let sidecar =
        sign_bytes(secret, kid, &SignBinding::path_only(&identity), &bytes).expect("sign");
    let mut sidecar_path = path.as_os_str().to_owned();
    sidecar_path.push(".sig");
    std::fs::write(&sidecar_path, sidecar).expect("write sig");
}

/// Sign a file bound to an explicit identity (the root document is
/// verified under the path the caller passed).
fn sign_root(secret: &ed25519_dalek::SigningKey, kid: &str, path: &std::path::Path) {
    let bytes = std::fs::read(path).expect("read for signing");
    let identity = path.to_string_lossy().into_owned();
    let sidecar =
        sign_bytes(secret, kid, &SignBinding::path_only(&identity), &bytes).expect("sign");
    let mut sidecar_path = path.as_os_str().to_owned();
    sidecar_path.push(".sig");
    std::fs::write(&sidecar_path, sidecar).expect("write sig");
}

fn temp_fixture(name: &str, contents: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("active-sign-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    path
}

#[test]
fn nested_includes_enforce_recursively() {
    use active_core::{parse_file_with_trust, ParseOptions};
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    // main → mid → leaf: all signed passes.
    let leaf = temp_fixture("nestleaf.active", "deep = 1\n");
    let mid = temp_fixture("nestmid.active", "include \"nestleaf.active\"\nmid = 1\n");
    let main = temp_fixture("nestmain.active", "include \"nestmid.active\"\nport = 1\n");
    // LANG-C-007: the ROOT is verified too; its identity is the path the
    // caller passes, while includes bind their literal spelling.
    sign_file(&secret, &kid, &leaf);
    sign_file(&secret, &kid, &mid);
    sign_root(&secret, &kid, &main);
    let module =
        parse_file_with_trust(&main, ParseOptions::default(), &trusted).expect("all signed");
    assert!(module.resolve_value_path("deep").is_some());
    // Unsigned leaf two levels down still fails closed.
    std::fs::remove_file(format!("{}.sig", leaf.display())).expect("unsign leaf");
    let diags = parse_file_with_trust(&main, ParseOptions::default(), &trusted)
        .expect_err("unsigned leaf must fail");
    assert!(diags.iter().any(|d| d.code == "E032"), "{diags:?}");
}

#[test]
fn secret_key_files_round_trip_and_reject_garbage() {
    let (secret, hex, _) = keypair();
    assert_eq!(hex.len(), 64);
    let reloaded = load_secret_key(&hex).expect("reload");
    assert_eq!(reloaded.to_bytes(), secret.to_bytes());
    assert_eq!(
        reloaded.verifying_key().as_bytes(),
        secret.verifying_key().as_bytes()
    );
    for bad in ["", "xyz", "00", &"ab".repeat(31)] {
        assert!(load_secret_key(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn trusted_keys_reject_malformed_sets() {
    assert!(load_trusted_keys("").is_err());
    assert!(load_trusted_keys("[[keys]]\nid = \"a\"\n").is_err());
    assert!(load_trusted_keys("[[keys]]\nid = \"a\"\npublic = \"zz\"\n").is_err());
    assert!(load_trusted_keys("[[keys]]\nid = \"a\"\npublic = \"00\"\n").is_err());
    let dup = "[[keys]]\nid = \"a\"\npublic = \"".to_string()
        + &"11".repeat(32)
        + "\"\n[[keys]]\nid = \"a\"\npublic = \""
        + &"11".repeat(32)
        + "\"\n";
    assert!(load_trusted_keys(&dup).is_err());
    // Revocation list shape is validated too.
    assert!(load_trusted_keys("revoked = \"a\"\n[[keys]]\nid = \"a\"\npublic = \"11\"\n").is_err());
    assert!(load_trusted_keys(&format!(
        "revoked = [1]\n[[keys]]\nid = \"a\"\npublic = \"{}\"\n",
        "11".repeat(32)
    ))
    .is_err());
}

#[test]
fn errors_carry_no_key_material() {
    // Even adversarial inputs must not echo secrets or keys.
    let (secret, secret_hex_str, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    let err = verify_bytes("f", "f", b"x", "garbage", &trusted).expect_err("bad sidecar");
    assert!(!err.to_string().contains(&secret_hex_str), "{err}");
    let err = toml_parse_probe();
    assert!(!err.contains(&secret_hex_str), "{err}");
}

fn toml_parse_probe() -> String {
    // A malformed trusted-keys file mentioning nothing secret.
    load_trusted_keys("[[keys]]\n").expect_err("must fail")
}

#[test]
fn include_enforcement_accepts_signed_and_rejects_the_rest() {
    use active_core::{parse_file_with_trust, ParseOptions};
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    // Signed library: verifies and merges.
    let lib = temp_fixture("signedlib.active", "host = \"db\"\n");
    sign_file(&secret, &kid, &lib);
    let main = temp_fixture(
        "signedmain.active",
        "include \"signedlib.active\"\nport = 1\n",
    );
    // The receiver verifies the document it was handed (LANG-C-007), so the
    // root carries a signature here as well.
    sign_root(&secret, &kid, &main);
    let module =
        parse_file_with_trust(&main, ParseOptions::default(), &trusted).expect("signed parses");
    assert!(module.resolve_value_path("host").is_some());
    // Unsigned library: E032, fail-closed before parsing.
    let _lib2 = temp_fixture("unsignedlib.active", "host = \"db\"\n");
    let main2 = temp_fixture(
        "unsignedmain.active",
        "include \"unsignedlib.active\"\nport = 1\n",
    );
    sign_root(&secret, &kid, &main2);
    let diags = parse_file_with_trust(&main2, ParseOptions::default(), &trusted)
        .expect_err("unsigned must fail");
    assert!(diags.iter().any(|d| d.code == "E032"), "{diags:?}");
    assert!(
        diags.iter().all(|d| !format!("{d:?}").contains("host = ")),
        "{diags:?}"
    );
    // Legacy entry point is unaffected by design (no enforcement).
    let module = active_core::parse_file(&main2, ParseOptions::default()).expect("legacy parses");
    assert!(module.resolve_value_path("host").is_some());
    // Tampered library: E032 even with a sidecar present.
    std::fs::write(&lib, "host = \"evil\"\n").expect("tamper lib");
    let diags = parse_file_with_trust(&main, ParseOptions::default(), &trusted)
        .expect_err("tampered must fail");
    assert!(diags.iter().any(|d| d.code == "E032"), "{diags:?}");
}

#[test]
fn the_root_document_is_verified_against_the_trust_set() {
    // LANG-C-007: before this, --trust-keys authenticated includes only and
    // a receiver acted on an unsigned root. Same key material, same wording.
    use active_core::{parse_file_with_trust, ParseOptions};
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);

    // Signed root: parses.
    let root = temp_fixture("rootverifysigned.active", "port = 1\n");
    sign_root(&secret, &kid, &root);
    let module = parse_file_with_trust(&root, ParseOptions::default(), &trusted)
        .expect("signed root parses");
    assert!(module.resolve_value_path("port").is_some());

    // Unsigned root: E032 naming the missing sidecar.
    let unsigned = temp_fixture("rootverifyunsigned.active", "port = 1\n");
    let diags = parse_file_with_trust(&unsigned, ParseOptions::default(), &trusted)
        .expect_err("unsigned root must fail");
    assert!(diags.iter().any(|d| d.code == "E032"), "{diags:?}");
    assert!(
        diags[0].message.contains("unsigned"),
        "the failure names the unsigned root: {}",
        diags[0].message
    );
    assert_eq!(
        diags[0].file.as_deref(),
        Some(unsigned.to_string_lossy().as_ref())
    );

    // Tampered root: the sidecar no longer matches the bytes read.
    std::fs::write(&root, "port = 2\n").expect("tamper root");
    let diags = parse_file_with_trust(&root, ParseOptions::default(), &trusted)
        .expect_err("tampered root must fail");
    assert!(diags[0].message.contains("signature mismatch"), "{diags:?}");

    // Root signed by a key outside the trust set: unknown key id.
    let foreign = generate_secret_key().expect("keygen");
    let foreign_root = temp_fixture("rootverifyforeign.active", "port = 1\n");
    sign_root(&foreign, "stranger", &foreign_root);
    let diags = parse_file_with_trust(&foreign_root, ParseOptions::default(), &trusted)
        .expect_err("untrusted key must fail");
    assert!(
        diags[0].message.contains("unknown signing key"),
        "{diags:?}"
    );

    // Negative control: without a trust set the legacy entry point accepts
    // the mismatched file (verification is opt-in, exactly as documented).
    assert!(active_core::parse_file(&root, ParseOptions::default()).is_ok());
}

#[test]
fn the_root_path_binding_rejects_a_renamed_document() {
    // The root's identity is the path the receiver passed. Signing under
    // one name and verifying under another is E039 (LANG-I-002).
    use active_core::{parse_file_with_trust, ParseOptions};
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    let root = temp_fixture("rootbind.active", "port = 1\n");
    sign_root(&secret, &kid, &root);
    // Copy the bytes and sidecar to another path: the sidecar still names
    // the old identity, so the receiver refuses.
    let renamed = temp_fixture("rootbindrenamed.active", "port = 1\n");
    let mut sidecar = std::fs::read(format!("{}.sig", root.display())).expect("sig");
    sidecar.push(b'\n');
    std::fs::write(format!("{}.sig", renamed.display()), sidecar).expect("copy sig");
    let diags = parse_file_with_trust(&renamed, ParseOptions::default(), &trusted)
        .expect_err("renamed root must fail");
    assert!(
        diags.iter().any(|d| d.code == "E039"),
        "expected E039: {diags:?}"
    );
}

#[test]
fn text_with_trust_verifies_the_supplied_bytes_not_the_path() {
    // L-P62 H1: a tool that holds the candidate text must verify THAT
    // text. A path re-read would let a swap decide the verdict.
    use active_core::{parse_text_with_trust, ParseOptions};
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    let path = temp_fixture("text-trust.active", "port = 8080\n");
    sign_root(&secret, &kid, &path);
    // The exact signed bytes parse under trust.
    parse_text_with_trust(
        "port = 8080\n",
        &path.to_string_lossy(),
        ParseOptions::default(),
        &path,
        &trusted,
    )
    .expect("matching bytes verify");
    // Different bytes (a swap) reject with the signature code even though
    // the path still holds the signed content.
    let error = parse_text_with_trust(
        "port = 9999\n",
        &path.to_string_lossy(),
        ParseOptions::default(),
        &path,
        &trusted,
    )
    .expect_err("swapped bytes must reject");
    assert!(
        error.iter().any(|d| d.code == "E032"),
        "expected E032: {error:?}"
    );
}
