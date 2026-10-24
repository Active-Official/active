//! Playground repair loop (GAP-CLOSURE §2.10): drives the shipped CLI
//! through generate -> check -> repair plan -> partial apply -> eval ->
//! full repair -> eval, and asserts each verdict. The playground demo in
//! `playground/loop/` runs the same commands; this test keeps the
//! transcript honest.

use std::path::{Path, PathBuf};
use std::process::Command;

fn active() -> Command {
    Command::new(env!("CARGO_BIN_EXE_active"))
}

fn doc() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("playground")
        .join("loop")
        .join("broken.active")
}

fn workdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("active-loop-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[test]
fn playground_loop_is_truthful_and_idempotent() {
    let doc = doc();
    let work = workdir();

    // 1. Generate a JSON Schema contract from the same document.
    let generated = active()
        .args(["generate", "--from"])
        .arg(&doc)
        .args(["--to", "jsonschema"])
        .output()
        .expect("run active");
    assert!(
        generated.status.success(),
        "generate: {}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let schema = String::from_utf8_lossy(&generated.stdout);
    assert!(schema.contains("\"type\": \"object\""), "{schema}");
    assert!(schema.contains("additionalProperties"), "{schema}");
    assert!(schema.contains("application_database_port"), "{schema}");

    // 2. Check reports the typos.
    let checked = active()
        .arg("check")
        .arg(&doc)
        .output()
        .expect("run active");
    assert!(!checked.status.success(), "broken document must fail check");
    let text = String::from_utf8_lossy(&checked.stderr);
    assert!(text.contains("T005"), "{text}");
    assert!(text.contains("application_database_prt"), "{text}");

    // 3. The plan is multi-step and safe.
    let planned = active()
        .args(["repair", "--from"])
        .arg(&doc)
        .args(["--format", "json"])
        .output()
        .expect("run active");
    assert!(
        planned.status.success(),
        "{}",
        String::from_utf8_lossy(&planned.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&planned.stdout).expect("plan JSON");
    let steps = plan["steps"].as_array().expect("steps");
    assert!(steps.len() >= 2, "expected a multi-step plan: {plan}");

    // 4. Partial apply writes exactly the reported subset and the
    //    subset still fails: one rename cannot fix both typos.
    let partial_path = work.join("partial.active");
    let partial = active()
        .args(["repair", "--from"])
        .arg(&doc)
        .args(["--apply-step", "0", "--format", "json", "--output"])
        .arg(&partial_path)
        .output()
        .expect("run active");
    assert!(
        partial.status.success(),
        "{}",
        String::from_utf8_lossy(&partial.stderr)
    );
    let applied: serde_json::Value = serde_json::from_slice(&partial.stdout).expect("partial JSON");
    let written = std::fs::read_to_string(&partial_path).expect("partial file");
    assert_eq!(
        applied["repaired_source"].as_str().expect("source"),
        written,
        "reported source must equal the written file"
    );
    assert!(
        applied["steps"][0]["status"] == "applied",
        "selected step must be applied: {applied}"
    );
    assert!(
        applied["steps"][1]["status"].get("skipped").is_some(),
        "unselected step must be skipped: {applied}"
    );
    assert_eq!(applied["clean"], false, "{applied}");
    let partial_eval = active()
        .arg("eval")
        .arg(&partial_path)
        .output()
        .expect("run active");
    assert!(
        !partial_eval.status.success(),
        "one rename must leave the other defect"
    );

    // 5. The full plan reaches clean and writes the repaired file.
    let repaired_path = work.join("repaired.active");
    let repaired = active()
        .args(["repair", "--from"])
        .arg(&doc)
        .arg("--output")
        .arg(&repaired_path)
        .output()
        .expect("run active");
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );

    // 6. The repaired document evaluates, and a second plan is empty.
    let evaluated = active()
        .arg("eval")
        .arg(&repaired_path)
        .output()
        .expect("run active");
    assert!(
        evaluated.status.success(),
        "{}",
        String::from_utf8_lossy(&evaluated.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&evaluated.stdout).expect("eval JSON");
    assert_eq!(value["application_database_port"], 8080, "{value}");
    assert_eq!(value["application_database_host"], "localhost", "{value}");

    let again = active()
        .args(["repair", "--from"])
        .arg(&repaired_path)
        .args(["--format", "json"])
        .output()
        .expect("run active");
    assert!(again.status.success());
    let replanned: serde_json::Value = serde_json::from_slice(&again.stdout).expect("plan JSON");
    assert_eq!(replanned["clean"], true, "{replanned}");
    assert_eq!(
        replanned["steps"].as_array().map(Vec::len),
        Some(0),
        "second run must have no steps: {replanned}"
    );
}
