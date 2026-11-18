//! `active test` (L-P20): one pass over schema validation, lint,
//! fmt-check, and conformance vectors, with one exit code.
//!
//! No external test framework: the command walks `.active` files with
//! std, reuses the shipped parser/engine/linter/formatter, and runs the
//! repository conformance manifest through the same core APIs.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use active_core::diag::Severity;
use active_core::lint;
use active_core::{fmt, Engine, ParseOptions, Style};

#[derive(Debug, Default)]
pub(crate) struct Report {
    pub files: usize,
    pub failures: Vec<String>,
    pub notes: Vec<String>,
    pub cases: usize,
    pub case_failures: usize,
}

/// Run every requested check. `paths` may be files or directories; a
/// directory is walked for `.active` files (hidden/target/node_modules
/// subtrees are skipped, and the `conformance` subtree is skipped when
/// this run owns its manifest).
pub(crate) fn run(
    paths: &[PathBuf],
    manifest: Option<&Path>,
    strict: bool,
    env: &HashMap<String, String>,
) -> anyhow::Result<Report> {
    let mut report = Report::default();
    let mut manifests: Vec<PathBuf> = Vec::new();
    if let Some(path) = manifest {
        manifests.push(path.to_path_buf());
    }
    if manifest.is_none() {
        for path in paths {
            let candidate = path.join("conformance").join("manifest.toml");
            if path.is_dir() && candidate.is_file() && !manifests.contains(&candidate) {
                manifests.push(candidate);
            }
        }
        if paths.is_empty() {
            let candidate = PathBuf::from("conformance").join("manifest.toml");
            if candidate.is_file() {
                manifests.push(candidate);
            } else {
                anyhow::bail!(
                    "nothing to test: pass files/directories or --conformance <manifest>"
                );
            }
        }
    }
    // Only the conformance directory OWNED by a manifest is skipped; an
    // unowned or nested `conformance` directory is ordinary source.
    let owned_roots: Vec<PathBuf> = manifests
        .iter()
        .filter_map(|manifest| manifest.parent().map(Path::to_path_buf))
        .collect();

    let mut files = BTreeSet::new();
    for path in paths {
        if path.is_file() {
            files.insert(path.clone());
        } else if path.is_dir() {
            collect_active(path, 0, &owned_roots, &mut files)?;
        } else {
            anyhow::bail!("{} does not exist", path.display());
        }
    }
    for file in &files {
        report.files += 1;
        check_file(file, strict, env, &mut report);
    }
    for manifest in &manifests {
        run_conformance(manifest, &mut report);
    }
    if report.files == 0 && report.cases == 0 {
        report
            .failures
            .push("no checks ran: no .active files and no conformance cases".to_string());
    }
    Ok(report)
}

fn collect_active(
    dir: &Path,
    depth: usize,
    owned_roots: &[PathBuf],
    files: &mut BTreeSet<PathBuf>,
) -> anyhow::Result<()> {
    if depth > 64 {
        anyhow::bail!("directory nesting exceeds 64 at {}", dir.display());
    }
    if owned_roots.iter().any(|root| same_dir(root, dir)) {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| anyhow::anyhow!("cannot read {}: {e}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|e| anyhow::anyhow!("cannot inspect {}: {e}", path.display()))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if name.starts_with('.')
                || matches!(name.as_ref(), "target" | "node_modules")
                || owned_roots.iter().any(|root| same_dir(root, &path))
            {
                continue;
            }
            collect_active(&path, depth + 1, owned_roots, files)?;
        } else if metadata.is_file() && path.extension().is_some_and(|e| e == "active") {
            files.insert(path);
        }
    }
    Ok(())
}

