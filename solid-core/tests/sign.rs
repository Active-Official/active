//! Signed overlays (C-052, spec/threat-model.md §4).
//!
//! Key loading, sign/verify round trips, tamper/unknown-key
//! failures, secret-free errors, and include enforcement (E032)
//! with legacy preservation.

use solid_core::sign::{
    generate_secret_key, load_secret_key, load_trusted_keys, public_hex, secret_hex, sign_bytes,
    verify_bytes,
};
use std::collections::HashMap;

fn keypair() -> (ed25519_dalek::SigningKey, String, String) {
    let secret = generate_secret_key().expect("keygen");
    let public = secret.verifying_key();
    let kid = "test-key".to_string();
    let secret_hex = secret_hex(&secret);
    let public_hex = public_hex(&public);
    let keys = format!("[[keys]]\nid = \"{kid}\"\npublic = \"{public_hex}\"\n");
    let trusted = load_trusted_keys(&keys).expect("trusted");
    assert!(trusted.contains_key(&kid));
    (secret, secret_hex, kid)
}

fn trusted_of(
    kid: &str,
    secret: &ed25519_dalek::SigningKey,
) -> HashMap<String, ed25519_dalek::VerifyingKey> {
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
    let sidecar = sign_bytes(&secret, &kid, bytes).expect("sign");
    assert_eq!(
        verify_bytes("lib.solid", bytes, &sidecar, &trusted).expect("verify"),
        kid
    );
}

#[test]
fn tampered_bytes_unknown_kids_and_wrong_keys_fail() {
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    let bytes = b"port = 1\n";
    let sidecar = sign_bytes(&secret, &kid, bytes).expect("sign");
    // Tampered content fails closed.
    let err = verify_bytes("lib.solid", b"port = 2\n", &sidecar, &trusted).expect_err("tamper");
    assert!(err.contains("signature mismatch"), "{err}");
    assert!(err.contains("lib.solid"), "{err}");
    // Unknown key id fails closed.
    let foreign = generate_secret_key().expect("keygen");
    let foreign_sidecar = sign_bytes(&foreign, "stranger", bytes).expect("sign");
    let err = verify_bytes("lib.solid", bytes, &foreign_sidecar, &trusted).expect_err("kid");
    assert!(err.contains("unknown signing key"), "{err}");
    // Malformed sidecars fail closed (never treated as unsigned).
    for bad in ["", "kid = \"x\"\n", "not toml [[["] {
        assert!(
            verify_bytes("lib.solid", bytes, bad, &trusted).is_err(),
            "{bad:?}"
        );
    }
    // Invalid key ids are refused at every layer (spaces are
    // ugly but harmless: TOML-safe and single-line).
    for bad_kid in ["", "quote\"x", "back\\slash", "new\nline", "tab\there"] {
        assert!(!solid_core::sign::valid_kid(bad_kid), "{bad_kid:?}");
        assert!(sign_bytes(&secret, bad_kid, bytes).is_err());
    }
    assert!(solid_core::sign::valid_kid("has space"));

    assert!(solid_core::sign::valid_kid(&kid));
}

#[test]
fn same_kid_different_key_fails() {
    // The critical confusion case: the kid matches a trusted id,
    // but the signature came from another key entirely.
    let (secret, _, kid) = keypair();
    let other = generate_secret_key().expect("keygen");
    let trusted = trusted_of(&kid, &secret);
    let bytes = b"port = 1\n";
    let sidecar = sign_bytes(&other, &kid, bytes).expect("sign");
    let err = verify_bytes("lib.solid", bytes, &sidecar, &trusted).expect_err("must fail");
    assert!(err.contains("signature mismatch"), "{err}");
}

#[test]
fn nested_includes_enforce_recursively() {
    use solid_core::{parse_file_with_trust, ParseOptions};
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    let sign = |path: &std::path::Path| {
        let bytes = std::fs::read(path).expect("read");
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(".sig");
        std::fs::write(&sidecar, sign_bytes(&secret, &kid, &bytes).expect("sign"))
            .expect("write sig");
    };
    // main → mid → leaf: all signed passes.
    let leaf = temp_fixture("nestleaf.solid", "deep = 1\n");
    let mid = temp_fixture("nestmid.solid", "include \"nestleaf.solid\"\nmid = 1\n");
    let main = temp_fixture("nestmain.solid", "include \"nestmid.solid\"\nport = 1\n");
    for path in [&leaf, &mid] {
        sign(path);
    }
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
}

#[test]
fn errors_carry_no_key_material() {
    // Even adversarial inputs must not echo secrets or keys.
    let (secret, secret_hex_str, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    let err = verify_bytes("f", b"x", "garbage", &trusted).expect_err("bad sidecar");
    assert!(!err.contains(&secret_hex_str), "{err}");
    let err = toml_parse_probe();
    assert!(!err.contains(&secret_hex_str), "{err}");
}

fn toml_parse_probe() -> String {
    // A malformed trusted-keys file mentioning nothing secret.
    load_trusted_keys("[[keys]]\n").expect_err("must fail")
}

fn temp_fixture(name: &str, contents: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("solid-sign-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    path
}

#[test]
fn include_enforcement_accepts_signed_and_rejects_the_rest() {
    use solid_core::{parse_file_with_trust, ParseOptions};
    let (secret, _, kid) = keypair();
    let trusted = trusted_of(&kid, &secret);
    // Signed library: verifies and merges.
    let lib = temp_fixture("signedlib.solid", "host = \"db\"\n");
    let lib_bytes = std::fs::read(&lib).expect("read lib");
    let mut sidecar = lib.as_os_str().to_owned();
    sidecar.push(".sig");
    std::fs::write(
        &sidecar,
        sign_bytes(&secret, &kid, &lib_bytes).expect("sign"),
    )
    .expect("write sig");
    let main = temp_fixture(
        "signedmain.solid",
        "include \"signedlib.solid\"\nport = 1\n",
    );
    let module =
        parse_file_with_trust(&main, ParseOptions::default(), &trusted).expect("signed parses");
    assert!(module.resolve_value_path("host").is_some());
    // Unsigned library: E032, fail-closed before parsing.
    let _lib2 = temp_fixture("unsignedlib.solid", "host = \"db\"\n");
    let main2 = temp_fixture(
        "unsignedmain.solid",
        "include \"unsignedlib.solid\"\nport = 1\n",
    );
    let diags = parse_file_with_trust(&main2, ParseOptions::default(), &trusted)
        .expect_err("unsigned must fail");
    assert!(diags.iter().any(|d| d.code == "E032"), "{diags:?}");
    assert!(
        diags.iter().all(|d| !format!("{d:?}").contains("host = ")),
        "{diags:?}"
    );
    // Legacy entry point is unaffected by design (no enforcement).
    let module = solid_core::parse_file(&main2, ParseOptions::default()).expect("legacy parses");
    assert!(module.resolve_value_path("host").is_some());
    // Tampered library: E032 even with a sidecar present.
    std::fs::write(&lib, "host = \"evil\"\n").expect("tamper lib");
    let diags = parse_file_with_trust(&main, ParseOptions::default(), &trusted)
        .expect_err("tampered must fail");
    assert!(diags.iter().any(|d| d.code == "E032"), "{diags:?}");
}
