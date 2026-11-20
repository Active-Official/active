//! Propose / gate conformance vectors (L-P62): drive the shipped binary
//! against every case in `conformance/lp62/manifest.toml`, so the
//! verdicts and receipts these vectors pin cannot regress silently.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

#[derive(serde::Deserialize)]
struct Manifest {
    case: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    id: String,
    file: String,
    mode: String,
    verdict: String,
    #[serde(default)]
    codes: Vec<String>,
    #[serde(default)]
    checks: Vec<String>,
    #[serde(default)]
    terminal: Option<String>,
}

#[test]
fn lp62_vectors_hold() {
    let dir = root().join("conformance").join("lp62");
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(dir.join("manifest.toml")).expect("manifest"))
            .expect("manifest parses");
    assert!(
        !manifest.case.is_empty(),
        "the vector set must not be empty"
    );
    for case in &manifest.case {
        let file = dir.join(&case.file);
        let output = match case.mode.as_str() {
            "gate" => Command::new(env!("CARGO_BIN_EXE_active"))
                .args(["gate"])
                .arg(&file)
                .args(["--format", "json"])
                .output()
                .expect("run active"),
            "propose" => Command::new(env!("CARGO_BIN_EXE_active"))
                .args(["propose", "--request", "conformance vector"])
                .arg("--candidate")
                .arg(&file)
                .args(["--rounds", "2", "--format", "json"])
                .output()
                .expect("run active"),
            other => panic!("unknown mode {other}"),
        };
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);
        assert_eq!(
            report["verdict"], case.verdict,
            "{}: wrong verdict: {report}",
            case.id
        );
        assert_eq!(
            output.status.success(),
            case.verdict == "accept",
            "{}: exit code disagrees with the verdict",
            case.id
        );
        let findings = report["findings"]
            .as_array()
            .cloned()
            .or_else(|| {
                report["rounds"].as_array().and_then(|rounds| {
                    rounds
                        .last()
                        .and_then(|round| round["findings"].as_array().cloned())
                })
            })
            .unwrap_or_default();
        let codes: Vec<String> = findings
            .iter()
            .flat_map(|finding| finding["codes"].as_array().cloned().unwrap_or_default())
            .filter_map(|code| code.as_str().map(str::to_string))
            .collect();
        for code in &case.codes {
            assert!(
                codes.iter().any(|found| found == code),
                "{}: expected code {code}: {report}",
                case.id
            );
        }
        for check in &case.checks {
            assert!(
                findings.iter().any(|finding| finding["check"] == *check),
                "{}: expected check {check}: {report}",
                case.id
            );
        }
        if let Some(terminal) = &case.terminal {
            assert_eq!(
                report["terminal"]["code"], *terminal,
                "{}: wrong terminal: {report}",
                case.id
            );
        }
    }
}
