//! Windows-specific behaviour pins (LANG-H-006).
//!
//! Windows is a claimed release platform; these tests turn the platform's
//! implicit behaviours into verified ones:
//!
//! - `keygen` writes both halves and the secret is the documented 64-hex
//!   seed (Windows has no POSIX modes: the files inherit the directory
//!   ACL, stated in `keygen --help` and the threat model, so a DACL
//!   assertion would need Windows APIs the CLI does not depend on);
//! - `env.NAME` folds to uppercase over the real process snapshot, the
//!   same rule every platform follows.
#![cfg(windows)]

use std::process::Command;

fn active() -> Command {
    Command::new(env!("CARGO_BIN_EXE_active"))
}

fn fixture(name: &str, contents: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("active-cli-tests");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    path
}

#[test]
fn keygen_writes_both_halves_with_the_documented_shapes() {
    let dir = std::env::temp_dir()
        .join("active-cli-tests")
        .join("keygen-windows");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let prefix = dir.join("win");
    let out = active()
        .args(["keygen", "--out"])
        .arg(&prefix)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let secret = std::fs::read_to_string(dir.join("win.secret")).expect("secret file");
    let public = std::fs::read_to_string(dir.join("win.public")).expect("public file");
    assert_eq!(
        secret.trim().len(),
        64,
        "the secret is the 32-byte seed in hex"
    );
    assert!(secret.trim().chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        public.trim().len(),
        64,
        "the public half is the 32-byte key in hex"
    );
    assert!(public.trim().chars().all(|c| c.is_ascii_hexdigit()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn env_references_fold_case_over_the_windows_process_snapshot() {
    let f = fixture(
        "env_windows.active",
        "a = env.hk_fold_windows\nb = env.HK_FOLD_WINDOWS\n",
    );
    let out = active()
        .env("HK_FOLD_WINDOWS", "hello")
        .args(["eval"])
        .arg(&f)
        .output()
        .expect("run active");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("eval emits json");
    assert_eq!(json["a"], "hello");
    assert_eq!(json["b"], "hello");
}
