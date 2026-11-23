//! L-P14: the evaluated-value handle is an immutable view at the C ABI.
//!
//! The header is the contract a C host compiles against. This test pins
//! the shape that makes mutation impossible: the handle is opaque, every
//! reader takes a `const` pointer, and the only mutable-pointer export is
//! ownership release. A new export touching `ActiveValue` must be reviewed
//! here (the allowlist).

use std::path::PathBuf;

fn header() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("active.h");
    std::fs::read_to_string(&path).expect("committed header readable")
}

/// Every declaration that names `ActiveValue`, exactly as the header
/// renders it. `active_eval` returns an owning handle (the pointer is
/// mutable because ownership transfers); no reader accepts a mutable one.
const ALLOWED: &[&str] = &[
    "struct ActiveValue *active_eval(",
    "struct ActiveValue *active_eval_sandboxed(",
    "struct ActiveValue *active_eval_sandboxed_len(",
    "void active_value_free(struct ActiveValue *value);",
    "char *active_value_to_json(const struct ActiveValue *value);",
    "bool active_value_is_secret(const struct ActiveValue *value);",
    "bool active_value_is_tainted(const struct ActiveValue *value);",
    "char *active_value_project(const struct ActiveValue *value,",
    "uint32_t active_value_type(const struct ActiveValue *value);",
];

#[test]
fn the_value_handle_is_opaque_and_read_only() {
    let text = header();
    assert!(
        text.contains("typedef struct ActiveValue ActiveValue;"),
        "the handle must be an opaque typedef"
    );
    assert!(
        !text.contains("struct ActiveValue {"),
        "the header must not expose the handle's fields"
    );

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
            continue;
        }
        if !line.contains("ActiveValue") {
            continue;
        }
        if trimmed.starts_with("typedef struct ActiveValue ActiveValue;") {
            continue;
        }
        assert!(
            ALLOWED.iter().any(|decl| trimmed.starts_with(decl)),
            "an unreviewed ActiveValue surface appeared: {line}"
        );
    }

    // The only export that takes a mutable handle only releases ownership;
    // no export mutates a value.
    for forbidden in [
        "active_value_set",
        "active_value_write",
        "active_value_mut",
        "active_value_assign",
        "active_value_clear",
    ] {
        assert!(
            !text.contains(forbidden),
            "a mutating value export is forbidden: {forbidden}"
        );
    }
}

#[test]
fn every_reader_takes_a_const_value() {
    let text = header();
    for name in [
        "active_value_to_json",
        "active_value_is_secret",
        "active_value_is_tainted",
        "active_value_project",
        "active_value_type",
    ] {
        let line = text
            .lines()
            .find(|line| line.contains(name) && line.contains('('))
            .unwrap_or_else(|| panic!("{name} declaration"));
        assert!(
            line.contains("const struct ActiveValue *"),
            "{name} must take a const handle: {line}"
        );
    }
}
