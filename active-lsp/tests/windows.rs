//! Windows-specific LSP behaviour pins (LANG-H-006).
//!
//! A `file://` URI is the only identity the server sees for a document.
//! On Windows a client sends drive-letter paths with backslashes; the
//! server must be able to turn that URI back into the exact filesystem
//! path it stamps on diagnostics and include nodes.
#![cfg(windows)]

use lsp_types::Url;

#[test]
fn file_urls_round_trip_windows_paths() {
    let path = std::path::Path::new(r"C:\temp\active dir\main.active");
    let url = Url::from_file_path(path).expect("file url");
    assert_eq!(
        url.as_str(),
        "file:///C:/temp/active%20dir/main.active",
        "forward slashes and percent-encoding are the wire form"
    );
    let back = url.to_file_path().expect("path back");
    assert_eq!(back, path, "the round trip is exact");
}

#[test]
fn case_differing_spellings_resolve_through_the_filesystem() {
    // Windows is case-insensitive: two spellings of one existing file are
    // the same document. Identity comes from canonicalization, not from a
    // case-folding shortcut (LANG-H-005).
    let dir = std::env::temp_dir().join(format!("active-lsp-win-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let upper = dir.join("Main.active");
    std::fs::write(&upper, "a = 1\n").expect("write");
    let lower = dir.join("main.active");
    // Writing the lower-case spelling lands on the same file.
    std::fs::write(&lower, "a = 1\n").expect("write lower");
    let a = Url::from_file_path(&upper).expect("upper url").to_string();
    let b = Url::from_file_path(&lower).expect("lower url").to_string();
    assert!(
        active_ls::analysis::same_uri(&a, &b),
        "case-insensitive filesystems resolve both spellings to one file"
    );
    assert!(active_ls::analysis::same_uri(&a, &a));
    let _ = std::fs::remove_dir_all(&dir);
}