/// Compare directories across relative/absolute spellings.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn check_file(path: &Path, strict: bool, env: &HashMap<String, String>, report: &mut Report) {
    let options = crate::with_parse_limits(ParseOptions::default());
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) => {
            report
                .failures
                .push(format!("read {}: {e}", path.display()));
            return;
        }
    };
    if !metadata.is_file() {
        report
            .failures
            .push(format!("read {}: not a regular file", path.display()));
        return;
    }
    if options.max_input_bytes != 0 && metadata.len() > options.max_input_bytes as u64 {
        report.failures.push(format!(
            "parse {}: input is {} bytes; the limit is {}",
            path.display(),
            metadata.len(),
            options.max_input_bytes
        ));
        return;
    }
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) => {
            report
                .failures
                .push(format!("read {}: {e}", path.display()));
            return;
        }
    };
    let name = path.to_string_lossy();
    // Syntax-only module for fmt (the formatter never merges includes),
    // and the resolved module for lint/eval.
    let syntax_options = crate::with_parse_limits(ParseOptions {
        resolve_includes: false,
        ..ParseOptions::default()
    });
    let syntax = match active_core::parser::parse_text_with_file(&src, &name, syntax_options, path)
    {
        Ok(module) => module,
        Err(diags) => {
            report.failures.push(format!(
                "parse {}: [{}]",
                path.display(),
                codes(&diags).join(",")
            ));
            return;
        }
    };
    if !syntax.healing.is_empty() {
        report.failures.push(format!(
            "repair {}: needs repair [{}] (run `active repair`)",
            path.display(),
            codes(&syntax.healing).join(",")
        ));
    } else if fmt::format(&syntax, Style::Indent) != src {
        report.failures.push(format!(
            "fmt {}: not canonical (run `active fmt --write`)",
            path.display()
        ));
    }
    let module = match active_core::parser::parse_text_with_file(&src, &name, options, path) {
        Ok(module) => module,
        Err(diags) => {
            report.failures.push(format!(
                "parse {}: [{}]",
                path.display(),
                codes(&diags).join(",")
            ));
            return;
        }
    };
    // Project lint policy travels with the file, exactly as `active
    // lint` applies it.
    let lint_options = match lint::LintConfig::discover(path) {
        Ok(Some((_, config))) => match config.options(&[]) {
            Ok(options) => options,
            Err(e) => {
                report
                    .failures
                    .push(format!("lint {}: {e}", path.display()));
                return;
            }
        },
        Ok(None) => lint::LintOptions::default(),
        Err(e) => {
            report
                .failures
                .push(format!("lint {}: {e}", path.display()));
            return;
        }
    };
    let findings = lint::lint_with(&module, &lint_options);
    let lint_fatal = findings
        .iter()
        .any(|d| d.severity == Severity::Error || (strict && d.severity == Severity::Warning));
    if lint_fatal {
        report.failures.push(format!(
            "lint {}: [{}]",
            path.display(),
            codes(&findings).join(",")
        ));
    } else if !findings.is_empty() {
        report.notes.push(format!(
            "lint {}: {} finding(s) [{}]",
            path.display(),
            findings.len(),
            codes(&findings).join(",")
        ));
    }
    // Strict, hermetic evaluation: schema repairs are not silently
    // downgraded. Warnings fail only under --strict and are notes
    // otherwise.
    let engine = Engine::strict().with_env(env.clone());
    match engine.evaluate(&module) {
        Ok((_, diags)) => {
            let fatal = diags.iter().any(|d| {
                d.severity == Severity::Error || (strict && d.severity == Severity::Warning)
            });
            if fatal {
                report.failures.push(format!(
                    "schema {}: [{}]",
                    path.display(),
                    codes(&diags).join(",")
                ));
            } else if !diags.is_empty() {
                report.notes.push(format!(
                    "schema {}: {} warning(s) [{}]",
                    path.display(),
                    diags.len(),
                    codes(&diags).join(",")
                ));
            }
        }
        Err(active_core::EvalError::ValidationFailed { diagnostics }) => {
            report.failures.push(format!(
                "schema {}: [{}]",
                path.display(),
                codes(&diagnostics).join(",")
            ));
        }
        Err(error) => report
            .failures
            .push(format!("schema {}: {}", path.display(), error.code())),
    }
}

fn run_conformance(manifest: &Path, report: &mut Report) {
    let mut cases = 0usize;
    let mut failed = 0usize;
    let text = match std::fs::read_to_string(manifest) {
        Ok(text) => text,
        Err(e) => {
            report.failures.push(format!(
                "conformance {}: cannot read: {e}",
                manifest.display()
            ));
            return;
        }
    };
    let doc: toml::Value = match toml::from_str(&text) {
        Ok(doc) => doc,
        Err(e) => {
            report
                .failures
                .push(format!("conformance {}: {e}", manifest.display()));
            return;
        }
    };
    if let Some(version) = doc.get("version").and_then(|v| v.as_integer()) {
        if version != 1 {
            report.failures.push(format!(
                "conformance {}: unsupported manifest version {version}",
                manifest.display()
            ));
            return;
        }
    }
    let root = manifest.parent().unwrap_or_else(|| Path::new("."));
    let Some(list) = doc.get("cases").and_then(|v| v.as_array()) else {
        report
            .failures
            .push(format!("conformance {}: no [[cases]]", manifest.display()));
        return;
    };
    if list.is_empty() {
        report.failures.push(format!(
            "conformance {}: manifest has no cases",
            manifest.display()
        ));
        return;
    }
    for case in list {
        cases += 1;
        let (Some(id), Some(file), Some(kind)) = (
            case.get("id").and_then(|v| v.as_str()),
            case.get("file").and_then(|v| v.as_str()),
            case.get("kind").and_then(|v| v.as_str()),
        ) else {
            failed += 1;
            report
                .failures
                .push("conformance: case without id/file/kind".to_string());
            continue;
        };
        let path = root.join(file);
        let want_codes = if kind == "clean" {
            Vec::new()
        } else {
            match read_codes(root, id) {
                Ok(codes) => codes,
                Err(e) => {
                    failed += 1;
                    report.failures.push(format!("conformance {id}: {e}"));
                    continue;
                }
            }
        };
        if let Err(e) = check_case(&path, kind, case, &want_codes, root, id) {
            failed += 1;
            report.failures.push(format!("conformance {id}: {e}"));
        }
    }
    report.cases += cases;
    report.case_failures += failed;
    report.notes.push(format!(
        "conformance {}: {} case(s), {} failure(s)",
        manifest.display(),
        cases,
        failed
    ));
}

