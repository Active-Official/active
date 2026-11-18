//! Semantic value diff (L-P52).
//!
//! A path-oriented comparison of two evaluated trees: which values were
//! added, removed, or changed, with each side's type. The caller supplies
//! value echoes from a redacted copy, so this module never renders a
//! value and a secret cannot leak through it.

use crate::ir::Value;
use serde::{Deserialize, Serialize};

/// How a value at one path changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
}

/// One path-level difference between two evaluated values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueChange {
    pub path: String,
    pub change: ChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_type: Option<String>,
    /// Redacted scalar echo, filled by the caller from a redacted tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
}

/// The value-domain name used in a change record.
pub fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::String(_) => "string",
        Value::DurationMs(_) => "duration",
        Value::Quantity(_, _) => "quantity",
        Value::Array(_) => "array",
        Value::Table(_) => "table",
        Value::Secret(_) => "secret",
        Value::Blob { .. } => "blob",
    }
}

/// Semantic equality for a **redacted** pair: NaN equals NaN,
/// quantities compare value and unit, and blobs compare identity, size,
/// taint, and media type (never payload bytes).
///
/// Material is never compared: a secret is always unequal, and a tainted
/// blob is always unequal, so no sequence of diffs can confirm a guess.
/// The caller must pass redacted trees; a raw secret here would still
/// compare unequal, but a raw *plain* material copy would not.
pub fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y || (x.is_nan() && y.is_nan()),
        (Value::String(x), Value::String(y)) => x == y,
        (Value::DurationMs(x), Value::DurationMs(y)) => x == y,
        (Value::Quantity(x, ux), Value::Quantity(y, uy)) => {
            (x == y || (x.is_nan() && y.is_nan())) && ux == uy
        }
        (
            Value::Blob {
                hash: xh,
                size: xs,
                tainted: xt,
                media_type: xm,
                ..
            },
            Value::Blob {
                hash: yh,
                size: ys,
                tainted: yt,
                media_type: ym,
                ..
            },
        ) => !xt && !yt && xh == yh && xs == ys && xm == ym,
        (Value::Array(xs), Value::Array(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same_value(x, y))
        }
        (Value::Table(xs), Value::Table(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .all(|(key, x)| ys.get(key).is_some_and(|y| same_value(x, y)))
        }
        // Includes Secret/Secret: material equality is never reported.
        _ => false,
    }
}

/// Compare two RAW evaluated trees for equality and structure. The
/// result is value-free: paths, kinds, and type names only. A caller
/// that wants value echoes must project them from a redacted copy, so a
/// secret can never reach this module's output by accident.
pub fn diff_values(old: &Value, new: &Value) -> Vec<ValueChange> {
    let mut changes = Vec::new();
    walk("", Some(old), Some(new), &mut changes);
    changes.sort_by(|a, b| a.path.cmp(&b.path).then(a.change.cmp(&b.change)));
    changes
}

