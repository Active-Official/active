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
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

/// Envelope major versions this library implements. The 1.x line
/// implements major 1; v2 adds receipts, sender identity, and the bound
/// result hash (L-T06). A receiver refuses any other major before
/// evaluation (spec/agent-interchange-profile.md §2 rule 3).
pub const SUPPORTED_ENVELOPE_MAJORS: &[u64] = &[1, 2];

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
        for _ in refused(&module.capabilities, &host) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "E034".into(),
                // Fixed wording: the declared name is document data and
                // may be secret-derived; the path and span locate it.
                message: format!(
                    "a declared capability is not offered by this receiver; offered: {}",
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
        match envelope_fields(module) {
            EnvelopeLookup::Absent => {}
            EnvelopeLookup::Unresolvable(node) => diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "E033".into(),
                message: "envelope must be a table (or a bounded alias of one)".into(),
                path: Some("envelope".into()),
                span: module.node_spans.get(&node).cloned(),
                repair: None,
                confidence: 1.0,
                file: None,
            }),
            EnvelopeLookup::Found(refs) => {
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
                    VersionCheck::Unparseable => diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "E033".into(),
                        // Fixed wording: the received text is document data
                        // and may be secret-derived (review C3).
                        message: "envelope version is not major.minor.patch with numeric parts"
                            .into(),
                        path: Some("envelope.version".into()),
                        span,
                        repair: None,
                        confidence: 1.0,
                        file: None,
                    }),
                    VersionCheck::Major(major) if !self.majors.contains(&major) => diagnostics
                        .push(Diagnostic {
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
                        }),
                    VersionCheck::Major(major) => {
                        if major == 2 {
                            self.check_v2(module, &refs, &mut diagnostics);
                        } else if major == 1 && v1_has_v2_fields(module, &refs) {
                            diagnostics.push(Diagnostic {
                                severity: Severity::Error,
                                code: "E033".into(),
                                message: "a v1 envelope cannot carry v2 receipt/sender fields"
                                    .into(),
                                path: Some("envelope".into()),
                                span,
                                repair: None,
                                confidence: 1.0,
                                file: None,
                            });
                        }
                    }
                }
            }
        }

        // Self-containment (LANG-F-007): an interchange document that reads
        // the receiver's environment, resolves includes against the
        // receiver's filesystem, or depends on a profile the receiver picks
        // is not the document the sender evaluated. Refuse it instead of
        // silently rebinding the sender's meaning.
        for (node, message) in self_containment_violations(module) {
            let span = node.and_then(|n| module.node_spans.get(&n).cloned());
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "E036".into(),
                message,
                path: None,
                span,
                repair: None,
                confidence: 1.0,
                file: None,
            });
        }

        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(EvalError::ValidationFailed { diagnostics })
        }
    }

    /// v2 shape rules (L-T06): a receipt is mandatory, its outcome and
    /// failure class are closed, clean results are required and equal,
    /// metadata is closed, and an optional sender block is structurally
    /// valid. Present-but-unresolvable fields are malformed, never
    /// silently absent. Signature verification is a receiver policy step
    /// ([`verify_sender`]); the bound result is verified post-evaluation
    /// by [`check_evaluated`].
    fn check_v2(&self, module: &Module, refs: &EnvelopeRefs, diagnostics: &mut Vec<Diagnostic>) {
        let Some(table) = deref_table(module, refs.envelope) else {
            return;
        };
        let Some(Node::Table { fields }) = module.get(table) else {
            return;
        };
        let mut push = |code: &'static str, path: &'static str, message: &'static str| {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: code.into(),
                message: message.into(),
                path: Some(path.into()),
                span: None,
                repair: None,
                confidence: 1.0,
                file: None,
            });
        };

        let receipt_fields = match field_table(module, fields, "receipt") {
            FieldTable::Absent => {
                push(
                    "E042",
                    "envelope.receipt",
                    "envelope v2 requires a `receipt` table; an adversarial outcome must ship its audit record",
                );
                return;
            }
            FieldTable::Malformed => {
                push(
                    "E042",
                    "envelope.receipt",
                    "envelope.receipt must be a table",
                );
                return;
            }
            FieldTable::Table(table) => match module.get(table) {
                Some(Node::Table { fields }) => fields,
                _ => {
                    push(
                        "E042",
                        "envelope.receipt",
                        "envelope.receipt must be a table",
                    );
                    return;
                }
            },
        };

        let outcome = match field_text(module, receipt_fields, "outcome") {
            FieldText::Text(text) => text,
            _ => {
                push(
                    "E042",
                    "envelope.receipt.outcome",
                    "envelope.receipt.outcome must be \"clean\" or \"failed\"",
                );
                return;
            }
        };
        if !matches!(outcome.as_str(), "clean" | "failed") {
            push(
                "E042",
                "envelope.receipt.outcome",
                "envelope.receipt.outcome must be \"clean\" or \"failed\"",
            );
            return;
        }
        match field_text(module, receipt_fields, "failure_class") {
            FieldText::Absent if outcome == "failed" => push(
                "E042",
                "envelope.receipt.failure_class",
                "a failed receipt must name its failure_class (parse, budget, validation, other)",
            ),
            FieldText::Text(class)
                if !matches!(class.as_str(), "parse" | "budget" | "validation" | "other") =>
            {
                push(
                    "E042",
                    "envelope.receipt.failure_class",
                    "envelope.receipt.failure_class must be one of \"parse\", \"budget\", \"validation\", \"other\"",
                )
            }
            FieldText::Malformed => push(
                "E042",
                "envelope.receipt.failure_class",
                "envelope.receipt.failure_class must be a literal string",
            ),
            _ => {}
        }
        let receipt_result = match field_text(module, receipt_fields, "result") {
            FieldText::Absent => None,
            FieldText::Text(text) if is_sha256(&text) => Some(text),
            _ => {
                push(
                    "E042",
                    "envelope.receipt.result",
                    "envelope.receipt.result must be `sha256:<64 hex>`",
                );
                None
            }
        };
        match field_text(module, receipt_fields, "mode") {
            FieldText::Absent => {}
            FieldText::Text(mode) if matches!(mode.as_str(), "config" | "data") => {}
            _ => push(
                "E042",
                "envelope.receipt.mode",
                "envelope.receipt.mode must be \"config\" or \"data\"",
            ),
        }
        for key in ["profile", "schema_version"] {
            if matches!(
                field_text(module, receipt_fields, key),
                FieldText::Malformed
            ) {
                push(
                    "E042",
                    "envelope.receipt",
                    "envelope.receipt metadata fields must be literal strings",
                );
                break;
            }
        }
        if matches!(
            field_table(module, receipt_fields, "budget"),
            FieldTable::Malformed
        ) {
            push(
                "E042",
                "envelope.receipt.budget",
                "envelope.receipt.budget must be a table",
            );
        }
        // L-P11: the optional closed-table audit is a sender claim (bound
        // by receipt_digest, not recomputed by the receiver), but a
        // malformed receipt is still a malformed receipt.
        if receipt_fields.contains_key("closed_tables")
            && !closed_tables_shape_ir(module, receipt_fields)
        {
            push(
                "E042",
                "envelope.receipt.closed_tables",
                "envelope.receipt.closed_tables must be an array of `{ path, keys }` tables with string names",
            );
        }

        let sender_fields = match field_table(module, fields, "sender") {
            FieldTable::Absent => None,
            FieldTable::Malformed => {
                push("E043", "envelope.sender", "envelope.sender must be a table");
                return;
            }
            FieldTable::Table(table) => match module.get(table) {
                Some(Node::Table { fields }) => Some(fields),
                _ => {
                    push("E043", "envelope.sender", "envelope.sender must be a table");
                    return;
                }
            },
        };
        if let Some(sender_fields) = sender_fields {
            match field_text(module, sender_fields, "id") {
                FieldText::Text(id)
                    if !id.trim().is_empty() && !id.chars().any(char::is_control) => {}
                _ => push(
                    "E043",
                    "envelope.sender.id",
                    "envelope.sender.id must be a non-empty, control-free string",
                ),
            }
            let key = match field_text(module, sender_fields, "key") {
                FieldText::Absent => None,
                FieldText::Text(key) if crate::sign::valid_kid(&key) => Some(key),
                _ => {
                    push(
                        "E043",
                        "envelope.sender.key",
                        "envelope.sender.key must be a valid key id",
                    );
                    None
                }
            };
            let signature = match field_text(module, sender_fields, "signature") {
                FieldText::Absent => None,
                FieldText::Text(signature)
                    if signature.len() == 128
                        && signature.bytes().all(|b| b.is_ascii_hexdigit()) =>
                {
                    Some(signature)
                }
                _ => {
                    push(
                        "E043",
                        "envelope.sender.signature",
                        "envelope.sender.signature must be 128 hex characters",
                    );
                    None
                }
            };
            if key.is_some() != signature.is_some() {
                push(
                    "E043",
                    "envelope.sender",
                    "envelope.sender.key and envelope.sender.signature must be given together",
                );
            }
        }

        let result = match field_text(module, fields, "result") {
            FieldText::Absent => None,
            FieldText::Text(text) if is_sha256(&text) => Some(text),
            _ => {
                push(
                    "E043",
                    "envelope.result",
                    "envelope.result must be `sha256:<64 hex>`",
                );
                None
            }
        };
        if outcome == "clean" {
            if result.is_none() || receipt_result.is_none() {
                push(
                    "E042",
                    "envelope.result",
                    "a clean v2 envelope must carry `result` and `receipt.result`",
                );
            } else if result != receipt_result {
                push(
                    "E042",
                    "envelope.result",
                    "envelope.result and envelope.receipt.result must be equal",
                );
            }
        } else if let (Some(left), Some(right)) = (&result, &receipt_result) {
            if left != right {
                push(
                    "E042",
                    "envelope.result",
                    "envelope.result and envelope.receipt.result must be equal",
                );
            }
        }
        match field_text(module, fields, "capability") {
            FieldText::Absent => {}
            FieldText::Text(capability) if is_closed_capability(&capability) => {}
            _ => push(
                "E043",
                "envelope.capability",
                "envelope.capability must be strict, data, agent, or trusted",
            ),
        }
    }
}