fn read_codes(root: &Path, id: &str) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(root.join("errors").join(format!("{id}.json")))
        .map_err(|e| format!("cannot read expected codes: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad expected codes: {e}"))?;
    let list = value
        .get("codes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "expected codes file has no `codes` array".to_string())?;
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        out.push(
            item.as_str()
                .ok_or_else(|| "expected codes contain a non-string entry".to_string())?
                .to_string(),
        );
    }
    Ok(out)
}

fn check_case(
    path: &Path,
    kind: &str,
    case: &toml::Value,
    want_codes: &[String],
    root: &Path,
    id: &str,
) -> Result<(), String> {
    let mut options = crate::with_parse_limits(ParseOptions::default());
    options.enable_includes = true;
    let module = match active_core::parse_file(path, options) {
        Ok(module) => module,
        Err(diags) => {
            let got = norm(codes(&diags));
            return if got == norm(want_codes.to_vec()) && kind == "refused" {
                Ok(())
            } else {
                Err(format!(
                    "parse refused [{}]; wanted [{}] ({kind})",
                    got.join(","),
                    norm(want_codes.to_vec()).join(",")
                ))
            };
        }
    };
    let mut engine = Engine::new().with_env(HashMap::new());
    engine.profile_override = case
        .get("profile")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    match engine.evaluate(&module) {
        Ok((value, diags)) => {
            if kind == "refused" {
                // The reference suite compares refused codes exactly and
                // requires a non-empty set (NORMALIZATION.md).
                let got = codes(&diags);
                if got.is_empty() || got != want_codes {
                    return Err(format!(
                        "refused codes [{}]; wanted [{}]",
                        got.join(","),
                        want_codes.join(",")
                    ));
                }
                return Ok(());
            }
            let got = norm(codes(&diags));
            let want = if kind == "clean" {
                Vec::new()
            } else {
                norm(want_codes.to_vec())
            };
            if got != want {
                return Err(format!(
                    "codes [{}]; wanted [{}] ({kind})",
                    got.join(","),
                    want.join(",")
                ));
            }
            let json = active_core::convert::value_to_pretty_json(&value);
            let got_value: serde_json::Value =
                serde_json::from_str(&json).map_err(|e| format!("bad value JSON: {e}"))?;
            let text = std::fs::read_to_string(root.join("expected").join(format!("{id}.json")))
                .map_err(|e| format!("cannot read expected value: {e}"))?;
            let want_value: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("bad expected value: {e}"))?;
            if got_value != want_value {
                return Err("value diverged from expected/<id>.json".to_string());
            }
            Ok(())
        }
        Err(active_core::EvalError::ValidationFailed { diagnostics }) => {
            let got = norm(codes(&diagnostics));
            if !got.is_empty() && got == norm(want_codes.to_vec()) && kind == "refused" {
                Ok(())
            } else {
                Err(format!(
                    "validation refused [{}]; wanted [{}] ({kind})",
                    got.join(","),
                    norm(want_codes.to_vec()).join(",")
                ))
            }
        }
        Err(error) => {
            let got = vec![error.code().to_string()];
            if got == want_codes && kind == "refused" {
                Ok(())
            } else {
                Err(format!(
                    "refused [{}]; wanted [{}] ({kind})",
                    got.join(","),
                    want_codes.join(",")
                ))
            }
        }
    }
}

fn codes(diags: &[active_core::Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.clone()).collect()
}

fn norm(mut codes: Vec<String>) -> Vec<String> {
    codes.sort();
    codes.dedup();
    codes
}