fn walk(path: &str, old: Option<&Value>, new: Option<&Value>, out: &mut Vec<ValueChange>) {
    match (old, new) {
        (None, None) => {}
        (None, Some(value)) => collect_subtree(path, value, ChangeKind::Added, out),
        (Some(value), None) => collect_subtree(path, value, ChangeKind::Removed, out),
        (Some(a), Some(b)) => {
            if same_value(a, b) {
                return;
            }
            match (a, b) {
                (Value::Table(left), Value::Table(right)) => {
                    for (key, item) in left {
                        let child = join(path, key);
                        match right.get(key) {
                            Some(other) => walk(&child, Some(item), Some(other), out),
                            None => walk(&child, Some(item), None, out),
                        }
                    }
                    for (key, item) in right {
                        if !left.contains_key(key) {
                            walk(&join(path, key), None, Some(item), out);
                        }
                    }
                }
                (Value::Array(left), Value::Array(right)) => {
                    let common = left.len().min(right.len());
                    for index in 0..common {
                        walk(
                            &format!("{path}[{index}]"),
                            Some(&left[index]),
                            Some(&right[index]),
                            out,
                        );
                    }
                    for (index, item) in left.iter().enumerate().skip(common) {
                        out.push(change(
                            &format!("{path}[{index}]"),
                            ChangeKind::Removed,
                            Some(item),
                            None,
                        ));
                    }
                    for (index, item) in right.iter().enumerate().skip(common) {
                        out.push(change(
                            &format!("{path}[{index}]"),
                            ChangeKind::Added,
                            None,
                            Some(item),
                        ));
                    }
                }
                _ => out.push(change(path, ChangeKind::Changed, Some(a), Some(b))),
            }
        }
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// An added or removed composite is reported at its leaf paths, so a
/// removed `db.password` is visible instead of one `db` table record.
fn collect_subtree(path: &str, value: &Value, kind: ChangeKind, out: &mut Vec<ValueChange>) {
    match value {
        Value::Table(map) if !map.is_empty() => {
            for (key, item) in map {
                collect_subtree(&join(path, key), item, kind, out);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (index, item) in items.iter().enumerate() {
                collect_subtree(&format!("{path}[{index}]"), item, kind, out);
            }
        }
        _ => {
            let (old, new) = match kind {
                ChangeKind::Added => (None, Some(value)),
                ChangeKind::Removed => (Some(value), None),
                ChangeKind::Changed => (Some(value), Some(value)),
            };
            out.push(change(path, kind, old, new));
        }
    }
}

fn change(path: &str, kind: ChangeKind, old: Option<&Value>, new: Option<&Value>) -> ValueChange {
    ValueChange {
        path: path.to_string(),
        change: kind,
        old_type: old.map(value_type_name).map(str::to_string),
        new_type: new.map(value_type_name).map(str::to_string),
        old: None,
        new: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    fn table(pairs: &[(&str, Value)]) -> Value {
        let mut map = IndexMap::new();
        for (key, value) in pairs {
            map.insert((*key).to_string(), value.clone());
        }
        Value::Table(map)
    }

    #[test]
    fn identical_trees_have_no_changes() {
        let a = table(&[("port", Value::Int(8080)), ("on", Value::Bool(true))]);
        assert!(diff_values(&a, &a).is_empty());
        assert!(same_value(&Value::Float(f64::NAN), &Value::Float(f64::NAN)));
    }

    #[test]
    fn added_removed_and_changed_values_keep_theirs_paths() {
        let old = table(&[
            ("port", Value::Int(8080)),
            ("legacy", Value::String("x".into())),
            ("nested", table(&[("a", Value::Int(1))])),
        ]);
        let new = table(&[
            ("port", Value::Int(9090)),
            (
                "nested",
                table(&[("a", Value::Int(1)), ("b", Value::Bool(false))]),
            ),
            ("feature", Value::Bool(true)),
        ]);
        let changes = diff_values(&old, &new);
        let seen: Vec<(&str, ChangeKind)> = changes
            .iter()
            .map(|change| (change.path.as_str(), change.change))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("feature", ChangeKind::Added),
                ("legacy", ChangeKind::Removed),
                ("nested.b", ChangeKind::Added),
                ("port", ChangeKind::Changed),
            ]
        );
        let port = changes.iter().find(|c| c.path == "port").expect("port");
        assert_eq!(port.old_type.as_deref(), Some("int"));
        assert_eq!(port.new_type.as_deref(), Some("int"));
        // The core is value-free; the CLI fills echoes from redaction.
        assert!(port.old.is_none() && port.new.is_none());
    }

    #[test]
    fn arrays_report_each_changed_index_and_the_length_delta() {
        let old = Value::Array(vec![Value::Int(1), Value::Int(2)]);
        let new = Value::Array(vec![Value::Int(1), Value::Int(3), Value::Int(4)]);
        let changes = diff_values(&old, &new);
        let seen: Vec<(&str, ChangeKind)> = changes
            .iter()
            .map(|change| (change.path.as_str(), change.change))
            .collect();
        assert_eq!(
            seen,
            vec![("[1]", ChangeKind::Changed), ("[2]", ChangeKind::Added)]
        );
    }

    #[test]
    fn a_changed_secret_is_reported_without_its_text() {
        let old = Value::Secret(zeroize::Zeroizing::new("old-value".into()));
        let new = Value::Secret(zeroize::Zeroizing::new("new-value".into()));
        let changes = diff_values(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change, ChangeKind::Changed);
        assert_eq!(changes[0].old_type.as_deref(), Some("secret"));
        assert!(!format!("{changes:?}").contains("old-value"));
    }

    #[test]
    fn material_is_never_equal_even_when_it_looks_identical() {
        // The redacted form of every secret is the same empty value; an
        // "identical" answer must still never come from comparing them.
        let empty = Value::Secret(zeroize::Zeroizing::new(String::new()));
        assert!(!same_value(&empty, &empty.clone()));
        let tainted = Value::Blob {
            hash: [7u8; 32],
            size: 3,
            tainted: true,
            media_type: "text/plain".into(),
            payload: None,
        };
        assert!(!same_value(&tainted, &tainted.clone()));
        assert_eq!(diff_values(&empty, &empty)[0].change, ChangeKind::Changed);
    }

    #[test]
    fn added_and_removed_composites_report_their_leaves() {
        let old = table(&[("keep", Value::Int(1))]);
        let new = table(&[
            ("keep", Value::Int(1)),
            (
                "db",
                table(&[
                    ("host", Value::String("h".into())),
                    (
                        "password",
                        Value::Secret(zeroize::Zeroizing::new(String::new())),
                    ),
                ]),
            ),
        ]);
        let added: Vec<String> = diff_values(&old, &new)
            .into_iter()
            .map(|change| change.path)
            .collect();
        assert_eq!(added, vec!["db.host", "db.password"]);
        let removed: Vec<String> = diff_values(&new, &old)
            .into_iter()
            .map(|change| change.path)
            .collect();
        assert_eq!(removed, vec!["db.host", "db.password"]);
    }
}