enum FieldText {
    Absent,
    Text(String),
    Malformed,
}

fn field_text(
    module: &Module,
    fields: &indexmap::IndexMap<String, crate::ir::NodeId>,
    key: &str,
) -> FieldText {
    match fields.get(key) {
        None => FieldText::Absent,
        Some(node) => match literal_string(module, *node) {
            Some(text) => FieldText::Text(text),
            None => FieldText::Malformed,
        },
    }
}

enum FieldTable {
    Absent,
    Table(crate::ir::NodeId),
    Malformed,
}

fn field_table(
    module: &Module,
    fields: &indexmap::IndexMap<String, crate::ir::NodeId>,
    key: &str,
) -> FieldTable {
    match fields.get(key) {
        None => FieldTable::Absent,
        Some(node) => match deref_table(module, *node) {
            Some(table) => FieldTable::Table(table),
            None => FieldTable::Malformed,
        },
    }
}

/// The source-AST shape of an optional `receipt.closed_tables` member
/// (L-P11): an array of tables with a string `path` and an array of string
/// `keys`. `true` when absent, or present and well shaped.
fn closed_tables_shape_ir(
    module: &Module,
    receipt_fields: &indexmap::IndexMap<String, crate::ir::NodeId>,
) -> bool {
    let Some(node) = receipt_fields.get("closed_tables") else {
        return true;
    };
    let Some(Node::Array { elements }) = module.get(*node) else {
        return false;
    };
    elements.iter().all(|element| match module.get(*element) {
        Some(Node::Table { fields }) => {
            // Exactly `{ path, keys }`: an extra member would let a sender
            // persist arbitrary text in a receipt a receiver may log.
            fields.len() == 2
                && fields.contains_key("path")
                && fields.contains_key("keys")
                && matches!(field_text(module, fields, "path"), FieldText::Text(_))
                && match fields.get("keys").and_then(|id| module.get(*id)) {
                    Some(Node::Array { elements }) => elements.iter().all(|key| {
                        matches!(module.get(*key), Some(Node::Literal(Value::String(_))))
                    }),
                    _ => false,
                }
        }
        _ => false,
    })
}

/// Whether a `closed_tables` member echoes text the receiver knows is
/// material (L-P11). A sender-side secret the receiver never saw cannot
/// be detected; this is the receiver's available defense in depth.
fn closed_tables_echo_material(
    receipt: &indexmap::IndexMap<String, Value>,
    profile: &crate::secret::SecretProfile,
) -> bool {
    let Some(Value::Array(items)) = receipt.get("closed_tables") else {
        return false;
    };
    fn material(text: &str, profile: &crate::secret::SecretProfile) -> bool {
        profile.is_material_text(text) || profile.redact_text(text).is_some()
    }
    items.iter().any(|item| {
        let Value::Table(map) = item else {
            return false;
        };
        let path_material = match map.get("path") {
            Some(Value::String(text)) => material(text, profile),
            _ => false,
        };
        let keys_material = match map.get("keys") {
            Some(Value::Array(keys)) => keys.iter().any(|key| match key {
                Value::String(text) => material(text, profile),
                _ => false,
            }),
            _ => false,
        };
        path_material || keys_material
    })
}

/// The evaluated shape of an optional `receipt.closed_tables` member
/// (L-P11). See [`closed_tables_shape_ir`].
fn closed_tables_shape_value(receipt: &indexmap::IndexMap<String, Value>) -> bool {
    let Some(value) = receipt.get("closed_tables") else {
        return true;
    };
    let Value::Array(items) = value else {
        return false;
    };
    items.iter().all(|item| match item {
        Value::Table(map) => {
            // Exactly `{ path, keys }` (see the IR twin).
            map.len() == 2
                && map.contains_key("path")
                && map.contains_key("keys")
                && matches!(map.get("path"), Some(Value::String(_)))
                && matches!(
                    map.get("keys"),
                    Some(Value::Array(keys))
                        if keys.iter().all(|key| matches!(key, Value::String(_)))
                )
        }
        _ => false,
    })
}

fn is_closed_capability(text: &str) -> bool {
    matches!(text, "strict" | "data" | "agent" | "trusted")
}

