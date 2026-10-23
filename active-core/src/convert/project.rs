//! Path-demanded output cones (PROPOSAL §2.2, CHECKLIST PE.001).
//!
//! `project` selects a sub-value from an evaluated tree using the same
//! dotted/index path language as `@patch` (`a.b[0].c`). The budget
//! accountant always guards the whole graph first (projection runs
//! after a full evaluation); the cone selects what gets emitted, so
//! agents fetch kilobytes instead of megabytes while safety stays
//! whole-document.

use crate::ir::Value;
use crate::parser::{validate_patch_path, PathSeg};

/// Select the demanded cone of an evaluated value. Missing paths and
/// malformed path syntax are errors (never silent nulls).
pub fn project<'v>(value: &'v Value, path: &str) -> Result<&'v Value, String> {
    let segs =
        validate_patch_path(path).map_err(|why| format!("invalid demand path `{path}`: {why}"))?;
    let mut cursor = value;
    for seg in &segs {
        cursor = match (seg, cursor) {
            (PathSeg::Key(key), Value::Table(map)) => map
                .get(key)
                .ok_or_else(|| format!("unknown path `{path}`: no such key"))?,
            (PathSeg::Index(i), Value::Array(elements)) => elements
                .get(*i)
                .ok_or_else(|| format!("unknown path `{path}`: index out of bounds"))?,
            _ => return Err(format!("unknown path `{path}`: shape mismatch")),
        };
    }
    Ok(cursor)
}
