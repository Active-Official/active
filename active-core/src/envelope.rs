//! Receiver-side interchange gate (spec/agent-interchange-profile.md §4,
//! LANG-C-004).
//!
//! The interchange profile's receiver rules - reject an unknown envelope
//! major, refuse capabilities the host does not offer, reject a missing or
//! unparseable envelope version - are the transport's negotiation layer.
//! They used to exist only on the `--sandbox` / `capability --check-doc`
//! paths, so a peer document evaluated through plain `eval` was acted on as
//! if this receiver implemented everything it claimed.
//!
//! [`EnvelopeGate`] makes the rules part of evaluation, default-on for every
//! embedding (`Engine::envelope_gate`). The gate runs before any evaluation
//! work, refuses with stable codes (`E033` for the version, `E034` for
//! capabilities), and fails closed in strict and resilient modes alike: a
//! peer cannot negotiate the receiver into evaluating semantics it does not
//! implement.

use crate::convert::capability::refused;
use crate::diag::{Diagnostic, Severity};
use crate::eval::EvalError;
use crate::ir::{Mode, Module, Node, Value};

/// Envelope major versions this library implements. The 1.x line
/// implements major 1; a receiver refuses any other major before
/// evaluation (spec/agent-interchange-profile.md §2 rule 3).
pub const SUPPORTED_ENVELOPE_MAJORS: &[u64] = &[1];

/// The receiver policy the gate enforces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeGate {
    /// Envelope majors this receiver implements. Empty refuses every
    /// envelope, which is what a receiver that implements none should say.
    pub majors: Vec<u64>,
    /// Tool names this receiver offers (the capability handshake set).
    /// Declared capabilities outside it are refused.
    pub host_tools: Vec<String>,
}

impl Default for EnvelopeGate {
    fn default() -> Self {
        Self::shipped()
    }
}

impl EnvelopeGate {
    /// The shipped receiver policy: major 1, every tool this library
    /// ships (`capability::known_tools()`, the union of the sandbox
    /// profiles and the capability descriptor).
    pub fn shipped() -> Self {
        Self {
            majors: SUPPORTED_ENVELOPE_MAJORS.to_vec(),
            host_tools: crate::convert::capability::known_tools()
                .into_iter()
                .map(str::to_string)
                .collect(),
        }
    }

    /// The shipped policy with a different offered-tool set: a host that
    /// implements a subset of the tools (an embedded runtime, a
    /// restricted sandbox) advertises exactly that subset.
    pub fn with_tools<I, S>(tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            majors: SUPPORTED_ENVELOPE_MAJORS.to_vec(),
            host_tools: tools.into_iter().map(Into::into).collect(),
        }
    }

    /// Enforce the receiver rules on `module`. `Ok(())` means the document
    /// may be evaluated; `Err` carries the refusal diagnostics, which the
    /// caller surfaces as a fail-closed evaluation error.
    ///
    /// Config Mode documents are not interchange envelopes (spec §2 rule
    /// 1), so they pass untouched. Data Mode documents are gated on the
    /// two claims a receiver must not take on faith: the version of the
    /// envelope they carry (when they carry one) and the capabilities
    /// they ask the receiver to offer.
    pub fn check(&self, module: &Module) -> Result<(), EvalError> {
        if module.mode != Mode::Data {
            return Ok(());
        }
        let mut diagnostics = Vec::new();

        // Capability handshake (spec §2 rule 2): anything declared but not
        // offered is refused, in declaration order, without duplicates.
        let host: Vec<&str> = self.host_tools.iter().map(String::as_str).collect();
        for name in refused(&module.capabilities, &host) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "E034".into(),
                message: format!(
                    "capability '{name}' is not offered by this receiver; offered: {}",
                    summarise(&self.host_tools)
                ),
                path: Some("active_capabilities".into()),
                span: None,
                repair: None,
                confidence: 1.0,
                file: None,
            });
        }

        // Envelope version (spec §2 rule 3, §3).
        if let Some(refs) = envelope_fields(module) {
            let span = refs
                .version
                .as_ref()
                .and_then(|node| module.node_spans.get(node).cloned())
                .or_else(|| module.node_spans.get(&refs.envelope).cloned());
            match version_major(module, refs.version) {
                VersionCheck::Missing => diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "E033".into(),
                    message: "envelope has no `version`: a receiver must know which envelope \
                              semantics it is being asked to honor"
                        .into(),
                    path: Some("envelope.version".into()),
                    span,
                    repair: None,
                    confidence: 1.0,
                    file: None,
                }),
                VersionCheck::Unparseable(text) => diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "E033".into(),
                    message: format!(
                        "envelope version `{text}` is not major.minor.patch with numeric parts"
                    ),
                    path: Some("envelope.version".into()),
                    span,
                    repair: None,
                    confidence: 1.0,
                    file: None,
                }),
                VersionCheck::Major(major) if !self.majors.contains(&major) => {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "E033".into(),
                        message: format!(
                            "envelope major {major} is not supported by this receiver; \
                             supported: {}",
                            summarise_majors(&self.majors)
                        ),
                        path: Some("envelope.version".into()),
                        span,
                        repair: None,
                        confidence: 1.0,
                        file: None,
                    })
                }
                VersionCheck::Major(_) => {}
            }
        }

        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(EvalError::ValidationFailed { diagnostics })
        }
    }
}