/// Post-evaluation v2 verification (review C1/C2/H1/H5). The pre-gate
/// checks the source AST; profiles and patches can rewrite or inject the
/// `envelope` afterwards, so the evaluated tree is checked again:
///
/// - the envelope is a table with a supported version;
/// - capabilities are offered and the document is self-contained;
/// - v2 receipts/results/senders obey the shape rules;
/// - a clean v2 `result` and `receipt.result` both equal the bound hash of
///   the (redacted) payload + schema version + profile + resolved budget +
///   mode + capability.
///
/// Any failure is fatal in strict and resilient modes: a receiver must
/// not act on an envelope it cannot verify.
pub fn check_evaluated(
    value: &Value,
    module: &Module,
    gate: &EnvelopeGate,
    budget: &crate::eval::Budget,
    profile: Option<&str>,
    material: Option<&crate::secret::SecretProfile>,
    trust: Option<&crate::sign::TrustedKeys>,
) -> Result<(), EvalError> {
    if module.mode != Mode::Data {
        return Ok(());
    }
    let Value::Table(root) = value else {
        return Ok(());
    };
    let Some(envelope) = root.get("envelope") else {
        return Ok(());
    };
    let Value::Table(fields) = envelope else {
        return Err(validation_error(
            "E033",
            "envelope",
            "envelope must be a table",
        ));
    };
    let version = match fields.get("version") {
        Some(Value::String(text)) => text.clone(),
        _ => {
            return Err(validation_error(
                "E033",
                "envelope.version",
                "envelope has no literal version",
            ))
        }
    };
    let Some(major) = parse_major(&version) else {
        return Err(validation_error(
            "E033",
            "envelope.version",
            "envelope version is not major.minor.patch with numeric parts",
        ));
    };
    if !gate.majors.contains(&major) {
        return Err(validation_error(
            "E033",
            "envelope.version",
            "envelope major is not supported by this receiver",
        ));
    }
    let host: Vec<&str> = gate.host_tools.iter().map(String::as_str).collect();
    let declared: Vec<String> = root
        .get("active_capabilities")
        .and_then(|value| match value {
            Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(|item| match item {
                        Value::String(name) => Some(name.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    if !refused(&declared, &host).is_empty() {
        return Err(validation_error(
            "E034",
            "active_capabilities",
            "a declared capability is not offered by this receiver",
        ));
    }
    if !self_containment_violations_any(module).is_empty() {
        return Err(validation_error(
            "E036",
            "envelope",
            "interchange document is not self-contained",
        ));
    }
    // The evaluated taint claim must match the evaluated payload; a patch
    // can flip or remove the source claim (review H3a).
    let claim = match fields.get("tainted") {
        Some(Value::Bool(claim)) => *claim,
        _ => {
            return Err(validation_error(
                "E035",
                "envelope.tainted",
                "envelope.tainted must be a boolean claim",
            ))
        }
    };
    let payload = fields.get("payload").cloned().unwrap_or(Value::Null);
    let actual = payload_tainted(&payload)
        || material
            .map(|profile| {
                crate::convert::value_to_json_with(profile, &payload)
                    .to_string()
                    .contains(crate::secret::MARKER)
            })
            .unwrap_or(false);
    if claim != actual {
        return Err(validation_error(
            "E035",
            "envelope.tainted",
            "the evaluated tainted claim does not match the evaluated payload",
        ));
    }
    if major != 2 {
        if major == 1 && (fields.contains_key("receipt") || fields.contains_key("sender")) {
            return Err(validation_error(
                "E033",
                "envelope",
                "a v1 envelope cannot carry v2 receipt/sender fields",
            ));
        }
        return Ok(());
    }
    check_v2_value(fields, module, budget, profile, material, trust)
}

fn validation_error(code: &'static str, path: &'static str, message: &'static str) -> EvalError {
    EvalError::ValidationFailed {
        diagnostics: vec![Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            path: Some(path.into()),
            span: None,
            repair: None,
            confidence: 1.0,
            file: None,
        }],
    }
}

fn value_text(fields: &indexmap::IndexMap<String, Value>, key: &str) -> FieldText {
    match fields.get(key) {
        None => FieldText::Absent,
        Some(Value::String(text)) => FieldText::Text(text.clone()),
        Some(_) => FieldText::Malformed,
    }
}

fn value_table<'a>(
    fields: &'a indexmap::IndexMap<String, Value>,
    key: &str,
) -> Result<Option<&'a indexmap::IndexMap<String, Value>>, ()> {
    match fields.get(key) {
        None => Ok(None),
        Some(Value::Table(table)) => Ok(Some(table)),
        Some(_) => Err(()),
    }
}

fn check_v2_value(
    fields: &indexmap::IndexMap<String, Value>,
    module: &Module,
    budget: &crate::eval::Budget,
    profile: Option<&str>,
    material: Option<&crate::secret::SecretProfile>,
    trust: Option<&crate::sign::TrustedKeys>,
) -> Result<(), EvalError> {
    let receipt = match value_table(fields, "receipt") {
        Ok(Some(table)) => table,
        Ok(None) => {
            return Err(validation_error(
                "E042",
                "envelope.receipt",
                "envelope v2 requires a `receipt` table",
            ))
        }
        Err(()) => {
            return Err(validation_error(
                "E042",
                "envelope.receipt",
                "envelope.receipt must be a table",
            ))
        }
    };
    let outcome = match value_text(receipt, "outcome") {
        FieldText::Text(text) if matches!(text.as_str(), "clean" | "failed") => text,
        _ => {
            return Err(validation_error(
                "E042",
                "envelope.receipt.outcome",
                "envelope.receipt.outcome must be \"clean\" or \"failed\"",
            ))
        }
    };
    if outcome == "failed" {
        if !matches!(fields.get("payload"), None | Some(Value::Null)) {
            return Err(validation_error(
                "E042",
                "envelope.payload",
                "a failed v2 envelope must not carry a payload",
            ));
        }
        match value_text(receipt, "failure_class") {
            FieldText::Text(class)
                if matches!(class.as_str(), "parse" | "budget" | "validation" | "other") => {}
            _ => {
                return Err(validation_error(
                    "E042",
                    "envelope.receipt.failure_class",
                    "a failed receipt must name its failure_class",
                ))
            }
        }
    }
    if let FieldText::Text(mode) = value_text(receipt, "mode") {
        if !matches!(mode.as_str(), "config" | "data") {
            return Err(validation_error(
                "E042",
                "envelope.receipt.mode",
                "envelope.receipt.mode must be \"config\" or \"data\"",
            ));
        }
    }
    if let Ok(Some(budget_table)) = value_table(receipt, "budget") {
        let _ = budget_table;
    } else if matches!(receipt.get("budget"), Some(value) if !matches!(value, Value::Table(_))) {
        return Err(validation_error(
            "E042",
            "envelope.receipt.budget",
            "envelope.receipt.budget must be a table",
        ));
    }
    // L-P11: the closed-table audit is a sender claim bound by the
    // receipt digest; its shape is still gated.
    if !closed_tables_shape_value(receipt) {
        return Err(validation_error(
            "E042",
            "envelope.receipt.closed_tables",
            "envelope.receipt.closed_tables must be an array of `{ path, keys }` tables with string names",
        ));
    }
    // A forged audit could copy material into a field a receiver may log.
    // The producer redacts; the receiver refuses text it knows is
    // material (it cannot detect a secret only the sender knew).
    if let Some(profile) = material {
        if closed_tables_echo_material(receipt, profile) {
            return Err(validation_error(
                "E042",
                "envelope.receipt.closed_tables",
                "a closed-table audit entry would echo material",
            ));
        }
    }
    if let Some(Value::Table(sender)) = fields.get("sender") {
        match value_text(sender, "id") {
            FieldText::Text(id) if !id.trim().is_empty() && !id.chars().any(char::is_control) => {}
            _ => {
                return Err(validation_error(
                    "E043",
                    "envelope.sender.id",
                    "envelope.sender.id must be a non-empty, control-free string",
                ))
            }
        }
        let key = value_text(sender, "key");
        let signature = value_text(sender, "signature");
        if matches!(key, FieldText::Absent) != matches!(signature, FieldText::Absent) {
            return Err(validation_error(
                "E043",
                "envelope.sender",
                "envelope.sender.key and envelope.sender.signature must be given together",
            ));
        }
        if let FieldText::Text(key) = &key {
            if !crate::sign::valid_kid(key) {
                return Err(validation_error(
                    "E043",
                    "envelope.sender.key",
                    "envelope.sender.key must be a valid key id",
                ));
            }
        }
        if let FieldText::Text(signature) = &signature {
            if signature.len() != 128 || !signature.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(validation_error(
                    "E043",
                    "envelope.sender.signature",
                    "envelope.sender.signature must be 128 hex characters",
                ));
            }
        }
        if matches!(key, FieldText::Malformed) || matches!(signature, FieldText::Malformed) {
            return Err(validation_error(
                "E043",
                "envelope.sender",
                "envelope.sender fields must be literal strings",
            ));
        }
    } else if fields.get("sender").is_some() {
        return Err(validation_error(
            "E043",
            "envelope.sender",
            "envelope.sender must be a table",
        ));
    }
    let envelope_result = value_text(fields, "result");
    let receipt_result = value_text(receipt, "result");
    for (value, path) in [
        (&envelope_result, "envelope.result"),
        (&receipt_result, "envelope.receipt.result"),
    ] {
        match value {
            FieldText::Absent => {}
            FieldText::Text(text) if is_sha256(text) => {}
            _ => {
                return Err(validation_error(
                    "E042",
                    path,
                    "result fields must be `sha256:<64 hex>`",
                ))
            }
        }
    }
    if outcome == "failed" && sender_signature_present(fields) {
        return Err(validation_error(
            "E043",
            "envelope.sender",
            "a failed receipt cannot carry a sender signature",
        ));
    }
    match value_text(fields, "capability") {
        FieldText::Text(capability) if is_closed_capability(&capability) => {}
        _ => {
            return Err(validation_error(
                "E043",
                "envelope.capability",
                "envelope.capability must be strict, data, agent, or trusted",
            ))
        }
    }
    if outcome == "clean" {
        // Receipt metadata is authoritative for reconstruction and must
        // match the local context (review H1). Cross-host budgets are
        // accepted only when they do not exceed this receiver's ceiling.
        let local_mode = match module.mode {
            Mode::Config => "config",
            Mode::Data => "data",
        };
        match value_text(receipt, "mode") {
            FieldText::Text(mode) if mode == local_mode => {}
            _ => {
                return Err(validation_error(
                    "E042",
                    "envelope.receipt.mode",
                    "a clean receipt must name the evaluated mode",
                ))
            }
        }
        match value_text(receipt, "profile") {
            FieldText::Absent if profile.is_none() => {}
            FieldText::Text(value) if Some(value.as_str()) == profile => {}
            _ => {
                return Err(validation_error(
                    "E042",
                    "envelope.receipt.profile",
                    "envelope.receipt.profile must match the effective profile",
                ))
            }
        }
        let local_schema = document_schema_version(module);
        match value_text(receipt, "schema_version") {
            FieldText::Absent if local_schema.is_none() => {}
            FieldText::Text(value) if Some(&value) == local_schema.as_ref() => {}
            _ => {
                return Err(validation_error(
                    "E042",
                    "envelope.receipt.schema_version",
                    "envelope.receipt.schema_version must match the document schema",
                ))
            }
        }
        let declared_budget = parse_budget(receipt)?;
        let local = budget.resolve();
        for (declared, ceiling, key) in [
            (declared_budget.max_nodes, local.max_nodes, "nodes"),
            (declared_budget.max_depth, local.max_depth, "depth"),
            (
                declared_budget.max_array_len,
                local.max_array_len,
                "array_len",
            ),
            (declared_budget.max_keys, local.max_keys, "keys"),
            (declared_budget.max_wall_ms, local.max_wall_ms, "wall_ms"),
            (
                declared_budget.max_value_size,
                local.max_value_size,
                "value_size",
            ),
            (
                declared_budget.max_total_value_size,
                local.max_total_value_size,
                "total_size",
            ),
        ] {
            if declared > ceiling {
                return Err(EvalError::ValidationFailed {
                    diagnostics: vec![Diagnostic {
                        severity: Severity::Error,
                        code: "E044".into(),
                        message: format!("envelope.receipt.budget.{key} exceeds the local ceiling"),
                        path: Some("envelope.receipt.budget".into()),
                        span: None,
                        repair: None,
                        confidence: 1.0,
                        file: None,
                    }],
                });
            }
        }
        let (FieldText::Text(left), FieldText::Text(right)) = (&envelope_result, &receipt_result)
        else {
            return Err(validation_error(
                "E042",
                "envelope.result",
                "a clean v2 envelope must carry `result` and `receipt.result`",
            ));
        };
        if left != right {
            return Err(validation_error(
                "E042",
                "envelope.result",
                "envelope.result and envelope.receipt.result must be equal",
            ));
        }
        // A present key/signature must verify against a trusted key. No
        // trust set means the signature cannot be accepted (review H4).
        if let Some(Value::Table(sender)) = fields.get("sender") {
            if let (FieldText::Text(key), FieldText::Text(signature)) =
                (value_text(sender, "key"), value_text(sender, "signature"))
            {
                let id = match value_text(sender, "id") {
                    FieldText::Text(id) => id,
                    _ => {
                        return Err(validation_error(
                            "E043",
                            "envelope.sender.id",
                            "envelope.sender.id must be a string",
                        ))
                    }
                };
                let trust = trust.ok_or_else(|| {
                    validation_error(
                        "E045",
                        "envelope.sender",
                        "sender signature cannot be verified: no trusted keys configured",
                    )
                })?;
                if trust.revoked.contains(&key) {
                    return Err(validation_error(
                        "E045",
                        "envelope.sender.key",
                        "sender key is revoked by the trust file",
                    ));
                }
                let verifying = trust.key(&key).ok_or_else(|| {
                    validation_error(
                        "E045",
                        "envelope.sender.key",
                        "sender key is not in the trusted set",
                    )
                })?;
                let version = match fields.get("version") {
                    Some(Value::String(version)) => version.clone(),
                    _ => String::new(),
                };
                let digest = receipt_digest(&Value::Table(receipt.clone()));
                crate::envelope::verify_sender(
                    &crate::sign::public_hex(verifying),
                    &id,
                    &key,
                    &version,
                    left,
                    &digest,
                    &signature,
                )
                .map_err(|reason| EvalError::ValidationFailed {
                    diagnostics: vec![Diagnostic {
                        severity: Severity::Error,
                        code: "E045".into(),
                        message: format!("sender signature is invalid: {reason}"),
                        path: Some("envelope.sender".into()),
                        span: None,
                        repair: None,
                        confidence: 1.0,
                        file: None,
                    }],
                })?;
            }
        }
        if !fields.contains_key("payload") {
            return Err(validation_error(
                "E042",
                "envelope.payload",
                "a clean v2 envelope must carry its payload",
            ));
        }
        let tainted = match fields.get("tainted") {
            Some(Value::Bool(tainted)) => *tainted,
            _ => {
                return Err(validation_error(
                    "E035",
                    "envelope.tainted",
                    "envelope.tainted must be a boolean claim",
                ))
            }
        };
        if tainted && sender_signature_present(fields) {
            return Err(validation_error(
                "E043",
                "envelope.sender",
                "a tainted payload cannot be authorship-bound; omit the signature",
            ));
        }
        let payload = fields.get("payload").cloned().unwrap_or(Value::Null);
        let payload = match material {
            Some(profile) => crate::convert::redact_value(profile, &payload),
            None => payload,
        };
        let capability = match value_text(fields, "capability") {
            FieldText::Text(capability) if is_closed_capability(&capability) => Some(capability),
            FieldText::Absent => None,
            _ => {
                return Err(validation_error(
                    "E043",
                    "envelope.capability",
                    "envelope.capability must be strict, data, agent, or trusted",
                ))
            }
        };
        let computed = bound_content_hash(
            &payload,
            document_schema_version(module).as_deref(),
            profile,
            &declared_budget,
            module.mode,
            capability.as_deref(),
            tainted,
        );
        if &computed != left {
            return Err(validation_error(
                "E044",
                "envelope.result",
                "envelope.result does not match the bound payload/schema/profile/budget/mode/capability",
            ));
        }
    }
    Ok(())
}

/// The document schema's `version`, when it declares one.
pub fn document_schema_version(module: &Module) -> Option<String> {
    let schema = module.schema?;
    match module.get(schema) {
        Some(Node::SchemaTable { meta, .. }) => meta.version.clone(),
        _ => None,
    }
}

/// Whether a payload structurally contains a secret (the taint claim an
/// emitter writes).
pub fn payload_tainted(value: &Value) -> bool {
    crate::eval::value_has_secret(value)
}

/// One audited closed table (L-P11): the document path of the table
/// instance and the key names present at it. Structural only - names,
/// never values, so a receipt can never echo material.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClosedTable {
    pub path: String,
    pub keys: Vec<String>,
}

