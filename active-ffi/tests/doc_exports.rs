//! Documentation/ABI drift (LANG-G-014): every `active_*` name that
//! `docs/FFI.md` tells a C author to call must be declared by the
//! committed header. `active_buf_free` shipped in prose for months while
//! no such symbol existed; this test makes that class of drift fail.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Identifiers starting with `active_` in `text` (ASCII words only, so
/// crate names such as `active-core` are not mistaken for symbols).
fn active_names(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| w.starts_with("active_") && w.len() > "active_".len())
        .map(str::to_string)
        .collect()
}

#[test]
fn every_documented_active_name_is_declared_in_the_committed_header() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let doc = std::fs::read_to_string(dir.join("..").join("docs").join("FFI.md"))
        .expect("docs/FFI.md readable");
    let header = std::fs::read_to_string(dir.join("active.h")).expect("active.h readable");

    // A declaration is the name followed by `(` in a prototype; the doc
    // may also name stable constants, which the header defines too.
    let declared: BTreeSet<String> = active_names(&header);
    let documented = active_names(&doc);
    let missing: Vec<&String> = documented
        .iter()
        .filter(|name| !declared.contains(*name))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/FFI.md names symbols the committed header does not declare: {missing:?}"
    );
}
