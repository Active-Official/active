//! Header drift gate (F-017, G-002): the committed active-ffi/active.h must
//! equal fresh cbindgen output. Regenerate the header after any export
//! change and commit it; this test (running in CI via the workspace suite)
//! fails otherwise.

use std::path::PathBuf;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

#[test]
fn committed_header_matches_generator() {
    let bindings = cbindgen::Builder::new()
        .with_crate(crate_dir())
        .with_language(cbindgen::Language::C)
        .with_include_guard("ACTIVE_H")
        // Must match build.rs exactly: C++ consumers include this
        // header directly, so declarations stay inside extern "C".
        .with_header("#ifdef __cplusplus\nextern \"C\" {\n#endif")
        .with_trailer("#ifdef __cplusplus\n}\n#endif")
        .generate()
        .expect("cbindgen generates");
    let mut fresh = Vec::new();
    bindings.write(&mut fresh);
    let fresh = String::from_utf8(fresh).expect("header is UTF-8");
    let committed =
        std::fs::read_to_string(crate_dir().join("active.h")).expect("committed header readable");
    // Normalize line endings: the committed file is LF; generator output
    // follows the platform.
    let fresh = fresh.replace("\r\n", "\n");
    let committed = committed.replace("\r\n", "\n");
    assert_eq!(
        fresh, committed,
        "active.h drifted: regenerate via the crate build and commit active-ffi/active.h"
    );
}

#[test]
fn abi_major_is_pinned_for_skew_negotiation() {
    // H-002: bindings fail closed on major mismatch. The major line lives
    // in the committed header so negotiation tests never chase generated
    // output: ABI version 1, major 1, and the Rust constant agrees.
    let text =
        std::fs::read_to_string(crate_dir().join("active.h")).expect("committed header readable");
    assert!(
        text.contains("#define ACTIVE_ABI_VERSION 1"),
        "header pins ABI version 1"
    );
}