/// Every closed schema table instance in an evaluated, redacted document,
/// with the key names present at each (L-P11). A closed table is a
/// promise ("no more keys"); this is the audit record of that promise.
///
/// The walk follows the schema tree over the value tree. Disjunction
/// branches participate only when their shape accepts the value
/// (structural acceptance: types, enum membership, required fields,
/// closedness, nested tables and arrays); constraint arguments are not
/// evaluated, because the entry audits the key set of a closed shape, not
/// which validation branch produced a pass. A branch that cannot be
/// proven to match contributes no fact. Paths are dotted
/// (`a.b[0].c`); `"."` is the document root. Entries and key lists are
/// sorted, and identical entries (several structurally matching branches
/// at one path) are deduplicated.
pub fn closed_tables(module: &Module, value: &Value) -> Vec<ClosedTable> {
    let Some(root) = module.schema else {
        return Vec::new();
    };
    let mut facts = Vec::new();
    collect_closed_tables(module, root, value, value, "", &mut facts);
    facts.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.keys.cmp(&right.keys))
    });
    facts.dedup();
    facts
}

fn collect_closed_tables(
    module: &Module,
    schema_id: crate::ir::NodeId,
    value: &Value,
    root: &Value,
    path: &str,
    out: &mut Vec<ClosedTable>,
) {
    match module.get(schema_id) {
        Some(Node::SchemaTable { fields, closed, .. }) => {
            let Value::Table(map) = value else {
                return;
            };
            if *closed {
                // Only declared keys belong to this closed shape. A
                // matching value declares them all; filtering keeps an
                // unexpected key (already T005) out of the audit rather
                // than attributing it to a promise it violates.
                let mut keys: Vec<String> = map
                    .keys()
                    .filter(|key| fields.contains_key(*key))
                    .cloned()
                    .collect();
                keys.sort();
                out.push(ClosedTable {
                    path: if path.is_empty() {
                        ".".to_string()
                    } else {
                        path.to_string()
                    },
                    keys,
                });
            }
            for (name, child) in fields {
                if let Some(child_value) = map.get(name) {
                    let child_path = if path.is_empty() {
                        name.clone()
                    } else {
                        format!("{path}.{name}")
                    };
                    collect_closed_tables(module, *child, child_value, root, &child_path, out);
                }
            }
        }
        Some(Node::SchemaField { field }) => {
            // A disjunction has no single nested table: descend into the
            // branches whose shape the value satisfies.
            for branch in &field.one_of {
                if node_accepts(module, *branch, value, root) {
                    collect_closed_tables(module, *branch, value, root, path, out);
                }
            }
        }
        Some(Node::SchemaArray { element }) => {
            let Value::Array(items) = value else {
                return;
            };
            for (index, item) in items.iter().enumerate() {
                collect_closed_tables(
                    module,
                    *element,
                    item,
                    root,
                    &format!("{path}[{index}]"),
                    out,
                );
            }
        }
        _ => {}
    }
}

