//! Capability declarations, sandbox profiles, and the negotiation
//! handshake (PROPOSAL §3.5, CHECKLIST Phase CAP).
//!
//! Documents declare what they need via root-level
//! `active_capabilities = [...]` (parsed into `Module::capabilities`).
//! Hosts advertise what they offer via the capability descriptor's
//! tool list. The handshake is set difference: anything declared but
//! not offered is refused, fail-closed, before evaluation. Sandbox
//! profiles bundle a budget preset with a tool subset so one flag
//! selects an auditable execution context.

/// A named sandbox profile: a budget preset plus the tool subset the
/// profile's host offers (PROPOSAL §3.5, CHECKLIST CAP.002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProfile {
    pub name: &'static str,
    pub budget: &'static str,
    pub tools: &'static [&'static str],
}

/// The four sandbox profiles. Mirrored in
/// `agent-kit/capability.schema.json` under `profiles` (a test pins
/// them equal — single contract, two renderings).
pub fn profiles() -> Vec<SandboxProfile> {
    vec![
        SandboxProfile {
            name: "strict",
            budget: "strict",
            tools: &["check", "eval", "format", "lint"],
        },
        SandboxProfile {
            name: "data",
            budget: "interactive",
            tools: &[
                "check",
                "eval",
                "format",
                "lint",
                "convert",
                "graph",
                "generate",
                "schema-introspect",
            ],
        },
        SandboxProfile {
            name: "agent",
            budget: "interactive",
            tools: &[
                "check",
                "eval",
                "format",
                "lint",
                "convert",
                "graph",
                "generate",
                "schema-introspect",
                "explain",
                "patch",
                "repair",
                "compatibility",
                "capability",
            ],
        },
        SandboxProfile {
            name: "trusted",
            budget: "trusted",
            tools: &[
                "check",
                "eval",
                "format",
                "lint",
                "convert",
                "graph",
                "generate",
                "schema-introspect",
                "explain",
                "patch",
                "repair",
                "compatibility",
                "capability",
            ],
        },
    ]
}

/// Look up a sandbox profile by name.
pub fn profile(name: &str) -> Option<SandboxProfile> {
    profiles().into_iter().find(|p| p.name == name)
}

/// Declared capabilities the host does not offer, in document order
/// without duplicates. Empty means the handshake passes.
pub fn refused<'a>(declared: &'a [String], host_tools: &[&str]) -> Vec<&'a str> {
    // Hash lookups: a hostile 1 MB declaration list must stay linear.
    let host: std::collections::HashSet<&str> = host_tools.iter().copied().collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for cap in declared {
        if !host.contains(cap.as_str()) && seen.insert(cap.as_str()) {
            out.push(cap.as_str());
        }
    }
    out
}
