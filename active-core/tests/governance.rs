//! Governance presence (C-064): required before a public 1.0
//! announcement. Pins the documents, their enforceable commitments
//! (fix SLAs, advisory reporting URL, referenced specs), and the
//! files the process names (CODEOWNERS, templates, RFC template).

use std::path::Path;

fn root(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name} missing: {e}"))
}

#[test]
fn governance_documents_exist_with_substance() {
    // Presence + key-phrase pins (not full prose assertions —
    // wording evolves, commitments must not silently vanish).
    let required: &[(&str, &[&str])] = &[
        (
            "SECURITY.md",
            &[
                "private security advisory",
                "3 days",
                "14 days",
                "30 days",
                "Active-Official/active/security/advisories",
                "spec/threat-model.md",
            ],
        ),
        (
            "CODE_OF_CONDUCT.md",
            &[
                "harassment-free",
                "private security advisory",
                "CODEOWNERS",
                "contributor-covenant.org",
            ],
        ),
        (
            "CONTRIBUTING.md",
            &[
                "Spec before code",
                "RFC",
                "Non-goals",
                "--locked",
                "DCO",
                "CODEOWNERS",
            ],
        ),
        (
            "docs/rfcs/README.md",
            &[
                "Lifecycle",
                "Non-goals",
                "14 days",
                "Fast-track",
                "CODEOWNERS",
            ],
        ),
        (
            "docs/rfcs/0000-template.md",
            &[
                "Non-goals check",
                "Test plan",
                "Status: draft",
                "Conformance",
            ],
        ),
        (".github/CODEOWNERS", &["@RatioArtificiosa"]),
        (
            ".github/PULL_REQUEST_TEMPLATE.md",
            &["Checklist item or RFC", "standing rules"],
        ),
        (
            ".github/ISSUE_TEMPLATE/bug_report.md",
            &["Minimal", "reproducer"],
        ),
        (
            ".github/ISSUE_TEMPLATE/proposal.md",
            &["rfc-draft", "Non-goals check"],
        ),
    ];
    for (rel, phrases) in required {
        let text = root(rel);
        for phrase in *phrases {
            assert!(text.contains(phrase), "{rel} missing {phrase:?}");
        }
    }
}
