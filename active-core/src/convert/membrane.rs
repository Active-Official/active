//! The redaction membrane (L-P36): a sealed wrapper that proves an
//! evaluated tree has been profile-redacted before it reaches a
//! serializer or a materialised payload exporter.
//!
//! `Value` deliberately does not implement `Serialize`: a generic
//! serializer is profile-unaware, so it cannot sweep a profile-selected
//! plain value or a copy of a runtime secret, and a raw tree can still
//! carry a retained blob payload. The only generic serialization of an
//! evaluated tree is through [`RedactedValue`], whose constructors are
//! the profile-aware redactor and the sealed projection. A host that
//! forgets to redact cannot serialize, and cannot reach a materialised
//! payload exporter, without the compiler refusing.

use crate::ir::Value;
use serde_json::Value as JsonValue;

/// A profile-redacted evaluated tree.
///
/// The field is private and there is no `From<Value>`, so the only ways
/// to build one are [`redact_value`](super::redact_value) and
/// [`RedactedValue::project`]. The profile that swept the tree is
/// therefore necessarily the one that produced the wrapper.
#[derive(Clone, Debug)]
pub struct RedactedValue(Value);

impl RedactedValue {
    /// Wrap an already-redacted tree. `pub(in crate::convert)`: only
    /// `convert::redact_value` calls this; `RedactedValue::project`
    /// builds the wrapper directly in this module. The invariant cannot
    /// be forged anywhere outside `convert`.
    pub(in crate::convert) fn new(value: Value) -> Self {
        Self(value)
    }

    /// Read-only view of the redacted tree. There is no `Deref`: auto-deref
    /// would let `serialize(&*sealed)` reach a `&Value` again, and an
    /// explicit call site is the point of the boundary.
    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// Consume the wrapper and return the redacted tree. The tree is
    /// already redacted, so unwrapping cannot un-redact it; this is the
    /// explicit door out of the typed boundary for callers that need the
    /// tree itself (compare, hash, inspect) rather than a serializer.
    pub fn into_value(self) -> Value {
        self.0
    }

    /// Select a cone from the redacted tree without re-redacting at a new
    /// root. Redaction is document-relative (a profile path is matched
    /// against the authored path), so the projection is taken on the
    /// already-redacted tree; the CLI's `--path` handling relies on this.
    pub fn project(&self, path: &str) -> Result<RedactedValue, String> {
        super::project::project(&self.0, path).map(|value| RedactedValue(value.clone()))
    }
}

/// The one generic serialization of an evaluated tree, backed by the
/// canonical ([`value_to_json`](super::value_to_json)) mapping, so a
/// redacted tree has exactly one wire form. The canonical mapping is
/// identity-only: blob payload bytes never cross, even here.
impl serde::Serialize for RedactedValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let json: JsonValue = super::value_to_json(&self.0);
        serde::Serialize::serialize(&json, serializer)
    }
}

/// Compile-fail pins for the membrane (L-P36-DESIGN.md section 7.1/7.2).
///
/// These are `compile_fail` doctests rather than `trybuild`: the repo
/// ships no `rust-toolchain.toml` and CI runs `dtolnay/rust-toolchain@stable`,
/// so `trybuild`'s pinned stderr would break on a stable bump. A
/// `compile_fail` doctest still fails `cargo test` the moment a forbidden
/// call compiles again (the negative control of section 7.8), with no new
/// dependency and no toolchain coupling.
///
/// The generic, profile-unaware door is closed: `Value` has no
/// `Serialize`. Re-adding `impl Serialize for Value` makes this compile
/// and the doctest fail.
///
/// ```compile_fail
/// let value = active_core::Value::Null;
/// let _ = serde_json::to_string(&value);
/// ```
///
/// The materialised payload exporter takes only the sealed value, so a
/// raw evaluated tree cannot reach `payload_b64`.
///
/// ```compile_fail
/// let value = active_core::Value::Null;
/// let _ = active_core::convert::value_to_json_materialised(&value);
/// ```
///
/// Positive control: the sanctioned door does compile and serialize. If
/// `serde_json` were unavailable to doctests this would fail loudly, so
/// the two `compile_fail` cases above cannot pass for the wrong reason.
///
/// ```
/// # use active_core::{parse, Engine};
/// # let module = parse("port = 8080\n", "doc").expect("parse");
/// # let engine = Engine::hermetic();
/// # let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("eval");
/// let profile = active_core::secret::SecretProfile::for_engine(&module, &engine, &seeds);
/// let sealed = active_core::convert::redact_value(&profile, &value);
/// let json = serde_json::to_string(&sealed).expect("serialize");
/// # assert!(json.contains("8080"));
/// // Pin the exact symbols the two `compile_fail` cases name, so a
/// // rename or a move turns THIS doctest red instead of letting those
/// // cases pass for the wrong reason (an unresolved path). The fn
/// // pointers also fix the seal on all four materialised exporters.
/// let raw: active_core::Value = active_core::Value::Null;
/// let _ = active_core::convert::redact_value(&profile, &raw);
/// let _: fn(&active_core::convert::RedactedValue) -> serde_json::Value =
///     active_core::convert::value_to_json_materialised;
/// let _: fn(&active_core::convert::RedactedValue) -> String =
///     active_core::convert::value_to_pretty_json_materialised;
/// let _: fn(&active_core::convert::RedactedValue) -> Result<String, String> =
///     active_core::convert::value_to_yaml_string_materialised;
/// let _: fn(
///     &active_core::convert::RedactedValue,
/// ) -> Result<(String, Vec<active_core::Diagnostic>), String> =
///     active_core::convert::value_to_toml_string_materialised;
/// ```
#[cfg(doctest)]
pub struct MembraneDoctestPins;
