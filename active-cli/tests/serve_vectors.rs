//! Serve transport vectors (L-T05): drive the shipped `active serve`
//! binary over real length-prefixed frames for every case in
//! `conformance/serve/manifest.toml`.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
    outcome: String,
    #[serde(default)]
    codes: Vec<String>,
}

fn frame(bytes: &[u8]) -> Vec<u8> {
    let mut out = (bytes.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}

fn read_frame(stream: &mut impl Read) -> serde_json::Value {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).expect("frame header");
    let len = u32::from_le_bytes(header) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).expect("frame payload");
    serde_json::from_slice(&payload).expect("frame json")
}

#[test]
fn serve_vectors_hold() {
    let dir = root().join("conformance").join("serve");
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(dir.join("manifest.toml")).expect("manifest"))
            .expect("manifest parses");
    assert!(
        !manifest.case.is_empty(),
        "the vector set must not be empty"
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_active"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    for (index, case) in manifest.case.iter().enumerate() {
        let source = std::fs::read_to_string(dir.join(&case.file)).expect("vector source");
        let request = serde_json::json!({
            "id": index,
            "source": source,
            "name": case.id,
        });
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(&frame(request.to_string().as_bytes()))
            .expect("write frame");
        let response = read_frame(child.stdout.as_mut().expect("stdout"));
        assert_eq!(response["id"], index, "{}: {response}", case.id);
        let envelope = &response["envelope"];
        assert_eq!(
            envelope["receipt"]["outcome"], case.outcome,
            "{}: {response}",
            case.id
        );
        let codes: Vec<String> = envelope["receipt"]["diagnostics"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|code| code.as_str().map(str::to_string))
            .collect();
        for code in &case.codes {
            assert!(
                codes.iter().any(|found| found == code),
                "{}: expected {code}: {response}",
                case.id
            );
        }
    }
    child.stdin.take();
    let status = child.wait().expect("wait");
    assert!(status.success(), "serve exits cleanly on EOF");
}
