//! Generate active.h via cbindgen (G-002). The committed header at
//! active-ffi/active.h is the reviewed artifact; CI regenerates to a temp
//! dir and fails on drift.

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    let bindings = cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_language(cbindgen::Language::C)
        .with_include_guard("ACTIVE_H")
        // C++ consumers include this header directly: wrap the
        // declarations so C++ name mangling cannot break linking.
        // Keep in sync with tests/header_drift.rs (same settings).
        .with_header("#ifdef __cplusplus\nextern \"C\" {\n#endif")
        .with_trailer("#ifdef __cplusplus\n}\n#endif")
        .generate()
        .expect("cbindgen generates active.h");
    let out = PathBuf::from(env::var("OUT_DIR").expect("out dir")).join("active.h");
    bindings.write_to_file(out);
    println!("cargo:rerun-if-changed=src/lib.rs");
}