/// The root envelope table and its `version` node, when the document has
/// one. A Data Mode document without an envelope table is a data document,
/// not an interchange envelope: it makes no version claim to police.
struct EnvelopeRefs {
    envelope: crate::ir::NodeId,
    version: Option<crate::ir::NodeId>,
}

fn envelope_fields(module: &Module) -> Option<EnvelopeRefs> {
    let root = module.root?;
    let fields = match module.get(root) {
        Some(Node::Table { fields }) => fields,
        _ => return None,
    };
    let envelope = *fields.get("envelope")?;
    let table = deref_table(module, envelope)?;
    let version = match module.get(table) {
        Some(Node::Table { fields }) => fields.get("version").copied(),
        _ => None,
    };
    Some(EnvelopeRefs { envelope, version })
}

/// Major version of the envelope's `version` value. An alias resolves; a
/// version that is not a literal string cannot be negotiated and is
/// refused rather than guessed.
///
/// The walk is ITERATIVE and bounded: the gate runs before evaluation, so
/// the cycle check has not run yet, and a crafted `envelope.version` that
/// aliases its way around a loop would otherwise overflow the stack here
/// (review finding on LANG-C-004; `deref_table` was already bounded).
fn version_major(module: &Module, version_node: Option<crate::ir::NodeId>) -> VersionCheck {
    let Some(mut node) = version_node else {
        return VersionCheck::Missing;
    };
    for _ in 0..64 {
        match module.get(node) {
            Some(Node::Literal(Value::String(text))) => {
                return match parse_major(text) {
                    Some(major) => VersionCheck::Major(major),
                    None => VersionCheck::Unparseable(text.clone()),
                }
            }
            Some(Node::Literal(value)) => return VersionCheck::Unparseable(format!("{value:?}")),
            Some(Node::Ref(target)) => node = *target,
            _ => return VersionCheck::Missing,
        }
    }
    // A ref chain longer than the bound is malformed (the parser's graph is
    // acyclic by construction; a loop means recovery built one). Refuse
    // rather than guess.
    VersionCheck::Missing
}

enum VersionCheck {
    Missing,
    Unparseable(String),
    Major(u64),
}

/// Follow `Ref` links to a table literal (bounded: a ref chain cannot loop
/// forever, but a malformed graph must not spin).
fn deref_table(module: &Module, mut id: crate::ir::NodeId) -> Option<crate::ir::NodeId> {
    for _ in 0..64 {
        match module.get(id)? {
            Node::Table { .. } => return Some(id),
            Node::Ref(next) => id = *next,
            _ => return None,
        }
    }
    None
}

/// Major component of a `major.minor.patch` version string, or `None` when
/// the text is not exactly three numeric parts holding a `u64` major.
/// Digits and dots only: a leading sign, whitespace, or build metadata is
/// not part of the envelope's schema version, so it is refused rather than
/// interpreted.
fn parse_major(text: &str) -> Option<u64> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let mut parts = text.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    for _ in 0..2 {
        parts.next()?.parse::<u64>().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(major)
}

/// Bounded list rendering for refusal messages (hostile documents cannot
/// inflate a diagnostic through an enormous declared capability list).
fn summarise(items: &[String]) -> String {
    const SHOWN: usize = 12;
    let mut text = items
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > SHOWN {
        text.push_str(&format!(" (+{} more)", items.len() - SHOWN));
    }
    text
}

fn summarise_majors(majors: &[u64]) -> String {
    if majors.is_empty() {
        return "none (this receiver implements no envelope major)".into();
    }
    majors
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_accepts_exactly_three_numeric_parts() {
        assert_eq!(parse_major("1.0.0"), Some(1));
        assert_eq!(parse_major("12.34.56"), Some(12));
        for bad in [
            "", "1", "1.0", "1.0.0.0", "one.0.0", "1.x.0", " 1.0.0", "1.0.0 ", "-1.0.0", "1..0",
            "1.0.-1", "+1.0.0",
        ] {
            assert_eq!(parse_major(bad), None, "{bad:?} must not parse");
        }
    }
}
