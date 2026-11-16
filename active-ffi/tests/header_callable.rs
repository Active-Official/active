//! The committed header must be callable from C (LANG-G-001).
//!
//! `header_drift.rs` proves the committed `active.h` equals fresh cbindgen
//! output. It cannot catch a declaration that is generated consistently and
//! still uncallable from C: an opaque struct where a function-pointer
//! typedef belongs left `active_set_include_loader` unusable. This test
//! compiles a C file that CALLS every export through the committed header,
//! so a regression fails here instead of shipping.
//!
//! Skips honestly when no C compiler is installed; CI runs it on every
//! platform.

use std::path::PathBuf;
use std::process::Command;

fn compiler() -> Option<&'static str> {
    for candidate in ["cc", "gcc", "clang"] {
        let probe = Command::new(candidate).arg("--version").output();
        if probe.is_ok_and(|out| out.status.success()) {
            return Some(candidate);
        }
    }
    None
}

#[test]
fn every_export_is_callable_through_the_committed_header() {
    let Some(cc) = compiler() else {
        eprintln!("SKIP: no C compiler (cc/gcc/clang) on PATH");
        return;
    };
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = crate_dir.join("tests").join("call_every_export.c");
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let object = out_dir.join("call_every_export.o");
    let compile = Command::new(cc)
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Werror")
        .arg("-c")
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .expect("run the C compiler");
    assert!(
        compile.status.success(),
        "the committed header must be callable from C:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(object.is_file(), "the compiler produced an object file");
}