/// Structural acceptance for the receipt walk: does this schema shape
/// accept this value? Constraint arguments are deliberately not
/// evaluated (see [`closed_tables`]).
fn node_accepts(
    module: &Module,
    schema_id: crate::ir::NodeId,
    value: &Value,
    root: &Value,
) -> bool {
    match module.get(schema_id) {
        Some(Node::SchemaTable { fields, closed, .. }) => {
            let Value::Table(map) = value else {
                return false;
            };
            if *closed && map.keys().any(|key| !fields.contains_key(key)) {
                return false;
            }
            for (name, child) in fields {
                match map.get(name) {
                    Some(child_value) => {
                        if !node_accepts(module, *child, child_value, root) {
                            return false;
                        }
                    }
                    None => {
                        if field_required(module, *child, map, root) {
                            return false;
                        }
                    }
                }
            }
            true
        }
        Some(Node::SchemaField { field }) => {
            if field.one_of.is_empty() {
                leaf_accepts(&field.typ, value)
            } else {
                field
                    .one_of
                    .iter()
                    .any(|branch| node_accepts(module, *branch, value, root))
            }
        }
        Some(Node::SchemaArray { element }) => match value {
            Value::Array(items) => items
                .iter()
                .all(|item| node_accepts(module, *element, item, root)),
            _ => false,
        },
        _ => false,
    }
}

/// A table entry is required unless it is Null-typed, a deprecated field
/// with a default (omission is the migration path), or a field whose
/// `requires` trigger is disabled (absence is its contract, L-P10 row 3).
/// Structural nodes (nested tables and arrays) are always required.
fn field_required(
    module: &Module,
    field_id: crate::ir::NodeId,
    table: &indexmap::IndexMap<String, Value>,
    root: &Value,
) -> bool {
    let Some(Node::SchemaField { field }) = module.get(field_id) else {
        return true;
    };
    if matches!(field.typ, crate::types::ActiveType::Null) {
        return false;
    }
    if field.deprecated.is_some() && field.default.is_some() {
        return false;
    }
    if let Some(target) = literal_requires_target(module, field.requires) {
        if crate::convert::requires_target_exists(module, field_id, &target) {
            let sibling = crate::validate::table_has(table, &target);
            let absolute = matches!(root, Value::Table(root_map)
                if crate::validate::table_has(root_map, &target));
            if !sibling && !absolute {
                return false;
            }
        }
    }
    true
}

/// The authored text of a `requires` target, for the excuse rule. A
/// literal or a reference to a literal is the documented form; a computed
/// target stays required (fail closed, as validation does).
fn literal_requires_target(module: &Module, slot: Option<crate::ir::NodeId>) -> Option<String> {
    match slot.and_then(|id| module.get(id)) {
        Some(Node::Literal(Value::String(text))) => Some(text.clone()),
        Some(Node::Ref(to)) => match module.get(*to) {
            Some(Node::Literal(Value::String(text))) => Some(text.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Leaf type acceptance, with the enum member check done here (validation
/// reports a wrong member as T003, so `type_accepts` alone would accept
/// any string).
fn leaf_accepts(typ: &crate::types::ActiveType, value: &Value) -> bool {
    match typ {
        crate::types::ActiveType::Enum(members) => {
            matches!(value, Value::String(text) if members.iter().any(|member| member == text))
        }
        _ => crate::validate::type_accepts(typ, value),
    }
}

/// Why a Data Mode document that carries an `envelope` table is not
/// self-contained (LANG-F-007), one entry per violation: the offending node
/// (for a span) and a message. `env.*` reads, `include` statements, and
/// profiles all resolve against the RECEIVER's ambient state, so the
/// sender's meaning silently rebinds. Shared by the sender-side lint (L017)
/// and the receiver gate (E036) so the two can never disagree.
pub(crate) fn self_containment_violations(
    module: &Module,
) -> Vec<(Option<crate::ir::NodeId>, String)> {
    self_containment_violations_inner(module, true)
}

/// Scan without requiring the SOURCE root to carry `envelope`: used by
/// the post-evaluation check, where a profile/patch can inject one
/// (review H3b).
pub(crate) fn self_containment_violations_any(
    module: &Module,
) -> Vec<(Option<crate::ir::NodeId>, String)> {
    self_containment_violations_inner(module, false)
}

fn self_containment_violations_inner(
    module: &Module,
    require_envelope: bool,
) -> Vec<(Option<crate::ir::NodeId>, String)> {
    let mut out = Vec::new();
    if module.mode != crate::ir::Mode::Data {
        return out;
    }
    let Some(root) = module.root else {
        return out;
    };
    let Some(Node::Table { fields }) = module.get(root) else {
        return out;
    };
    if require_envelope && !fields.contains_key("envelope") {
        return out;
    }
    for (index, node) in module.arena.iter().enumerate() {
        if let Node::Env { .. } = node {
            out.push((
                Some(crate::ir::NodeId(index as u32)),
                "interchange document reads the receiver's environment; pin the value before \
                 transmission"
                    .to_string(),
            ));
        }
    }
    for _include in &module.includes {
        out.push((
            None,
            "interchange document includes a file; the receiver resolves includes against its \
             own filesystem - inline the content before transmission"
                .to_string(),
        ));
    }
    if !module.profiles.is_empty() {
        out.push((
            None,
            "interchange document declares profiles; the receiver selects a profile from its \
             own configuration - send the selected value"
                .into(),
        ));
    }
    out
}

/// The state of the sender's `envelope.tainted` claim (LANG-F-004). This is
/// an UNVERIFIED sender assertion: the receiver cross-checks it against the
/// evaluated *payload's* structural taint, and a missing or non-boolean
/// claim in an envelope table is malformed rather than silently ignored.
pub(crate) enum TaintClaim {
    /// The document has no envelope table: a plain data document makes no
    /// claim to police.
    NoEnvelope,
    /// A literal boolean claim.
    Claim(bool),
    /// The envelope exists but has no `tainted` member.
    Missing,
    /// The member exists but does not resolve to a literal boolean.
    Malformed,
}

pub(crate) fn sender_taint(module: &Module) -> TaintClaim {
    let refs = match envelope_fields(module) {
        EnvelopeLookup::Found(refs) => refs,
        EnvelopeLookup::Absent => return TaintClaim::NoEnvelope,
        EnvelopeLookup::Unresolvable(_) => return TaintClaim::Malformed,
    };
    // An `envelope` key is the interchange envelope in Data mode, or when it
    // carries a version. In Config mode without a version it is ordinary
    // data, and policing it would reject unrelated documents (LANG-F-004:
    // the claim rule applies to envelopes, not to any table named
    // `envelope`).
    if module.mode != crate::ir::Mode::Data && refs.version.is_none() {
        return TaintClaim::NoEnvelope;
    }
    let Some(table) = deref_table(module, refs.envelope) else {
        return TaintClaim::Malformed;
    };
    let fields = match module.get(table) {
        Some(Node::Table { fields }) => fields,
        _ => return TaintClaim::Malformed,
    };
    let Some(mut id) = fields.get("tainted").copied() else {
        return TaintClaim::Missing;
    };
    for _ in 0..64 {
        match module.get(id) {
            Some(Node::Literal(Value::Bool(claimed))) => return TaintClaim::Claim(*claimed),
            Some(Node::Ref(next)) => id = *next,
            _ => return TaintClaim::Malformed,
        }
    }
    TaintClaim::Malformed
}

/// The root envelope table and its `version` node. `Absent` is a plain
/// data document; `Unresolvable` is a present `envelope` key that is not
/// a table (or an alias chain past the bound) and must fail closed rather
/// than pass as "no envelope" (review H3).
struct EnvelopeRefs {
    envelope: crate::ir::NodeId,
    version: Option<crate::ir::NodeId>,
}

enum EnvelopeLookup {
    Absent,
    Unresolvable(crate::ir::NodeId),
    Found(EnvelopeRefs),
}

fn envelope_fields(module: &Module) -> EnvelopeLookup {
    let Some(root) = module.root else {
        return EnvelopeLookup::Absent;
    };
    let fields = match module.get(root) {
        Some(Node::Table { fields }) => fields,
        _ => return EnvelopeLookup::Absent,
    };
    let Some(envelope) = fields.get("envelope").copied() else {
        return EnvelopeLookup::Absent;
    };
    let Some(table) = deref_table(module, envelope) else {
        return EnvelopeLookup::Unresolvable(envelope);
    };
    let version = match module.get(table) {
        Some(Node::Table { fields }) => fields.get("version").copied(),
        _ => None,
    };
    EnvelopeLookup::Found(EnvelopeRefs { envelope, version })
}

fn v1_has_v2_fields(module: &Module, refs: &EnvelopeRefs) -> bool {
    let Some(table) = deref_table(module, refs.envelope) else {
        return false;
    };
    let Some(Node::Table { fields }) = module.get(table) else {
        return false;
    };
    fields.contains_key("receipt") || fields.contains_key("sender")
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
                    None => VersionCheck::Unparseable,
                }
            }
            Some(Node::Literal(_)) => return VersionCheck::Unparseable,
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
    Unparseable,
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

/// Domain-separated value for the bound hash: a `Secret` serializes as
/// `{"$active.secret":"***SECRET***"}`, so it cannot share bytes with a
/// literal string `"***SECRET***"` (review H2). Secret *values* remain
/// non-authenticatable by design; a tainted payload cannot carry a sender
/// signature (E043).
fn hash_value(value: &Value) -> Value {
    match value {
        Value::Table(fields) => Value::Table(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), hash_value(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(hash_value).collect()),
        Value::Secret(_) => {
            let mut marker = indexmap::IndexMap::new();
            marker.insert(
                "$active.secret".to_string(),
                Value::String("***SECRET***".to_string()),
            );
            Value::Table(marker)
        }
        other => other.clone(),
    }
}

/// Parse the required clean-receipt budget table into resolved limits.
fn parse_budget(
    receipt: &indexmap::IndexMap<String, Value>,
) -> Result<crate::eval::Budget, EvalError> {
    let table = match value_table(receipt, "budget") {
        Ok(Some(table)) => table,
        _ => {
            return Err(validation_error(
                "E042",
                "envelope.receipt.budget",
                "a clean receipt must carry its resolved budget limits",
            ))
        }
    };
    let number = |key: &str| -> Result<u64, EvalError> {
        match table.get(key) {
            Some(Value::Int(value)) if *value > 0 => Ok(*value as u64),
            _ => Err(validation_error(
                "E042",
                "envelope.receipt.budget",
                "envelope.receipt.budget fields must be non-negative integers",
            )),
        }
    };
    Ok(crate::eval::Budget {
        max_nodes: number("nodes")?,
        max_depth: number("depth")?,
        max_array_len: number("array_len")?,
        max_keys: number("keys")?,
        max_wall_ms: number("wall_ms")?,
        max_value_size: number("value_size")?,
        max_total_value_size: number("total_size")?,
    })
}

fn sender_signature_present(fields: &indexmap::IndexMap<String, Value>) -> bool {
    match fields.get("sender") {
        Some(Value::Table(sender)) => matches!(sender.get("signature"), Some(Value::String(_))),
        _ => false,
    }
}

/// v2 result identity (L-T06): the payload plus the schema version,
/// profile, resolved budget, mode, and capability that produced it.
/// Redact plain locator copies before calling; blob payloads are stripped
/// exactly as [`crate::provenance::content_hash`] strips them. The JSON is
/// key-sorted recursively, so source table order cannot change identity
/// (B-004 review M4).
pub fn bound_content_hash(
    value: &Value,
    schema_version: Option<&str>,
    profile: Option<&str>,
    budget: &crate::eval::Budget,
    mode: Mode,
    capability: Option<&str>,
    tainted: bool,
) -> String {
    let budget = budget.resolve();
    let mut object = serde_json::Map::new();
    object.insert("active.envelope".into(), serde_json::json!(2));
    object.insert("tainted".into(), serde_json::json!(tainted));
    object.insert(
        "mode".into(),
        serde_json::json!(match mode {
            Mode::Config => "config",
            Mode::Data => "data",
        }),
    );
    object.insert(
        "budget".into(),
        serde_json::json!({
            "nodes": budget.max_nodes,
            "depth": budget.max_depth,
            "array_len": budget.max_array_len,
            "keys": budget.max_keys,
            "wall_ms": budget.max_wall_ms,
            "value_size": budget.max_value_size,
            "total_size": budget.max_total_value_size,
        }),
    );
    if let Some(version) = schema_version {
        object.insert("schema_version".into(), serde_json::json!(version));
    }
    if let Some(profile) = profile {
        object.insert("profile".into(), serde_json::json!(profile));
    }
    if let Some(capability) = capability {
        object.insert("capability".into(), serde_json::json!(capability));
    }
    object.insert(
        "value".into(),
        crate::convert::value_to_json(&crate::convert::strip_blob_payloads(&hash_value(value))),
    );
    let canonical = canonical_json(&serde_json::Value::Object(object));
    format!(
        "sha256:{}",
        crate::provenance::sha256_hex(canonical.as_bytes())
    )
}

/// Canonical JSON bytes: object keys sorted recursively, scalars through
/// serde_json. Used for the v2 bound hash and the sender receipt digest.
pub fn canonical_json(value: &serde_json::Value) -> String {
    fn write(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(key).unwrap_or_default());
                    out.push(':');
                    write(&map[key], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

/// Canonical digest of a receipt, signed by the sender so outcome,
/// failure class, mode, profile, schema version, and budget cannot be
/// changed after signing (review H4).
pub fn receipt_digest(receipt: &Value) -> String {
    let json = crate::convert::value_to_json(&crate::convert::strip_blob_payloads(receipt));
    format!(
        "sha256:{}",
        crate::provenance::sha256_hex(canonical_json(&json).as_bytes())
    )
}

/// The bytes a v2 sender signs: the sender identity, key id, envelope
/// version, bound result hash, and receipt digest. Fields are length-
/// prefixed so no two tuples can concatenate to the same bytes (review
/// M2); the receipt digest binds outcome/failure class/mode/profile/
/// schema/budget to the signature (review H4).
pub fn sender_message(
    sender_id: &str,
    key_id: &str,
    envelope_version: &str,
    result: &str,
    receipt_digest: &str,
) -> Vec<u8> {
    let mut out = String::from("active-envelope-sender-v2\n");
    for field in [sender_id, key_id, envelope_version, result, receipt_digest] {
        out.push_str(&field.len().to_string());
        out.push(':');
        out.push_str(field);
        out.push('\n');
    }
    out.into_bytes()
}

/// Sign an envelope sender block with an Ed25519 secret key.
pub fn sign_sender(
    key: &SigningKey,
    sender_id: &str,
    key_id: &str,
    envelope_version: &str,
    result: &str,
    receipt_digest: &str,
) -> Result<String, String> {
    if sender_id.trim().is_empty() || sender_id.chars().any(char::is_control) {
        return Err("refusing to sign an empty or control-bearing sender identity".to_string());
    }
    if !crate::sign::valid_kid(key_id) {
        return Err("refusing to sign under an invalid key id".to_string());
    }
    if parse_major(envelope_version) != Some(2) {
        return Err("refusing to sign a non-v2 envelope version".to_string());
    }
    if !is_sha256(result) || !is_sha256(receipt_digest) {
        return Err("refusing to sign a malformed result or receipt digest".to_string());
    }
    let signature = key.sign(&sender_message(
        sender_id,
        key_id,
        envelope_version,
        result,
        receipt_digest,
    ));
    Ok(hex_encode(&signature.to_bytes()))
}

/// Verify a v2 sender block against a public key (64 hex chars). Every
/// failure is a plain reason; callers map it to their transport code.
#[allow(clippy::too_many_arguments)]
pub fn verify_sender(
    public_hex: &str,
    sender_id: &str,
    key_id: &str,
    envelope_version: &str,
    result: &str,
    receipt_digest: &str,
    signature_hex: &str,
) -> Result<(), String> {
    if !crate::sign::valid_kid(key_id) {
        return Err("sender key id is invalid".to_string());
    }
    let public = hex_decode(public_hex)
        .ok_or_else(|| "sender public key is not 64 hex characters".to_string())?;
    let public: [u8; 32] = public
        .try_into()
        .map_err(|_| "sender public key is not 32 bytes".to_string())?;
    let key = VerifyingKey::from_bytes(&public)
        .map_err(|e| format!("sender public key is invalid: {e}"))?;
    let signature =
        hex_decode(signature_hex).ok_or_else(|| "sender signature is not hex".to_string())?;
    let signature = Signature::from_slice(&signature)
        .map_err(|e| format!("sender signature is malformed: {e}"))?;
    key.verify_strict(
        &sender_message(sender_id, key_id, envelope_version, result, receipt_digest),
        &signature,
    )
    .map_err(|_| "sender signature does not match the envelope receipt".to_string())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    let (pairs, _) = bytes.as_chunks::<2>();
    for pair in pairs {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push((high as u8) << 4 | low as u8);
    }
    Some(out)
}

/// `sha256:<64 hex>`.
fn is_sha256(text: &str) -> bool {
    let Some(hex) = text.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Follow `Ref` links to a string literal.
fn literal_string(module: &Module, mut id: crate::ir::NodeId) -> Option<String> {
    for _ in 0..64 {
        match module.get(id)? {
            Node::Literal(Value::String(text)) => return Some(text.clone()),
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

#[cfg(test)]
mod v2_tests {
    use super::*;

    const RECEIPT: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn bound(
        value: &Value,
        schema: Option<&str>,
        profile: Option<&str>,
        budget: &crate::eval::Budget,
        mode: Mode,
    ) -> String {
        bound_content_hash(value, schema, profile, budget, mode, Some("agent"), false)
    }

    fn module(src: &str) -> Module {
        crate::parser::parse(src, "envelope").expect("parse")
    }

    const V2_OK: &str = r#"active_mode = "data"
envelope = {
  version = "2.0.0",
  capability = "agent",
  tainted = false,
  payload = { ok = true },
  receipt = { outcome = "clean", mode = "data", result = "sha256:96b0634c75260144b34ae641a7d11a0a054a9edb6978a7e5bc097dc6ab6674a4", budget = { nodes = 100000, depth = 256, array_len = 100000, keys = 100000, wall_ms = 100, value_size = 10000000, total_size = 100000000 } },
  sender = { id = "agent://example" },
  result = "sha256:96b0634c75260144b34ae641a7d11a0a054a9edb6978a7e5bc097dc6ab6674a4"
}
"#;

    fn codes(result: Result<(), EvalError>) -> Vec<String> {
        match result {
            Ok(()) => Vec::new(),
            Err(EvalError::ValidationFailed { diagnostics }) => {
                diagnostics.iter().map(|d| d.code.clone()).collect()
            }
            Err(error) => vec![error.code().to_string()],
        }
    }

    #[test]
    fn a_well_formed_v2_envelope_passes() {
        let gate = EnvelopeGate::shipped();
        assert!(
            gate.check(&module(V2_OK)).is_ok(),
            "v2 envelope must pass: {:?}",
            codes(gate.check(&module(V2_OK)))
        );
    }

    #[test]
    fn v2_receipt_and_sender_shapes_are_enforced() {
        let gate = EnvelopeGate::shipped();
        for (label, src) in [
            (
                "missing receipt",
                "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true } }\n",
            ),
            (
                "bad outcome",
                "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"maybe\" } }\n",
            ),
            (
                "sender not a table",
                "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"clean\" }, sender = \"agent\" }\n",
            ),
            (
                "sender without signature",
                "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"clean\" }, sender = { id = \"a\", key = \"k\" } }\n",
            ),
            (
                "bad signature shape",
                "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"clean\" }, sender = { id = \"a\", key = \"k\", signature = \"abc\" } }\n",
            ),
            (
                "bad result",
                "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"clean\" }, result = \"abc\" }\n",
            ),
        ] {
            let got = codes(gate.check(&module(src)));
            assert!(
                got.iter().any(|c| c == "E042" || c == "E043"),
                "{label}: E042/E043 expected, got {got:?}"
            );
        }
        let got = codes(gate.check(&module(
            "active_mode = \"data\"\nenvelope = { version = \"3.0.0\" }\n",
        )));
        assert_eq!(got, vec!["E033"], "unknown major is still E033");
    }

    #[test]
    fn bound_hash_covers_payload_schema_profile_budget_and_mode() {
        let budget = crate::eval::Budget::default();
        let base = bound(
            &Value::Int(1),
            Some("1.0.0"),
            Some("prod"),
            &budget,
            Mode::Config,
        );
        assert_eq!(
            base,
            bound(
                &Value::Int(1),
                Some("1.0.0"),
                Some("prod"),
                &budget,
                Mode::Config
            )
        );
        assert_ne!(
            base,
            bound(
                &Value::Int(2),
                Some("1.0.0"),
                Some("prod"),
                &budget,
                Mode::Config
            )
        );
        assert_ne!(
            base,
            bound(
                &Value::Int(1),
                Some("2.0.0"),
                Some("prod"),
                &budget,
                Mode::Config
            )
        );
        assert_ne!(
            base,
            bound(
                &Value::Int(1),
                Some("1.0.0"),
                Some("dev"),
                &budget,
                Mode::Config
            )
        );
        assert_ne!(
            base,
            bound(
                &Value::Int(1),
                Some("1.0.0"),
                Some("prod"),
                &budget,
                Mode::Data
            )
        );
        let mut other_budget = crate::eval::Budget::default();
        other_budget.max_nodes += 1;
        assert_ne!(
            base,
            bound(
                &Value::Int(1),
                Some("1.0.0"),
                Some("prod"),
                &other_budget,
                Mode::Config
            )
        );
        assert!(base.starts_with("sha256:") && base.len() == 71);
    }

    #[test]
    fn sender_signature_binds_the_receipt() {
        let key = crate::sign::generate_secret_key().expect("key");
        let public = crate::sign::public_hex(&key.verifying_key());
        let result = "sha256:96b0634c75260144b34ae641a7d11a0a054a9edb6978a7e5bc097dc6ab6674a4";
        let signature =
            sign_sender(&key, "agent://a", "k1", "2.0.0", result, RECEIPT).expect("sign");
        verify_sender(
            &public,
            "agent://a",
            "k1",
            "2.0.0",
            result,
            RECEIPT,
            &signature,
        )
        .expect("round trip");
        assert!(
            verify_sender(
                &public,
                "agent://a",
                "k1",
                "2.0.0",
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                RECEIPT,
                &signature
            )
            .is_err(),
            "a different result must not verify"
        );
        assert!(
            verify_sender(
                &public,
                "agent://b",
                "k1",
                "2.0.0",
                result,
                RECEIPT,
                &signature
            )
            .is_err(),
            "a different sender must not verify"
        );
        assert!(sign_sender(&key, "", "k1", "2.0.0", result, RECEIPT).is_err());
        assert!(sign_sender(&key, "a", "bad\"kid", "2.0.0", result, RECEIPT).is_err());
    }
}

#[cfg(test)]
mod strict_v2_tests {
    use super::*;

    fn module(src: &str) -> Module {
        crate::parser::parse(src, "envelope").expect("parse")
    }

    fn codes(err: EvalError) -> Vec<String> {
        match err {
            EvalError::ValidationFailed { diagnostics } => {
                diagnostics.iter().map(|d| d.code.clone()).collect()
            }
            other => vec![other.code().to_string()],
        }
    }

    #[test]
    fn a_malformed_envelope_is_not_absent() {
        let gate = EnvelopeGate::shipped();
        let err = gate
            .check(&module(
                "active_mode = \"data\"\nenvelope = \"not a table\"\n",
            ))
            .expect_err("must refuse");
        assert_eq!(codes(err), vec!["E033"]);
    }

    #[test]
    fn a_failed_receipt_requires_its_class() {
        let gate = EnvelopeGate::shipped();
        for src in [
            "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"failed\" } }\n",
            "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"failed\", failure_class = 5 } }\n",
        ] {
            let err = gate.check(&module(src)).expect_err("must refuse");
            assert_eq!(codes(err), vec!["E042"], "{src}");
        }
    }

    #[test]
    fn clean_results_are_required_equal_and_sha256() {
        let gate = EnvelopeGate::shipped();
        let base = "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"clean\"";
        for suffix in [
            " } }\n",
            ", result = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" } }\n",
            ", result = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" }, result = \"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\" }\n",
            ", result = 5 }, result = 5 }\n",
        ] {
            let err = gate.check(&module(&format!("{base}{suffix}"))).expect_err("must refuse");
            assert!(
                codes(err).iter().any(|c| c == "E042" || c == "E043"),
                "{suffix}"
            );
        }
    }

    #[test]
    fn closed_metadata_and_sender_shapes() {
        let gate = EnvelopeGate::shipped();
        for src in [
            "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", capability = \"root\", tainted = false, payload = { ok = true }, receipt = { outcome = \"clean\", result = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" }, result = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" }\n",
            "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"clean\", result = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\", mode = \"bogus\" }, result = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" }\n",
            "active_mode = \"data\"\nenvelope = { version = \"2.0.0\", tainted = false, payload = { ok = true }, receipt = { outcome = \"clean\", result = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" }, sender = { id = \"  \" }, result = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" }\n",
        ] {
            assert!(gate.check(&module(src)).is_err(), "{src}");
        }
    }

    #[test]
    fn canonical_hashing_ignores_table_order() {
        let mut a = indexmap::IndexMap::new();
        a.insert("b".to_string(), Value::Int(2));
        a.insert("a".to_string(), Value::Int(1));
        let mut b = indexmap::IndexMap::new();
        b.insert("a".to_string(), Value::Int(1));
        b.insert("b".to_string(), Value::Int(2));
        let budget = crate::eval::Budget::default();
        assert_eq!(
            bound_content_hash(
                &Value::Table(a),
                None,
                None,
                &budget,
                Mode::Data,
                Some("agent"),
                false,
            ),
            bound_content_hash(
                &Value::Table(b),
                None,
                None,
                &budget,
                Mode::Data,
                Some("agent"),
                false,
            )
        );
    }

    #[test]
    fn sign_sender_refuses_malformed_shapes() {
        let key = crate::sign::generate_secret_key().expect("key");
        let result = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let receipt = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        assert!(sign_sender(&key, "a", "k", "1.0.0", result, receipt).is_err());
        assert!(sign_sender(&key, "a", "k", "2.0.0", "not-a-hash", receipt).is_err());
        assert!(sign_sender(&key, "a", "k", "2.0.0", result, "not-a-hash").is_err());
        let sig = sign_sender(&key, "a", "k", "2.0.0", result, receipt).expect("sign");
        let public = crate::sign::public_hex(&key.verifying_key());
        assert!(verify_sender(&public, "a", "k", "2.0.0", result, receipt, &sig).is_ok());
        let other_receipt =
            "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        assert!(verify_sender(&public, "a", "k", "2.0.0", result, other_receipt, &sig).is_err());
    }
}

#[cfg(test)]
mod trust_tests {
    use super::*;

    fn table(pairs: Vec<(&str, Value)>) -> Value {
        let mut map = indexmap::IndexMap::new();
        for (key, value) in pairs {
            map.insert(key.to_string(), value);
        }
        Value::Table(map)
    }

    fn budget_value() -> Value {
        let b = crate::eval::Budget::default();
        table(vec![
            ("nodes", Value::Int(b.max_nodes as i64)),
            ("depth", Value::Int(b.max_depth as i64)),
            ("array_len", Value::Int(b.max_array_len as i64)),
            ("keys", Value::Int(b.max_keys as i64)),
            ("wall_ms", Value::Int(b.max_wall_ms as i64)),
            ("value_size", Value::Int(b.max_value_size as i64)),
            ("total_size", Value::Int(b.max_total_value_size as i64)),
        ])
    }

    #[test]
    fn a_signed_v2_envelope_requires_and_verifies_trust() {
        let key = crate::sign::generate_secret_key().expect("key");
        let public = crate::sign::public_hex(&key.verifying_key());
        let trust = crate::sign::load_trusted_keys(&format!(
            "[[keys]]\nid = \"k\"\npublic = \"{public}\"\n"
        ))
        .expect("trust");
        let payload = table(vec![("ok", Value::Bool(true)), ("port", Value::Int(8080))]);
        let budget = crate::eval::Budget::default();
        let bound = bound_content_hash(
            &payload,
            None,
            None,
            &budget,
            Mode::Data,
            Some("agent"),
            false,
        );
        let receipt = table(vec![
            ("outcome", Value::String("clean".into())),
            ("mode", Value::String("data".into())),
            ("result", Value::String(bound.clone())),
            ("budget", budget_value()),
        ]);
        let digest = receipt_digest(&receipt);
        let signature =
            sign_sender(&key, "agent://a", "k", "2.0.0", &bound, &digest).expect("sign");
        let sender = table(vec![
            ("id", Value::String("agent://a".into())),
            ("key", Value::String("k".into())),
            ("signature", Value::String(signature)),
        ]);
        let envelope = table(vec![
            ("version", Value::String("2.0.0".into())),
            ("capability", Value::String("agent".into())),
            ("tainted", Value::Bool(false)),
            ("payload", payload),
            ("receipt", receipt),
            ("sender", sender),
            ("result", Value::String(bound)),
        ]);
        let value = table(vec![("envelope", envelope)]);
        let module = crate::parser::parse("active_mode = \"data\"\nx = 1\n", "e").expect("parse");
        let gate = EnvelopeGate::shipped();
        check_evaluated(&value, &module, &gate, &budget, None, None, Some(&trust))
            .expect("a valid signature with trust passes");
        // No trust set: an unverifiable signature is refused.
        let err = check_evaluated(&value, &module, &gate, &budget, None, None, None)
            .expect_err("no trust must refuse a signature");
        assert_eq!(codes(err), vec!["E045"]);
        // A tampered payload no longer matches the bound result.
        let mut fields = match &value {
            Value::Table(root) => root.clone(),
            _ => unreachable!(),
        };
        if let Some(Value::Table(envelope)) = fields.get_mut("envelope") {
            envelope.insert(
                "payload".into(),
                table(vec![("ok", Value::Bool(true)), ("port", Value::Int(9090))]),
            );
        }
        let err = check_evaluated(
            &Value::Table(fields),
            &module,
            &gate,
            &budget,
            None,
            None,
            Some(&trust),
        )
        .expect_err("a tampered payload must fail");
        assert_eq!(codes(err), vec!["E044"]);
    }

    fn codes(err: EvalError) -> Vec<String> {
        match err {
            EvalError::ValidationFailed { diagnostics } => {
                diagnostics.iter().map(|d| d.code.clone()).collect()
            }
            other => vec![other.code().to_string()],
        }
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    #[test]
    fn document_schema_version_is_read_from_schema_meta() {
        let module = crate::parser::parse(
            "schema = { version = \"1.2.0\", port = Int }\nport = 1\n",
            "schema",
        )
        .expect("parse");
        assert_eq!(document_schema_version(&module).as_deref(), Some("1.2.0"));
        let budget = crate::eval::Budget::default();
        let a = bound_content_hash(
            &Value::Int(1),
            Some("1.2.0"),
            None,
            &budget,
            Mode::Data,
            Some("agent"),
            false,
        );
        let b = bound_content_hash(
            &Value::Int(1),
            Some("9.9.9"),
            None,
            &budget,
            Mode::Data,
            Some("agent"),
            false,
        );
        assert_ne!(a, b, "the schema version must change the bound hash");
    }

    #[test]
    fn a_revoked_sender_key_is_refused() {
        let key = crate::sign::generate_secret_key().expect("key");
        let public = crate::sign::public_hex(&key.verifying_key());
        let trust = crate::sign::load_trusted_keys(&format!(
            "revoked = [\"k\"]\n[[keys]]\nid = \"k\"\npublic = \"{public}\"\n"
        ))
        .expect("trust");
        let payload = Value::Table(indexmap::IndexMap::from([(
            "ok".to_string(),
            Value::Bool(true),
        )]));
        let budget = crate::eval::Budget::default();
        let bound = bound_content_hash(
            &payload,
            None,
            None,
            &budget,
            Mode::Data,
            Some("agent"),
            false,
        );
        let mut receipt_fields = indexmap::IndexMap::new();
        receipt_fields.insert("outcome".to_string(), Value::String("clean".into()));
        receipt_fields.insert("mode".to_string(), Value::String("data".into()));
        receipt_fields.insert("result".to_string(), Value::String(bound.clone()));
        let mut budget_fields = indexmap::IndexMap::new();
        for (key, value) in [
            ("nodes", budget.max_nodes),
            ("depth", budget.max_depth),
            ("array_len", budget.max_array_len),
            ("keys", budget.max_keys),
            ("wall_ms", budget.max_wall_ms),
            ("value_size", budget.max_value_size),
            ("total_size", budget.max_total_value_size),
        ] {
            budget_fields.insert(key.to_string(), Value::Int(value as i64));
        }
        receipt_fields.insert("budget".to_string(), Value::Table(budget_fields));
        let receipt = Value::Table(receipt_fields);
        let digest = receipt_digest(&receipt);
        let signature =
            sign_sender(&key, "agent://a", "k", "2.0.0", &bound, &digest).expect("sign");
        let sender = Value::Table(indexmap::IndexMap::from([
            ("id".to_string(), Value::String("agent://a".into())),
            ("key".to_string(), Value::String("k".into())),
            ("signature".to_string(), Value::String(signature)),
        ]));
        let mut fields = indexmap::IndexMap::new();
        fields.insert("version".to_string(), Value::String("2.0.0".into()));
        fields.insert("capability".to_string(), Value::String("agent".into()));
        fields.insert("tainted".to_string(), Value::Bool(false));
        fields.insert("payload".to_string(), payload);
        fields.insert("receipt".to_string(), receipt);
        fields.insert("sender".to_string(), sender);
        fields.insert("result".to_string(), Value::String(bound));
        let value = Value::Table(indexmap::IndexMap::from([(
            "envelope".to_string(),
            Value::Table(fields),
        )]));
        let module = crate::parser::parse("active_mode = \"data\"\nx = 1\n", "e").expect("parse");
        let err = check_evaluated(
            &value,
            &module,
            &EnvelopeGate::shipped(),
            &budget,
            None,
            None,
            Some(&trust),
        )
        .expect_err("a revoked key must be refused");
        let EvalError::ValidationFailed { diagnostics } = err else {
            panic!("expected validation failure")
        };
        assert_eq!(diagnostics[0].code, "E045");
    }
}
